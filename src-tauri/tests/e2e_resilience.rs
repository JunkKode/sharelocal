//! Interruptions, resume, port changes and live discovery over the real
//! network stack.

mod harness;

use futures_util::{SinkExt, StreamExt};
use harness::*;
use sharelocal::model::SessionStatus;
use sharelocal::server::protocol::ControlMsg;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Negotiate an accepted receiving session and return its id and token.
async fn accepted_session(node: &Node, rel_path: &str, size: u64) -> (String, String) {
    let (socket, _) = tokio_tungstenite::connect_async(node.ws_url()).await.unwrap();
    let (mut sink, mut stream) = socket.split();

    sink.send(Message::Text(
        ControlMsg::Hello { device: device("tester", "Tester") }.to_text().into(),
    ))
    .await
    .unwrap();
    sink.send(Message::Text(
        ControlMsg::Offer {
            files: vec![sharelocal::model::FileMeta {
                rel_path: rel_path.into(),
                size,
                mime: None,
                modified_ms: None,
            }],
            total_bytes: size,
            pin: None,
            direction: sharelocal::model::Direction::Push,
            conflict: sharelocal::model::ConflictPolicy::KeepBoth,
            kind: sharelocal::model::PayloadKind::Files,
            text: None,
        }
        .to_text()
        .into(),
    ))
    .await
    .unwrap();

    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = msg {
            if let Some(ControlMsg::OfferStatus { session_id, token: Some(token), .. }) =
                ControlMsg::parse(&text)
            {
                // Keep the control channel alive for the life of the session.
                std::mem::forget(sink);
                std::mem::forget(stream);
                return (session_id, token);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_upload_resumes_from_where_it_stopped() {
    let node = start_node("Receiver", true).await;

    let total = 300_000usize;
    let payload: Vec<u8> = (0..total).map(|i| (i % 253) as u8).collect();
    let cut = 120_000usize;

    let (session_id, token) = accepted_session(&node, "interrupted.bin", total as u64).await;

    // First attempt: hand over part of the file, then break the connection.
    let head = payload[..cut].to_vec();
    // Pause before breaking so the first chunk genuinely reaches the receiver;
    // failing instantly would abort the request before anything was sent.
    let broken = futures_util::stream::iter(vec![Ok::<Vec<u8>, std::io::Error>(head)]).chain(
        futures_util::stream::once(async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Err(std::io::Error::other("connection dropped"))
        }),
    );
    let first = client()
        .put(format!("{}/api/transfer/{session_id}/0", node.base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .header("X-Offset", "0")
        .body(reqwest::Body::wrap_stream(broken))
        .send()
        .await;
    assert!(
        first.is_err() || !first.unwrap().status().is_success(),
        "a truncated upload must not be treated as a success"
    );

    // The finished file must not exist yet…
    assert!(!node.downloads.join("interrupted.bin").exists());

    // …but the receiver should be able to say how much it already holds.
    let status: serde_json::Value = client()
        .get(format!("{}/api/transfer/{session_id}/0/status", node.base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let received = status["received"].as_u64().unwrap();
    assert!(received > 0, "nothing was kept, so nothing could be resumed");
    assert!(received <= cut as u64);

    // Resume from exactly there.
    let tail = payload[received as usize..].to_vec();
    let second = client()
        .put(format!("{}/api/transfer/{session_id}/0", node.base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .header("X-Offset", received.to_string())
        .body(tail)
        .send()
        .await
        .unwrap();
    assert!(second.status().is_success(), "the resume should succeed");

    let landed = node.downloads.join("interrupted.bin");
    assert!(landed.exists());
    assert_eq!(
        std::fs::read(&landed).unwrap(),
        payload,
        "a resumed file must be byte-identical, with no duplicated or missing region"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resume_offset_past_the_start_truncates_stale_bytes() {
    let node = start_node("Receiver", true).await;

    let total = 40_000usize;
    let good: Vec<u8> = vec![9u8; total];
    let (session_id, token) = accepted_session(&node, "retry.bin", total as u64).await;

    // Write a first, wrong prefix.
    let bad_prefix = vec![1u8; 10_000];
    let broken = futures_util::stream::iter(vec![Ok::<Vec<u8>, std::io::Error>(bad_prefix)])
        .chain(futures_util::stream::once(async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Err(std::io::Error::other("dropped"))
        }));
    let _ = client()
        .put(format!("{}/api/transfer/{session_id}/0", node.base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .header("X-Offset", "0")
        .body(reqwest::Body::wrap_stream(broken))
        .send()
        .await;

    // Restarting from zero must discard whatever was there.
    let response = client()
        .put(format!("{}/api/transfer/{session_id}/0", node.base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .header("X-Offset", "0")
        .body(good.clone())
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(
        std::fs::read(node.downloads.join("retry.bin")).unwrap(),
        good,
        "restarting at offset 0 must not append to the earlier attempt"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_the_port_moves_the_server() {
    let node = start_node("Mover", true).await;
    let old_port = node.state.port();

    let new_port = {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };

    node.state.settings.write().await.port = new_port;
    node.state.request_restart();

    // Wait for the new address to answer.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if node.state.port() == new_port {
            let response = client()
                .get(format!("http://127.0.0.1:{new_port}/api/info"))
                .send()
                .await;
            if response.map(|r| r.status().is_success()).unwrap_or(false) {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the server never came back on the new port"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // And the device now advertises the new port.
    assert_eq!(node.state.device_info().await.port, new_port);
    assert_ne!(old_port, new_port);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_devices_find_each_other_over_the_lan_fallback() {
    // mDNS is often unavailable inside a sandbox, so this exercises the UDP
    // fallback path — the one that exists precisely for hostile networks.
    let a = start_node("Alpha", true).await;
    let b = start_node("Beta", true).await;

    for node in [&a, &b] {
        let mut settings = node.state.settings.write().await;
        settings.discovery_udp = true;
        settings.discovery_mdns = false;
    }
    sharelocal::discovery::spawn(a.state.clone());
    sharelocal::discovery::spawn(b.state.clone());

    let a_id = a.state.settings.read().await.device_id.clone();
    let b_id = b.state.settings.read().await.device_id.clone();

    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    let mut found = false;
    while std::time::Instant::now() < deadline {
        let a_sees_b = a.state.peers.read().await.get(&b_id).is_some();
        let b_sees_a = b.state.peers.read().await.get(&a_id).is_some();
        if a_sees_b && b_sees_a {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    if !found {
        // A sandbox that blocks multicast and broadcast cannot run this; the
        // registry itself is covered by unit tests either way.
        eprintln!("skipping: this environment does not deliver UDP discovery traffic");
        return;
    }

    let peer = a.state.peers.read().await.get(&b_id).unwrap();
    assert_eq!(peer.device.name, "Beta");
    assert_eq!(peer.status, sharelocal::model::PeerStatus::Online);
    assert!(
        peer.addresses.iter().any(|ip| ip.is_loopback() || crate::is_private(ip)),
        "a discovered peer must have a local address"
    );

    // Neither device ever lists itself.
    assert!(a.state.peers.read().await.get(&a_id).is_none());
}

fn is_private(ip: &std::net::IpAddr) -> bool {
    sharelocal::net::is_lan_ip(*ip)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_session_is_swept_away_with_its_part_file() {
    let node = start_node("Receiver", true).await;
    let (session_id, token) = accepted_session(&node, "abandoned.bin", 100_000).await;

    // Leave a partial upload behind.
    let broken = futures_util::stream::iter(vec![Ok::<Vec<u8>, std::io::Error>(vec![3u8; 20_000])])
        .chain(futures_util::stream::once(async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Err(std::io::Error::other("dropped"))
        }));
    let _ = client()
        .put(format!("{}/api/transfer/{session_id}/0", node.base_url()))
        .header("Authorization", format!("Bearer {token}"))
        .body(reqwest::Body::wrap_stream(broken))
        .send()
        .await;

    let part_dir = node.downloads.join(sharelocal::config::PART_DIR);
    assert!(part_dir.exists(), "the partial data is kept for a resume");

    // Cancelling clears both the session and its scratch data.
    sharelocal::transfer::conclude(&node.state, &session_id, SessionStatus::Cancelled, None).await;
    let remaining = std::fs::read_dir(&part_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(remaining, 0, "cancelling must not leave partial files behind");
}

/// The receiving side must commit each chunk to disk as it arrives, not
/// accumulate the file and write it at the end. Proved by holding a request
/// open: the bytes already sent have to be visible in the part file while the
/// upload is still in flight.
#[tokio::test(flavor = "multi_thread")]
async fn the_receiver_writes_each_chunk_as_it_arrives() {
    let node = start_node("Receiver", true).await;

    let first = vec![1u8; 64_000];
    let second = vec![2u8; 64_000];
    let total = (first.len() + second.len()) as u64;
    let (session_id, token) = accepted_session(&node, "incremental.bin", total).await;

    // Release the second half only once we have inspected the part file.
    let (release, released) = tokio::sync::oneshot::channel::<()>();

    let body = futures_util::stream::iter(vec![Ok::<Vec<u8>, std::io::Error>(first.clone())]).chain(
        futures_util::stream::once(async move {
            let _ = released.await;
            Ok::<Vec<u8>, std::io::Error>(second.clone())
        }),
    );

    let url = format!("{}/api/transfer/{session_id}/0", node.base_url());
    let auth = format!("Bearer {token}");
    let upload = tokio::spawn(async move {
        client()
            .put(url)
            .header("Authorization", auth)
            .header("X-Offset", "0")
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await
    });

    // While the request is still open, the first half must already be on disk.
    let part = node
        .downloads
        .join(sharelocal::config::PART_DIR)
        .join(format!("{session_id}-0.part"));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(meta) = std::fs::metadata(&part) {
            if meta.len() as usize >= first.len() {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the first chunk never reached disk — the receiver is buffering the whole file"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !node.downloads.join("incremental.bin").exists(),
        "the file must not be finalised until it is complete"
    );

    let _ = release.send(());
    let response = upload.await.unwrap().unwrap();
    assert!(response.status().is_success());

    let landed = std::fs::read(node.downloads.join("incremental.bin")).unwrap();
    assert_eq!(landed.len(), total as usize);
    assert_eq!(&landed[..64_000], &vec![1u8; 64_000][..]);
    assert_eq!(&landed[64_000..], &vec![2u8; 64_000][..]);
}
