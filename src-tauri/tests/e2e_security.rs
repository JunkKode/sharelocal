//! Security-facing behaviour: tokens, PINs, path traversal, size limits and
//! filesystem permissions. Every test drives the real HTTP/WebSocket surface.

mod harness;

use futures_util::{SinkExt, StreamExt};
use harness::*;
use sharelocal::model::{SessionStatus, TransferError};
use sharelocal::server::protocol::ControlMsg;
use sharelocal::transfer::send::start_send;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const T: Duration = Duration::from_secs(30);

/// Open a control channel and complete the handshake.
async fn open_control(
    node: &Node,
) -> (
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let (socket, _) = tokio_tungstenite::connect_async(node.ws_url()).await.unwrap();
    let (mut sink, stream) = socket.split();
    sink.send(Message::Text(
        ControlMsg::Hello { device: device("tester", "Tester") }.to_text().into(),
    ))
    .await
    .unwrap();
    (sink, stream)
}

async fn next_control<S>(stream: &mut S) -> ControlMsg
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("the server should answer")
            .expect("the stream should stay open")
            .expect("a valid frame");
        if let Message::Text(text) = msg {
            if let Some(parsed) = ControlMsg::parse(&text) {
                return parsed;
            }
        }
    }
}

fn offer(paths: &[(&str, u64)]) -> ControlMsg {
    let files: Vec<sharelocal::model::FileMeta> = paths
        .iter()
        .map(|(p, size)| sharelocal::model::FileMeta {
            rel_path: (*p).into(),
            size: *size,
            mime: None,
            modified_ms: None,
        })
        .collect();
    ControlMsg::Offer {
        total_bytes: files.iter().map(|f| f.size).sum(),
        files,
        pin: None,
        direction: sharelocal::model::Direction::Push,
        conflict: sharelocal::model::ConflictPolicy::KeepBoth,
            kind: sharelocal::model::PayloadKind::Files,
            text: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_traversal_path_is_refused_before_a_session_exists() {
    let node = start_node("Target", true).await;
    let (mut sink, mut stream) = open_control(&node).await;
    let _welcome = next_control(&mut stream).await;

    for evil in [
        "../../../../etc/passwd",
        "/etc/passwd",
        "..\\..\\Windows\\System32\\drivers\\etc\\hosts",
        "C:/Windows/system.ini",
        "ok/../../escape.txt",
    ] {
        sink.send(Message::Text(offer(&[(evil, 16)]).to_text().into()))
            .await
            .unwrap();
        let reply = next_control(&mut stream).await;
        match reply {
            ControlMsg::Error { code, .. } => {
                assert_eq!(code, "invalid-path", "wrong rejection for {evil}");
            }
            other => panic!("{evil} was not rejected: {other:?}"),
        }
    }

    assert!(
        node.sessions().await.is_empty(),
        "a hostile offer must not create a session"
    );
    // And nothing was written anywhere outside the download folder.
    assert!(!std::path::Path::new("/tmp/escape.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_incorrect_pin_is_rejected() {
    let node = start_node("Target", true).await;
    node.state.settings.write().await.pin = Some("4821".into());

    let (mut sink, mut stream) = open_control(&node).await;
    match next_control(&mut stream).await {
        ControlMsg::Welcome { pin_required, .. } => {
            assert!(pin_required, "the peer is told a PIN is needed");
        }
        other => panic!("expected a welcome, got {other:?}"),
    }

    // No PIN at all.
    sink.send(Message::Text(offer(&[("x.bin", 4)]).to_text().into())).await.unwrap();
    assert!(matches!(
        next_control(&mut stream).await,
        ControlMsg::Error { ref code, .. } if code == "pin-rejected"
    ));

    // Wrong PIN.
    let ControlMsg::Offer { files, total_bytes, direction, conflict, .. } = offer(&[("x.bin", 4)])
    else {
        unreachable!()
    };
    sink.send(Message::Text(
        ControlMsg::Offer {
            files: files.clone(),
            total_bytes,
            pin: Some("0000".into()),
            direction,
            conflict,
            kind: sharelocal::model::PayloadKind::Files,
            text: None,
        }
        .to_text()
        .into(),
    ))
    .await
    .unwrap();
    assert!(matches!(
        next_control(&mut stream).await,
        ControlMsg::Error { ref code, .. } if code == "pin-rejected"
    ));
    assert!(node.sessions().await.is_empty());

    // The correct PIN gets through.
    sink.send(Message::Text(
        ControlMsg::Offer {
            files,
            total_bytes,
            pin: Some("4821".into()),
            direction,
            conflict,
            kind: sharelocal::model::PayloadKind::Files,
            text: None,
        }
        .to_text()
        .into(),
    ))
    .await
    .unwrap();
    assert!(matches!(
        next_control(&mut stream).await,
        ControlMsg::OfferStatus { status: SessionStatus::Accepted, .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sender_that_knows_the_pin_completes_the_transfer() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;
    receiver.state.settings.write().await.pin = Some("135790".into());

    let source = sender.dir.join("secret.bin");
    let expected = make_file(&source, 4096);
    let peer = receiver.as_peer().await;

    let id = start_send(&sender.state, peer.clone(), vec![source.clone()], Some("000000".into()))
        .await
        .unwrap();
    let rejected = await_terminal(&sender, &id, T).await;
    assert!(
        matches!(rejected.error, Some(TransferError::PinRejected(_)) | Some(TransferError::Other(_))),
        "wrong PIN should fail: {:?}",
        rejected.error
    );

    let id = start_send(&sender.state, peer, vec![source], Some("135790".into()))
        .await
        .unwrap();
    let ok = await_terminal(&sender, &id, T).await;
    assert_eq!(ok.status, SessionStatus::Completed, "{:?}", ok.error);
    assert_eq!(
        std::fs::read(receiver.downloads.join("secret.bin")).unwrap(),
        expected
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_data_plane_refuses_a_bad_token() {
    let node = start_node("Target", true).await;
    let (mut sink, mut stream) = open_control(&node).await;
    let _welcome = next_control(&mut stream).await;

    sink.send(Message::Text(offer(&[("payload.bin", 8)]).to_text().into()))
        .await
        .unwrap();
    let (session_id, good_token) = match next_control(&mut stream).await {
        ControlMsg::OfferStatus { session_id, token: Some(token), .. } => (session_id, token),
        other => panic!("expected an accepted offer, got {other:?}"),
    };

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("{}/api/transfer/{session_id}/0", node.base_url());

    for bad in ["", "not-the-token", &good_token[..good_token.len() - 1]] {
        let response = client
            .put(&url)
            .header("Authorization", format!("Bearer {bad}"))
            .body(vec![0u8; 8])
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "token {bad:?} should not be accepted"
        );
    }

    // A completely unknown session is refused even with a well-formed token.
    let response = client
        .put(format!("{}/api/transfer/not-a-session/0", node.base_url()))
        .header("Authorization", format!("Bearer {good_token}"))
        .body(vec![0u8; 8])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Nothing was written by any of those attempts.
    assert!(!node.downloads.join("payload.bin").exists());

    // The real token still works, so the check is not simply refusing everything.
    let response = client
        .put(&url)
        .header("Authorization", format!("Bearer {good_token}"))
        .body(vec![7u8; 8])
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "the genuine token must work");
    assert_eq!(std::fs::read(node.downloads.join("payload.bin")).unwrap(), vec![7u8; 8]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sender_cannot_write_more_than_it_declared() {
    let node = start_node("Target", true).await;
    let (mut sink, mut stream) = open_control(&node).await;
    let _welcome = next_control(&mut stream).await;

    sink.send(Message::Text(offer(&[("small.bin", 10)]).to_text().into()))
        .await
        .unwrap();
    let (session_id, token) = match next_control(&mut stream).await {
        ControlMsg::OfferStatus { session_id, token: Some(token), .. } => (session_id, token),
        other => panic!("expected acceptance, got {other:?}"),
    };

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .put(format!("{}/api/transfer/{session_id}/0", node.base_url()))
        .header("Authorization", format!("Bearer {token}"))
        // Ten bytes were announced; try to write a megabyte.
        .body(vec![0u8; 1024 * 1024])
        .send()
        .await
        .unwrap();

    assert!(!response.status().is_success(), "the overrun must be refused");
    assert!(
        !node.downloads.join("small.bin").exists(),
        "an over-long upload must not be finalised"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_info_endpoint_leaks_nothing_sensitive() {
    let node = start_node("Target", true).await;
    node.state.settings.write().await.pin = Some("998877".into());

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let body = client
        .get(format!("{}/api/info", node.base_url()))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(body.contains("\"pin_required\":true"), "peers must know a PIN is needed");
    assert!(!body.contains("998877"), "the PIN itself must never be published");
    assert!(!body.contains("download_dir"), "local paths must not be published");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unauthenticated_browser_offer_is_refused() {
    let node = start_node("Target", true).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let response = client
        .post(format!("{}/api/web/offer", node.base_url()))
        .json(&serde_json::json!({
            "files": [{ "rel_path": "x.bin", "size": 4 }],
            "client_id": "web-deadbeefdeadbeef"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(node.sessions().await.is_empty());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_download_folder_reports_a_permission_error() {
    use std::os::unix::fs::PermissionsExt;

    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    let locked = receiver.dir.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    receiver.state.settings.write().await.download_dir = locked.clone();

    let source = sender.dir.join("blocked.bin");
    make_file(&source, 2048);
    let peer = receiver.as_peer().await;
    let id = start_send(&sender.state, peer, vec![source], None).await.unwrap();

    let finished = await_terminal(&sender, &id, T).await;
    assert_eq!(finished.status, SessionStatus::Failed);
    let error = finished.error.expect("a reason is reported");
    assert!(
        matches!(error, TransferError::PermissionDenied(_) | TransferError::Other(_)),
        "unexpected error for a read-only folder: {error:?}"
    );

    // Restore permissions so the temp dir can be cleaned up.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn binary_frames_are_not_accepted_on_the_control_channel() {
    let node = start_node("Target", true).await;
    let (socket, _) = tokio_tungstenite::connect_async(node.ws_url()).await.unwrap();
    let (mut sink, mut stream) = socket.split();

    sink.send(Message::Binary(vec![0u8; 64].into())).await.unwrap();
    match next_control(&mut stream).await {
        ControlMsg::Error { code, .. } => assert_eq!(code, "protocol"),
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_offer_is_refused() {
    let node = start_node("Target", true).await;
    let (mut sink, mut stream) = open_control(&node).await;
    let _welcome = next_control(&mut stream).await;

    sink.send(Message::Text(offer(&[]).to_text().into())).await.unwrap();
    match next_control(&mut stream).await {
        ControlMsg::Error { code, .. } => assert_eq!(code, "empty-offer"),
        other => panic!("expected an empty-offer error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pausing_sharing_makes_the_device_refuse_transfers() {
    let node = start_node("Target", true).await;
    node.state.set_sharing(false);

    let (mut sink, mut stream) = open_control(&node).await;
    let _welcome = next_control(&mut stream).await;
    sink.send(Message::Text(offer(&[("x.bin", 4)]).to_text().into())).await.unwrap();

    match next_control(&mut stream).await {
        ControlMsg::Error { code, .. } => assert_eq!(code, "sharing-off"),
        other => panic!("expected sharing to be off, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_text_share_is_refused() {
    let node = start_node("Target", true).await;
    let (mut sink, mut stream) = open_control(&node).await;
    let _welcome = next_control(&mut stream).await;

    let huge = "x".repeat(sharelocal::model::MAX_TEXT_BYTES + 1);
    sink.send(Message::Text(
        ControlMsg::Offer {
            files: vec![sharelocal::model::FileMeta {
                rel_path: "shared text".into(),
                size: huge.len() as u64,
                mime: None,
                modified_ms: None,
            }],
            total_bytes: huge.len() as u64,
            pin: None,
            direction: sharelocal::model::Direction::Push,
            conflict: sharelocal::model::ConflictPolicy::KeepBoth,
            kind: sharelocal::model::PayloadKind::Text,
            text: Some(huge),
        }
        .to_text()
        .into(),
    ))
    .await
    .unwrap();

    match next_control(&mut stream).await {
        ControlMsg::Error { code, .. } => assert_eq!(code, "text-too-long"),
        other => panic!("expected the snippet to be refused, got {other:?}"),
    }
    assert!(node.sessions().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_text_share_is_refused() {
    let node = start_node("Target", true).await;
    let (mut sink, mut stream) = open_control(&node).await;
    let _welcome = next_control(&mut stream).await;

    sink.send(Message::Text(
        ControlMsg::Offer {
            files: vec![sharelocal::model::FileMeta {
                rel_path: "shared text".into(),
                size: 0,
                mime: None,
                modified_ms: None,
            }],
            total_bytes: 0,
            pin: None,
            direction: sharelocal::model::Direction::Push,
            conflict: sharelocal::model::ConflictPolicy::KeepBoth,
            kind: sharelocal::model::PayloadKind::Text,
            text: None,
        }
        .to_text()
        .into(),
    ))
    .await
    .unwrap();

    match next_control(&mut stream).await {
        ControlMsg::Error { code, .. } => assert_eq!(code, "empty-offer"),
        other => panic!("expected an empty-offer error, got {other:?}"),
    }
}
