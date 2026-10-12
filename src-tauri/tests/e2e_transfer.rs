//! End-to-end transfers between two live ShareLocal instances.
//!
//! These exercise the real code path: WebSocket negotiation, streamed HTTP
//! upload, path sanitisation, conflict handling and history — no mocks.

mod harness;

use harness::*;
use sharelocal::config::STREAM_CHUNK;
use sharelocal::model::{ConflictPolicy, SessionStatus, TransferError};
use sharelocal::transfer::send::start_send;
use std::time::Duration;

const T: Duration = Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread")]
async fn a_file_arrives_byte_for_byte() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    let source = sender.dir.join("movie.mp4");
    let expected = make_file(&source, 512 * 1024);

    let peer = receiver.as_peer().await;
    let session_id = start_send(&sender.state, peer, vec![source], None).await.unwrap();

    let finished = await_terminal(&sender, &session_id, T).await;
    assert_eq!(finished.status, SessionStatus::Completed, "{:?}", finished.error);
    assert_eq!(finished.progress.bytes_done, expected.len() as u64);

    let landed = receiver.downloads.join("movie.mp4");
    assert!(landed.exists(), "the file was not written to the download folder");
    assert_eq!(std::fs::read(&landed).unwrap(), expected);

    // Nothing may be left behind in the part directory.
    let parts = receiver.downloads.join(sharelocal::config::PART_DIR);
    if parts.exists() {
        assert_eq!(std::fs::read_dir(&parts).unwrap().count(), 0, "part files leaked");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn folder_structure_survives_the_hop() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    let root = sender.dir.join("Trip");
    make_file(&root.join("readme.md"), 64);
    make_file(&root.join("day1").join("a.jpg"), 4096);
    make_file(&root.join("day1").join("raw").join("b.dng"), 8192);

    let peer = receiver.as_peer().await;
    let session_id = start_send(&sender.state, peer, vec![root], None).await.unwrap();
    let finished = await_terminal(&sender, &session_id, T).await;
    assert_eq!(finished.status, SessionStatus::Completed, "{:?}", finished.error);

    assert!(receiver.downloads.join("Trip/readme.md").exists());
    assert!(receiver.downloads.join("Trip/day1/a.jpg").exists());
    assert!(
        receiver.downloads.join("Trip/day1/raw/b.dng").exists(),
        "nested folders must be recreated on the receiving side"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_file_streams_in_bounded_chunks() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    // Several times the streaming chunk, so the loop runs many iterations.
    let size = STREAM_CHUNK * 40 + 7;
    let source = sender.dir.join("big.bin");
    let expected = make_file(&source, size);

    let peer = receiver.as_peer().await;
    let session_id = start_send(&sender.state, peer, vec![source], None).await.unwrap();

    let finished = await_terminal(&sender, &session_id, T).await;
    assert_eq!(finished.status, SessionStatus::Completed, "{:?}", finished.error);
    assert_eq!(finished.progress.bytes_done, size as u64);

    // That the bytes are *streamed* rather than buffered is asserted
    // deterministically elsewhere: `streaming_body_emits_bounded_chunks_and_
    // the_whole_file` on the sending side and
    // `the_receiver_writes_each_chunk_as_it_arrives` on the receiving side.
    // Sampling progress here would race a loopback transfer that can finish
    // in a few milliseconds.

    let landed = receiver.downloads.join("big.bin");
    let got = std::fs::read(&landed).unwrap();
    assert_eq!(got.len(), expected.len());
    assert_eq!(got, expected, "a large file must arrive unchanged");
}

#[tokio::test(flavor = "multi_thread")]
async fn several_transfers_run_at_once() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;
    receiver.state.settings.write().await.max_concurrent = 8;
    sender.state.settings.write().await.max_concurrent = 8;

    let mut ids = Vec::new();
    for n in 0..4 {
        let source = sender.dir.join(format!("file{n}.bin"));
        make_file(&source, 200_000 + n * 1000);
        let peer = receiver.as_peer().await;
        ids.push(start_send(&sender.state, peer, vec![source], None).await.unwrap());
    }

    for id in &ids {
        let finished = await_terminal(&sender, id, T).await;
        assert_eq!(finished.status, SessionStatus::Completed, "{:?}", finished.error);
    }
    for n in 0..4 {
        assert!(receiver.downloads.join(format!("file{n}.bin")).exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_clash_keeps_both_copies() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;
    receiver.state.settings.write().await.conflict_policy = ConflictPolicy::KeepBoth;

    let existing = receiver.downloads.join("report.pdf");
    std::fs::write(&existing, b"the original").unwrap();

    let source = sender.dir.join("report.pdf");
    let expected = make_file(&source, 1024);

    let peer = receiver.as_peer().await;
    let id = start_send(&sender.state, peer, vec![source], None).await.unwrap();
    let finished = await_terminal(&sender, &id, T).await;
    assert_eq!(finished.status, SessionStatus::Completed, "{:?}", finished.error);

    assert_eq!(std::fs::read(&existing).unwrap(), b"the original");
    let kept = receiver.downloads.join("report (2).pdf");
    assert!(kept.exists(), "the incoming file should be saved alongside");
    assert_eq!(std::fs::read(&kept).unwrap(), expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_clash_can_replace_instead() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;
    receiver.state.settings.write().await.conflict_policy = ConflictPolicy::Replace;

    let existing = receiver.downloads.join("report.pdf");
    std::fs::write(&existing, b"the original").unwrap();

    let source = sender.dir.join("report.pdf");
    let expected = make_file(&source, 1024);

    let peer = receiver.as_peer().await;
    let id = start_send(&sender.state, peer, vec![source], None).await.unwrap();
    await_terminal(&sender, &id, T).await;

    assert_eq!(std::fs::read(&existing).unwrap(), expected);
    assert!(!receiver.downloads.join("report (2).pdf").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_receiver_must_agree_before_anything_is_written() {
    let sender = start_node("Sender", false).await;
    // Auto-accept off: the transfer waits for a human.
    let receiver = start_node("Receiver", false).await;

    let source = sender.dir.join("private.txt");
    make_file(&source, 2048);

    let peer = receiver.as_peer().await;
    let id = start_send(&sender.state, peer, vec![source], None).await.unwrap();

    // Give the offer time to arrive and confirm nothing has been written.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !receiver.downloads.join("private.txt").exists(),
        "a pending offer must not write to disk"
    );
    assert!(
        receiver.sessions().await.iter().any(|s| s.status == SessionStatus::Pending),
        "the receiver should be waiting for an answer"
    );

    receiver.accept_pending(ConflictPolicy::KeepBoth).await.expect("an offer to accept");
    let finished = await_terminal(&sender, &id, T).await;
    assert_eq!(finished.status, SessionStatus::Completed, "{:?}", finished.error);
    assert!(receiver.downloads.join("private.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_offer_writes_nothing() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", false).await;
    receiver.auto_answer(false, ConflictPolicy::KeepBoth).await;

    let source = sender.dir.join("nope.bin");
    make_file(&source, 4096);

    let peer = receiver.as_peer().await;
    let id = start_send(&sender.state, peer, vec![source], None).await.unwrap();

    let finished = await_terminal(&sender, &id, T).await;
    assert_eq!(finished.status, SessionStatus::Rejected);
    assert!(!receiver.downloads.join("nope.bin").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_stops_the_transfer_and_leaves_no_finished_file() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    let source = sender.dir.join("huge.bin");
    make_file(&source, STREAM_CHUNK * 200);

    let peer = receiver.as_peer().await;
    let id = start_send(&sender.state, peer, vec![source], None).await.unwrap();

    // Cancel once bytes are moving, so we are interrupting a live stream.
    for _ in 0..600 {
        if sender
            .session(&id)
            .await
            .map(|v| v.progress.bytes_done > 0)
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    {
        let sessions = sender.state.sessions.read().await;
        sessions.get(&id).unwrap().control.cancel();
    }
    sharelocal::transfer::conclude(&sender.state, &id, SessionStatus::Cancelled, None).await;

    let view = sender.session(&id).await.unwrap();
    assert_eq!(view.status, SessionStatus::Cancelled);
    assert!(
        !receiver.downloads.join("huge.bin").exists(),
        "a cancelled transfer must not leave a finished-looking file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn history_records_the_transfer_but_not_its_contents() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    let source = sender.dir.join("note.txt");
    make_file(&source, 32_000);

    let peer = receiver.as_peer().await;
    let id = start_send(&sender.state, peer, vec![source], None).await.unwrap();
    await_terminal(&sender, &id, T).await;

    let entries = sender.state.history.lock().await.entries().to_vec();
    let entry = entries.first().expect("the send is recorded");
    assert_eq!(entry.display_name, "note.txt");
    assert_eq!(entry.total_bytes, 32_000);
    assert_eq!(entry.status, SessionStatus::Completed);

    let json = serde_json::to_string(&entries).unwrap();
    assert!(!json.contains("content"), "history must never hold file data");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_source_file_fails_before_anything_is_offered() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    let peer = receiver.as_peer().await;
    let result = start_send(
        &sender.state,
        peer,
        vec![sender.dir.join("does-not-exist.bin")],
        None,
    )
    .await;

    assert!(matches!(result, Err(TransferError::FileMissing(_))), "{result:?}");
    assert!(receiver.sessions().await.is_empty(), "no session should be created");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_device_reports_that_it_is_offline() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    let source = sender.dir.join("x.bin");
    make_file(&source, 128);

    // Point at a port nothing is listening on.
    let mut peer = receiver.as_peer().await;
    peer.device.port = peer.device.port.wrapping_add(9_000);

    let id = start_send(&sender.state, peer, vec![source], None).await.unwrap();
    let finished = await_terminal(&sender, &id, T).await;

    assert_eq!(finished.status, SessionStatus::Failed);
    let error = finished.error.expect("a reason is reported");
    assert!(
        matches!(
            error,
            TransferError::ConnectionRefused(_) | TransferError::DeviceOffline(_)
        ),
        "unexpected error: {error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_text_snippet_is_shared_and_saved() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    let snippet = "https://example.com/a-link-i-want-on-my-phone\nsecond line";
    let peer = receiver.as_peer().await;
    let id = sharelocal::transfer::send::start_text_send(
        &sender.state,
        peer,
        snippet.to_string(),
        None,
    )
    .await
    .unwrap();

    let finished = await_terminal(&sender, &id, T).await;
    assert_eq!(finished.status, SessionStatus::Completed, "{:?}", finished.error);
    assert_eq!(finished.kind, sharelocal::model::PayloadKind::Text);

    // It lands as a plain text file the user can open later…
    let saved: Vec<_> = std::fs::read_dir(&receiver.downloads)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "txt").unwrap_or(false))
        .collect();
    assert_eq!(saved.len(), 1, "the snippet is written to the download folder");
    assert_eq!(std::fs::read_to_string(&saved[0]).unwrap(), snippet);

    // …and the receiver holds it in memory so the UI can offer "copy".
    let received = receiver
        .sessions()
        .await
        .into_iter()
        .find(|s| s.role == sharelocal::model::Role::Receiving)
        .expect("the receiver has the session");
    assert_eq!(received.text.as_deref(), Some(snippet));
    assert_eq!(received.kind, sharelocal::model::PayloadKind::Text);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_text_share_can_be_declined_and_writes_nothing() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", false).await;
    receiver.auto_answer(false, ConflictPolicy::KeepBoth).await;

    let peer = receiver.as_peer().await;
    let id = sharelocal::transfer::send::start_text_send(
        &sender.state,
        peer,
        "secret".into(),
        None,
    )
    .await
    .unwrap();

    let finished = await_terminal(&sender, &id, T).await;
    assert_eq!(finished.status, SessionStatus::Rejected);

    let txt_count = std::fs::read_dir(&receiver.downloads)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "txt").unwrap_or(false))
        .count();
    assert_eq!(txt_count, 0, "a declined snippet must not be written");
}

#[tokio::test(flavor = "multi_thread")]
async fn text_history_keeps_the_snippet_for_re_copying() {
    let sender = start_node("Sender", false).await;
    let receiver = start_node("Receiver", true).await;

    let peer = receiver.as_peer().await;
    let id = sharelocal::transfer::send::start_text_send(
        &sender.state,
        peer,
        "wifi password: hunter2".into(),
        None,
    )
    .await
    .unwrap();
    await_terminal(&sender, &id, T).await;

    let entries = sender.state.history.lock().await.entries().to_vec();
    let entry = entries.first().expect("recorded");
    assert_eq!(entry.kind, sharelocal::model::PayloadKind::Text);
    assert_eq!(entry.text.as_deref(), Some("wifi password: hunter2"));
    // The display name is a readable preview, not "shared text".
    assert_eq!(entry.display_name, "wifi password: hunter2");
}
