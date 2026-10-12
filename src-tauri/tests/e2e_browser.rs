//! The phone-without-an-app path: QR pairing, browser upload, browser download.

mod harness;

use harness::*;
use sharelocal::model::{ConflictPolicy, Direction, Role, SessionStatus};
use sharelocal::transfer::session::Session;
use std::time::Duration;

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Redeem a pairing token exactly as the browser client does.
async fn join(node: &Node, pairing: &str, client_id: &str) -> reqwest::Response {
    client()
        .post(format!("{}/api/web/session", node.base_url()))
        .json(&serde_json::json!({
            "pairing_token": pairing,
            "client_id": client_id,
            "name": "iPhone · Safari",
            "os": "ios",
            "kind": "phone",
        }))
        .send()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_pairs_and_uploads_a_file() {
    let node = start_node("Studio Mac", true).await;
    let (url, pairing) = node.state.issue_pairing().await;
    assert!(url.contains("/?t="), "the QR encodes a pairing link: {url}");

    let response = join(&node, &pairing, "aaaabbbbccccdddd").await;
    assert!(response.status().is_success(), "pairing should succeed");
    let body: serde_json::Value = response.json().await.unwrap();
    let client_id = body["client_id"].as_str().unwrap().to_string();
    let client_token = body["client_token"].as_str().unwrap().to_string();
    assert_eq!(client_id, "web-aaaabbbbccccdddd");

    // The phone now shows up as a peer on the desktop.
    assert!(
        node.state.peers.read().await.get(&client_id).is_some(),
        "a paired browser becomes a visible device"
    );

    // Offer a file, then stream it up.
    let payload = vec![42u8; 200_000];
    let offer: serde_json::Value = client()
        .post(format!("{}/api/web/offer", node.base_url()))
        .header("Authorization", format!("Bearer {client_token}"))
        .json(&serde_json::json!({
            "files": [{ "rel_path": "holiday.jpg", "size": payload.len() }],
            "client_id": client_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(offer["status"], "accepted", "auto-accept is on for this node");
    let session_id = offer["session_id"].as_str().unwrap();
    let token = offer["token"].as_str().unwrap();

    let upload = client()
        .put(format!("{}/api/transfer/{session_id}/0", node.base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .header("X-Offset", "0")
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert!(upload.status().is_success(), "upload failed: {:?}", upload.status());

    let landed = node.downloads.join("holiday.jpg");
    assert!(landed.exists());
    assert_eq!(std::fs::read(&landed).unwrap(), payload);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_upload_waits_for_approval_when_auto_accept_is_off() {
    let node = start_node("Studio Mac", false).await;
    let (_url, pairing) = node.state.issue_pairing().await;
    let body: serde_json::Value = join(&node, &pairing, "1111222233334444").await.json().await.unwrap();
    let client_id = body["client_id"].as_str().unwrap().to_string();
    let client_token = body["client_token"].as_str().unwrap().to_string();

    let offer: serde_json::Value = client()
        .post(format!("{}/api/web/offer", node.base_url()))
        .header("Authorization", format!("Bearer {client_token}"))
        .json(&serde_json::json!({
            "files": [{ "rel_path": "doc.pdf", "size": 16 }],
            "client_id": client_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(offer["status"], "pending");
    assert!(offer["token"].is_null(), "a pending offer carries no transfer token");
    let session_id = offer["session_id"].as_str().unwrap().to_string();

    // Polling with the client token shows it is still waiting.
    let status: serde_json::Value = client()
        .get(format!("{}/api/offer/{session_id}", node.base_url()))
        .header("Authorization", format!("Bearer {client_token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["status"], "pending");

    // The desktop user accepts; the token is then released.
    node.accept_pending(ConflictPolicy::KeepBoth).await.unwrap();
    let status: serde_json::Value = client()
        .get(format!("{}/api/offer/{session_id}", node.base_url()))
        .header("Authorization", format!("Bearer {client_token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["status"], "accepted");
    assert!(status["token"].is_string(), "an accepted offer releases the token");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_or_wrong_pairing_token_is_refused() {
    let node = start_node("Studio Mac", true).await;
    let (_url, pairing) = node.state.issue_pairing().await;

    assert_eq!(
        join(&node, "totally-wrong", "aaaabbbbccccdddd").await.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    // Revoking is immediate — the code on screen stops working.
    node.state.revoke_pairing().await;
    assert_eq!(
        join(&node, &pairing, "aaaabbbbccccdddd").await.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_client_id_is_refused() {
    let node = start_node("Studio Mac", true).await;
    let (_url, pairing) = node.state.issue_pairing().await;
    // Too little entropy to be a real browser identity.
    assert!(!join(&node, &pairing, "abc").await.status().is_success());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_downloads_a_file_the_desktop_offered() {
    let node = start_node("Studio Mac", true).await;

    let source = node.dir.join("presentation.key");
    let expected = make_file(&source, 300_000);

    // The desktop offers files for the browser to pull.
    let mut session = Session::new(
        device("web-phone", "iPhone"),
        vec![sharelocal::model::FileMeta {
            rel_path: "presentation.key".into(),
            size: expected.len() as u64,
            mime: None,
            modified_ms: None,
        }],
        Direction::Pull,
        Role::Sending,
        ConflictPolicy::KeepBoth,
        node.downloads.clone(),
    );
    session.source_paths = vec![source];
    session.set_status(SessionStatus::Accepted);
    let token = session.token().to_string();
    let session_id = node.state.sessions.write().await.insert(session);

    // The phone fetches it with the token in the query string, which is what a
    // plain `<a download>` can carry.
    let response = client()
        .get(format!(
            "{}/api/transfer/{session_id}/0?token={token}",
            node.base_url()
        ))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    let disposition = response
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        disposition.contains("presentation.key"),
        "the browser is told the file name: {disposition}"
    );

    let got = response.bytes().await.unwrap();
    assert_eq!(got.len(), expected.len());
    assert_eq!(got.as_ref(), expected.as_slice());

    let finished = await_terminal(&node, &session_id, Duration::from_secs(15)).await;
    assert_eq!(finished.status, SessionStatus::Completed);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_can_be_resumed_with_a_range_request() {
    let node = start_node("Studio Mac", true).await;

    let source = node.dir.join("video.mp4");
    let expected = make_file(&source, 120_000);

    let mut session = Session::new(
        device("web-phone", "iPhone"),
        vec![sharelocal::model::FileMeta {
            rel_path: "video.mp4".into(),
            size: expected.len() as u64,
            mime: None,
            modified_ms: None,
        }],
        Direction::Pull,
        Role::Sending,
        ConflictPolicy::KeepBoth,
        node.downloads.clone(),
    );
    session.source_paths = vec![source];
    session.set_status(SessionStatus::Accepted);
    let token = session.token().to_string();
    let session_id = node.state.sessions.write().await.insert(session);

    let response = client()
        .get(format!(
            "{}/api/transfer/{session_id}/0?token={token}",
            node.base_url()
        ))
        .header("Range", "bytes=50000-")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok()),
        Some("bytes 50000-119999/120000")
    );
    let got = response.bytes().await.unwrap();
    assert_eq!(got.as_ref(), &expected[50_000..]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_browser_interface_is_served_from_the_binary() {
    let node = start_node("Studio Mac", true).await;
    let response = client().get(node.base_url()).send().await.unwrap();

    assert!(response.status().is_success());
    let headers = response.headers().clone();
    assert_eq!(
        headers.get("x-content-type-options").and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert!(
        headers
            .get("content-security-policy")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .contains("frame-ancestors 'none'"),
        "the served page must not be embeddable"
    );

    let body = response.text().await.unwrap();
    assert!(body.contains("<title>ShareLocal</title>") || body.contains("ShareLocal"));

    // Client-side routes fall back to the app shell rather than 404ing.
    let deep = client()
        .get(format!("{}/some/client/route", node.base_url()))
        .send()
        .await
        .unwrap();
    assert!(deep.status().is_success());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_shares_text_with_the_desktop() {
    let node = start_node("Studio Mac", true).await;
    let (_url, pairing) = node.state.issue_pairing().await;
    let body: serde_json::Value = join(&node, &pairing, "ccccddddeeeeffff").await.json().await.unwrap();
    let client_id = body["client_id"].as_str().unwrap().to_string();
    let client_token = body["client_token"].as_str().unwrap().to_string();

    let snippet = "https://example.com/link-from-my-phone";
    let offer: serde_json::Value = client()
        .post(format!("{}/api/web/offer", node.base_url()))
        .header("Authorization", format!("Bearer {client_token}"))
        .json(&serde_json::json!({
            "files": [{ "rel_path": "shared text", "size": snippet.len() }],
            "client_id": client_id,
            "kind": "text",
            "text": snippet,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(offer["status"], "accepted");

    // No upload step: the snippet arrived with the offer and is already saved.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let saved = loop {
        let found: Vec<_> = std::fs::read_dir(&node.downloads)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "txt").unwrap_or(false))
            .collect();
        if !found.is_empty() {
            break found;
        }
        assert!(std::time::Instant::now() < deadline, "the snippet was never written");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(std::fs::read_to_string(&saved[0]).unwrap(), snippet);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_cannot_smuggle_a_path_through_a_file_offer() {
    let node = start_node("Studio Mac", true).await;
    let (_url, pairing) = node.state.issue_pairing().await;
    let body: serde_json::Value = join(&node, &pairing, "99998888777766aa").await.json().await.unwrap();
    let client_id = body["client_id"].as_str().unwrap().to_string();
    let client_token = body["client_token"].as_str().unwrap().to_string();

    let response = client()
        .post(format!("{}/api/web/offer", node.base_url()))
        .header("Authorization", format!("Bearer {client_token}"))
        .json(&serde_json::json!({
            "files": [{ "rel_path": "../../../../etc/passwd", "size": 8 }],
            "client_id": client_id,
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(node.sessions().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_peer_is_never_dialled_directly() {
    // A browser has no server of its own, so its peer record carries port 0.
    // Dialling it produced "Can't assign requested address (os error 49)";
    // the transfer has to be turned around into something it can fetch.
    let node = start_node("Studio Mac", true).await;

    let phone = sharelocal::model::Peer {
        device: sharelocal::model::DeviceInfo {
            id: "web-phone".into(),
            name: "iPhone · Safari".into(),
            os: "ios".into(),
            kind: "browser".into(),
            version: sharelocal::model::PROTOCOL_VERSION,
            port: 0,
            tls: false,
            pin_required: false,
        },
        addresses: vec!["127.0.0.1".parse().unwrap()],
        primary: "127.0.0.1".parse().unwrap(),
        source: sharelocal::model::DiscoverySource::Qr,
        status: sharelocal::model::PeerStatus::Online,
        same_subnet: true,
        link: sharelocal::model::LinkKind::Wifi,
        rtt_ms: None,
        last_speed_bps: None,
        last_seen_ms: 0,
    };

    let source = node.dir.join("photo.jpg");
    make_file(&source, 128);

    let result =
        sharelocal::transfer::send::start_send(&node.state, phone.clone(), vec![source], None).await;
    let err = result.expect_err("dialling a browser must not be attempted");
    let message = err.message();
    assert!(
        message.contains("QR code"),
        "the error should explain why, not surface a socket errno: {message}"
    );

    let text_result =
        sharelocal::transfer::send::start_text_send(&node.state, phone, "hi".into(), None).await;
    assert!(text_result.is_err(), "text has the same constraint");
}

/// The flow that used to fail with "Can't assign requested address": the
/// desktop sending *to* a phone that joined by QR.
#[tokio::test(flavor = "multi_thread")]
async fn the_desktop_sends_files_to_a_paired_phone() {
    let node = start_node("Studio Mac", true).await;
    let (_url, pairing) = node.state.issue_pairing().await;
    let body: serde_json::Value = join(&node, &pairing, "aabbccddeeff0011").await.json().await.unwrap();
    let client_id = body["client_id"].as_str().unwrap().to_string();
    let client_token = body["client_token"].as_str().unwrap().to_string();

    let source = node.dir.join("slides.pdf");
    let expected = make_file(&source, 250_000);

    // Exactly what the desktop's "send" does for a browser peer.
    let session_id = sharelocal::transfer::send::offer_to_web(
        &node.state,
        &client_id,
        vec![source],
        None,
    )
    .await
    .expect("offering to a paired browser succeeds");

    // The phone learns the transfer token the same way the UI does.
    let status: serde_json::Value = client()
        .get(format!("{}/api/offer/{session_id}", node.base_url()))
        .header("Authorization", format!("Bearer {client_token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = status["token"].as_str().expect("an accepted pull offer carries its token");

    let response = client()
        .get(format!(
            "{}/api/transfer/{session_id}/0?token={token}",
            node.base_url()
        ))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(response.bytes().await.unwrap().as_ref(), expected.as_slice());

    let finished = await_terminal(&node, &session_id, Duration::from_secs(15)).await;
    assert_eq!(finished.status, SessionStatus::Completed);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_desktop_sends_text_to_a_paired_phone() {
    let node = start_node("Studio Mac", true).await;
    let (_url, pairing) = node.state.issue_pairing().await;
    let body: serde_json::Value = join(&node, &pairing, "1122334455667788").await.json().await.unwrap();
    let client_id = body["client_id"].as_str().unwrap().to_string();

    let snippet = "the wifi password is hunter2";
    let session_id = sharelocal::transfer::send::offer_to_web(
        &node.state,
        &client_id,
        Vec::new(),
        Some(snippet.into()),
    )
    .await
    .unwrap();

    // The snippet rides in the session itself — the phone needs no download.
    let session = node.session(&session_id).await.expect("session exists");
    assert_eq!(session.kind, sharelocal::model::PayloadKind::Text);
    assert_eq!(session.text.as_deref(), Some(snippet));
    assert_eq!(session.direction, sharelocal::model::Direction::Pull);
    // Delivered the moment it is published: it must not linger as in-progress.
    assert_eq!(session.status, SessionStatus::Completed);
}

#[tokio::test(flavor = "multi_thread")]
async fn offering_to_an_unknown_browser_is_refused() {
    let node = start_node("Studio Mac", true).await;
    let result =
        sharelocal::transfer::send::offer_to_web(&node.state, "web-nobody", Vec::new(), Some("x".into()))
            .await;
    assert!(result.is_err(), "an unpaired browser cannot be offered anything");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connected_phone_stays_online_past_the_peer_ttl() {
    use futures_util::StreamExt;
    use sharelocal::discovery::registry::PEER_TTL;
    use sharelocal::model::PeerStatus;

    let node = start_node("Studio Mac", true).await;
    let (_url, pairing) = node.state.issue_pairing().await;
    let body: serde_json::Value = join(&node, &pairing, "5566778899aabbcc").await.json().await.unwrap();
    let client_id = body["client_id"].as_str().unwrap().to_string();
    let token = body["client_token"].as_str().unwrap().to_string();

    // The phone's page holds its control connection open and only listens,
    // like the real browser client. Reading keeps answering server pings.
    let url = format!("{}?client_id={client_id}&token={token}", node.ws_url());
    let (socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let (_write, mut read) = socket.split();
    let reader = tokio::spawn(async move { while read.next().await.is_some() {} });

    tokio::time::sleep(PEER_TTL + std::time::Duration::from_secs(3)).await;
    let peer = node.state.peers.read().await.get(&client_id).expect("still listed");
    assert_eq!(peer.status, PeerStatus::Online, "an open page must not age out");
    reader.abort();
}
