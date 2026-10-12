//! The outbound half of a transfer.
//!
//! Negotiation happens over the WebSocket control channel; the bytes then go
//! out as one streamed HTTP `PUT` per file. A dropped connection is retried
//! from the exact byte the receiver already has, so a 20 GB file survives a
//! Wi-Fi hiccup without starting over.

use crate::config::STREAM_CHUNK;
use crate::events::{AppEvent, NoticeLevel};
use crate::model::{Direction, Peer, Role, SessionStatus, TransferError};
use crate::server::protocol::ControlMsg;
use crate::state::AppState;
use crate::transfer::session::{Control, Session};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_tungstenite::tungstenite::Message;

/// How many times one file is retried (with resume) before giving up.
const MAX_ATTEMPTS: usize = 4;
const RETRY_BACKOFF: Duration = Duration::from_millis(700);
const OFFER_TIMEOUT: Duration = Duration::from_secs(150);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);
/// How often the sender reports progress on the control channel. This doubles
/// as the keepalive for [`crate::server::ws::IDLE_TIMEOUT`].
pub const CONTROL_HEARTBEAT: Duration = Duration::from_secs(2);

/// Queue a send to `peer`. Returns the local session id immediately; the
/// transfer itself runs on its own task.
pub async fn start_send(
    state: &Arc<AppState>,
    peer: Peer,
    inputs: Vec<PathBuf>,
    pin: Option<String>,
) -> Result<String, TransferError> {
    // A peer with no port is a browser client: it has no server to dial, and
    // attempting it produces a baffling "can't assign requested address".
    // Those transfers must go through the pull path instead.
    if peer.device.port == 0 {
        return Err(TransferError::Other(format!(
            "{} joined through the QR code, so it cannot be dialled directly — \
             offer the files for it to download instead",
            peer.device.name
        )));
    }

    let (files, sources) = crate::transfer::collect_files(&inputs)?;

    let (max_concurrent, conflict, download_dir) = {
        let s = state.settings.read().await;
        (s.max_concurrent, s.conflict_policy, s.download_dir.clone())
    };
    if state.sessions.read().await.active_count() >= max_concurrent {
        return Err(TransferError::Other(format!(
            "already running {max_concurrent} transfers — raise the limit in Settings or wait"
        )));
    }

    let mut session = Session::new(
        peer.device.clone(),
        files,
        Direction::Push,
        Role::Sending,
        conflict,
        download_dir,
    );
    session.source_paths = sources;
    session.remote_base = Some(peer.base_url());
    let id = state.sessions.write().await.insert(session);

    state.publish_session(&id).await;

    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        if let Err(err) = run_send(&task_state, &task_id, peer, pin).await {
            let status = if matches!(err, TransferError::Rejected(_)) {
                SessionStatus::Rejected
            } else if matches!(err, TransferError::Interrupted(ref m) if m.contains("cancelled")) {
                SessionStatus::Cancelled
            } else {
                SessionStatus::Failed
            };
            task_state.publish(AppEvent::notice(
                NoticeLevel::Error,
                error_code(&err),
                err.message().to_string(),
            ));
            crate::transfer::conclude(&task_state, &task_id, status, Some(err)).await;
        }
    });

    Ok(id)
}

/// Publish a transfer for a paired browser to pull.
///
/// A browser that joined by QR is a client: it has no server to dial, so the
/// direction is reversed — we hold the files and it fetches them. Used for
/// both files and text, so the command layer never has to know the difference.
pub async fn offer_to_web(
    state: &Arc<AppState>,
    client_id: &str,
    inputs: Vec<PathBuf>,
    text: Option<String>,
) -> Result<String, TransferError> {
    let client = {
        let clients = state.web_clients.read().await;
        clients.get(client_id).cloned().ok_or_else(|| {
            TransferError::DeviceOffline("that browser is no longer connected".into())
        })?
    };

    let (conflict, download_dir) = {
        let s = state.settings.read().await;
        (s.conflict_policy, s.download_dir.clone())
    };

    let (files, sources, kind) = match &text {
        Some(body) => {
            if body.len() > crate::model::MAX_TEXT_BYTES {
                return Err(TransferError::Other(format!(
                    "shared text is limited to {} KB",
                    crate::model::MAX_TEXT_BYTES / 1024
                )));
            }
            (
                vec![crate::model::FileMeta {
                    rel_path: "shared text".into(),
                    size: body.len() as u64,
                    mime: Some("text/plain".into()),
                    modified_ms: None,
                }],
                Vec::new(),
                crate::model::PayloadKind::Text,
            )
        }
        None => {
            let (files, sources) = crate::transfer::collect_files(&inputs)?;
            (files, sources, crate::model::PayloadKind::Files)
        }
    };

    let mut session = Session::new(
        client.device.clone(),
        files,
        crate::model::Direction::Pull,
        Role::Sending,
        conflict,
        download_dir,
    );
    session.source_paths = sources;
    session.kind = kind;
    session.text = text;
    // Pre-accepted on this side; the browser still has to tap to receive.
    session.set_status(SessionStatus::Accepted);
    let view = session.view();
    let id = state.sessions.write().await.insert(session);

    state.publish(AppEvent::IncomingOffer { session: view });
    state.publish_session(&id).await;
    // Text rides inside the offer, so publishing it *is* delivering it. Left
    // `Accepted`, it would sit under "In progress" until the session expired.
    if kind == crate::model::PayloadKind::Text {
        crate::transfer::conclude(state, &id, SessionStatus::Completed, None).await;
    }
    Ok(id)
}

/// Queue a text share. Mirrors [`start_send`] but there are no files to walk
/// and no bytes to stream — the snippet rides along with the offer.
pub async fn start_text_send(
    state: &Arc<AppState>,
    peer: Peer,
    text: String,
    pin: Option<String>,
) -> Result<String, TransferError> {
    if peer.device.port == 0 {
        return Err(TransferError::Other(format!(
            "{} joined through the QR code and cannot be dialled directly",
            peer.device.name
        )));
    }

    let (conflict, download_dir) = {
        let s = state.settings.read().await;
        (s.conflict_policy, s.download_dir.clone())
    };

    // One nominal "file" so progress, history and the peer's UI all have
    // something concrete to describe.
    let files = vec![crate::model::FileMeta {
        rel_path: "shared text".into(),
        size: text.len() as u64,
        mime: Some("text/plain".into()),
        modified_ms: None,
    }];

    let mut session = Session::new(
        peer.device.clone(),
        files,
        Direction::Push,
        Role::Sending,
        conflict,
        download_dir,
    );
    session.kind = crate::model::PayloadKind::Text;
    session.text = Some(text);
    session.remote_base = Some(peer.base_url());
    let id = state.sessions.write().await.insert(session);
    state.publish_session(&id).await;

    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        if let Err(err) = run_send(&task_state, &task_id, peer, pin).await {
            let status = if matches!(err, TransferError::Rejected(_)) {
                SessionStatus::Rejected
            } else {
                SessionStatus::Failed
            };
            task_state.publish(AppEvent::notice(
                NoticeLevel::Error,
                error_code(&err),
                err.message().to_string(),
            ));
            crate::transfer::conclude(&task_state, &task_id, status, Some(err)).await;
        }
    });

    Ok(id)
}

fn error_code(err: &TransferError) -> &'static str {
    use TransferError::*;
    match err {
        DeviceOffline(_) => "device-offline",
        ConnectionRefused(_) => "connection-refused",
        NetworkChanged(_) => "network-changed",
        Interrupted(_) => "transfer-interrupted",
        NotEnoughSpace(_) => "no-space",
        PermissionDenied(_) => "permission-denied",
        FileMissing(_) => "file-missing",
        DuplicateName(_) => "duplicate-name",
        InvalidToken(_) => "invalid-token",
        InvalidPath(_) => "invalid-path",
        PinRejected(_) => "pin-rejected",
        Rejected(_) => "rejected",
        ClientIsolation(_) => "client-isolation",
        Other(_) => "transfer-failed",
    }
}

async fn run_send(
    state: &Arc<AppState>,
    session_id: &str,
    peer: Peer,
    pin: Option<String>,
) -> Result<(), TransferError> {
    let (files, total_bytes, control, conflict, sources, kind, text) = {
        let sessions = state.sessions.read().await;
        let s = sessions
            .get(session_id)
            .ok_or_else(|| TransferError::Other("session vanished".into()))?;
        (
            s.files.clone(),
            s.total_bytes,
            s.control.clone(),
            s.conflict,
            s.source_paths.clone(),
            s.kind,
            s.text.clone(),
        )
    };

    let ws = crate::server::tls::connect_ws(&peer.ws_url())
        .await
        .map_err(|e| connect_error(&peer, e))?;
    let (sink, mut stream) = ws.split();
    let sink: ControlSink = Arc::new(Mutex::new(sink));

    let me = state.device_info().await;
    send_control(&sink, ControlMsg::Hello { device: me })
        .await
        .map_err(|e| TransferError::ConnectionRefused(e.to_string()))?;
    send_control(
        &sink,
        ControlMsg::Offer {
            files: files.clone(),
            total_bytes,
            pin,
            direction: Direction::Push,
            conflict,
            kind,
            text: text.clone(),
        },
    )
    .await
    .map_err(|e| TransferError::ConnectionRefused(e.to_string()))?;

    // Wait for the receiver (or its user) to answer.
    let (remote_id, token) = wait_for_acceptance(state, session_id, &mut stream, &control).await?;

    {
        let mut sessions = state.sessions.write().await;
        if let Some(s) = sessions.get_mut(session_id) {
            s.remote_id = Some(remote_id.clone());
            s.remote_token = Some(token.clone());
            s.set_status(SessionStatus::Transferring);
        }
    }
    state.publish_session(session_id).await;

    // From here on the receiver may cancel at any moment; watch for it on a
    // separate task so it takes effect mid-upload.
    let watch_control = control.clone();
    let watcher = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            if let Message::Text(text) = msg {
                match ControlMsg::parse(&text) {
                    Some(ControlMsg::Cancel { .. }) | Some(ControlMsg::Error { .. }) => {
                        watch_control.cancel();
                        return;
                    }
                    _ => {}
                }
            }
        }
        // The control channel closing mid-transfer means the peer went away.
        watch_control.cancel();
    });

    if kind == crate::model::PayloadKind::Text {
        // The snippet travelled inside the offer, so acceptance *is* delivery.
        let _ = send_control(&sink, ControlMsg::Complete { session_id: remote_id }).await;
        let _ = sink.lock().await.close().await;
        {
            let mut sessions = state.sessions.write().await;
            if let Some(s) = sessions.get_mut(session_id) {
                s.files_done = s.files.len();
                s.bytes_before_current = s.total_bytes;
            }
        }
        crate::transfer::conclude(state, session_id, SessionStatus::Completed, None).await;
        return Ok(());
    }

    // Report progress on the control channel while the bytes flow over HTTP.
    // The receiving UI uses these, and they double as a keepalive: a large
    // transfer otherwise produces no control traffic at all, and the receiver
    // drops connections that go quiet.
    let heartbeat = {
        let state = state.clone();
        let sink = sink.clone();
        let local_id = session_id.to_string();
        let remote = remote_id.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CONTROL_HEARTBEAT).await;
                let progress = {
                    let sessions = state.sessions.read().await;
                    match sessions.get(&local_id) {
                        Some(s) if !s.status.is_terminal() => s.progress(),
                        _ => return,
                    }
                };
                let sent = send_control(
                    &sink,
                    ControlMsg::Progress {
                        session_id: remote.clone(),
                        bytes_done: progress.bytes_done,
                        file_index: progress.file_index,
                        speed_bps: progress.speed_bps,
                    },
                )
                .await;
                if sent.is_err() {
                    return;
                }
            }
        })
    };

    let base = peer.base_url();
    let mut result: Result<(), TransferError> = Ok(());

    for (index, file) in files.iter().enumerate() {
        if control.is_cancelled() {
            result = Err(TransferError::Interrupted("transfer cancelled".into()));
            break;
        }
        {
            let mut sessions = state.sessions.write().await;
            if let Some(s) = sessions.get_mut(session_id) {
                s.file_index = index;
                s.file_bytes_done = 0;
                s.bytes_before_current = files.iter().take(index).map(|f| f.size).sum();
            }
        }
        let source = sources
            .get(index)
            .cloned()
            .ok_or_else(|| TransferError::FileMissing(file.rel_path.clone()))?;

        match send_one_file(
            state, session_id, &base, &remote_id, &token, index, &source, file.size, &control,
        )
        .await
        {
            Ok(()) => {
                let mut sessions = state.sessions.write().await;
                if let Some(s) = sessions.get_mut(session_id) {
                    s.complete_current_file(None);
                }
            }
            Err(err) => {
                result = Err(err);
                break;
            }
        }
        state.publish_session(session_id).await;
    }

    watcher.abort();
    heartbeat.abort();

    match result {
        Ok(()) => {
            let _ = send_control(&sink, ControlMsg::Complete { session_id: remote_id }).await;
            let _ = sink.lock().await.close().await;
            crate::transfer::conclude(state, session_id, SessionStatus::Completed, None).await;
            Ok(())
        }
        Err(err) => {
            let _ = send_control(&sink, ControlMsg::Cancel { session_id: remote_id }).await;
            let _ = sink.lock().await.close().await;
            Err(err)
        }
    }
}

/// The write half of a control channel, shared between the transfer loop and
/// the heartbeat task.
type ControlSink = Arc<
    Mutex<
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
    >,
>;

async fn send_control(
    sink: &ControlSink,
    msg: ControlMsg,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    sink.lock()
        .await
        .send(Message::Text(msg.to_text().into()))
        .await
}

fn connect_error(peer: &Peer, err: impl std::fmt::Display) -> TransferError {
    let text = err.to_string();
    if !peer.same_subnet {
        return TransferError::ClientIsolation(format!(
            "{} is on a different subnet and did not answer. Guest or \"client isolation\" \
             Wi-Fi settings block device-to-device traffic.",
            peer.device.name
        ));
    }
    if text.contains("refused") {
        TransferError::ConnectionRefused(format!(
            "{} refused the connection — ShareLocal may be closed, or a firewall is blocking port {}.",
            peer.device.name, peer.device.port
        ))
    } else {
        TransferError::DeviceOffline(format!("{} did not answer ({text})", peer.device.name))
    }
}

/// Block until the peer accepts, rejects, or the offer times out.
async fn wait_for_acceptance<S>(
    state: &Arc<AppState>,
    session_id: &str,
    stream: &mut S,
    control: &Arc<Control>,
) -> Result<(String, String), TransferError>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = tokio::time::Instant::now() + OFFER_TIMEOUT;

    loop {
        if control.is_cancelled() {
            return Err(TransferError::Interrupted("transfer cancelled".into()));
        }
        let next = tokio::time::timeout_at(deadline, stream.next()).await;
        let msg = match next {
            Err(_) => {
                return Err(TransferError::Interrupted(
                    "the other device did not answer the transfer request".into(),
                ))
            }
            Ok(None) => {
                return Err(TransferError::DeviceOffline(
                    "the other device closed the connection".into(),
                ))
            }
            Ok(Some(Err(err))) => return Err(TransferError::Interrupted(err.to_string())),
            Ok(Some(Ok(msg))) => msg,
        };

        let Message::Text(text) = msg else { continue };
        match ControlMsg::parse(&text) {
            Some(ControlMsg::OfferStatus {
                session_id: remote,
                status,
                token,
                reason,
            }) => {
                match status {
                    SessionStatus::Pending => {
                        // Surface "waiting for the other side" in the UI.
                        let mut sessions = state.sessions.write().await;
                        if let Some(s) = sessions.get_mut(session_id) {
                            s.remote_id = Some(remote.clone());
                            s.set_status(SessionStatus::Pending);
                        }
                        drop(sessions);
                        state.publish_session(session_id).await;
                    }
                    SessionStatus::Accepted => {
                        let token = token.ok_or_else(|| {
                            TransferError::InvalidToken("receiver accepted without a token".into())
                        })?;
                        return Ok((remote, token));
                    }
                    SessionStatus::Rejected => {
                        return Err(TransferError::Rejected(
                            reason.unwrap_or_else(|| "the other device declined".into()),
                        ))
                    }
                    _ => {}
                }
            }
            Some(ControlMsg::Error { code, message, .. }) => {
                return Err(if code == "pin-rejected" {
                    TransferError::PinRejected(message)
                } else {
                    TransferError::Other(message)
                })
            }
            Some(ControlMsg::Cancel { .. }) => {
                return Err(TransferError::Interrupted("transfer cancelled".into()))
            }
            _ => {}
        }
    }
}

/// Upload one file, resuming from whatever the receiver already holds.
#[allow(clippy::too_many_arguments)]
async fn send_one_file(
    state: &Arc<AppState>,
    session_id: &str,
    base: &str,
    remote_id: &str,
    token: &str,
    index: usize,
    source: &PathBuf,
    size: u64,
    control: &Arc<Control>,
) -> Result<(), TransferError> {
    let mut last_err: Option<TransferError> = None;

    for attempt in 0..MAX_ATTEMPTS {
        if control.is_cancelled() {
            return Err(TransferError::Interrupted("transfer cancelled".into()));
        }
        if attempt > 0 {
            tokio::time::sleep(RETRY_BACKOFF * attempt as u32).await;
        }

        // Ask how much the receiver already has, so a retry resumes.
        let offset = match query_offset(state, base, remote_id, token, index).await {
            Ok(offset) => offset.min(size),
            Err(err) => {
                last_err = Some(err);
                continue;
            }
        };
        if offset >= size && size > 0 {
            return Ok(());
        }

        {
            let mut sessions = state.sessions.write().await;
            if let Some(s) = sessions.get_mut(session_id) {
                s.file_bytes_done = offset;
            }
        }

        let mut file = tokio::fs::File::open(source)
            .await
            .map_err(|e| crate::transfer::io_error(&e, &format!("opening {}", source.display())))?;
        // A file that shrank since it was offered cannot be sent correctly.
        let current_len = file
            .metadata()
            .await
            .map(|m| m.len())
            .map_err(|e| crate::transfer::io_error(&e, "reading file size"))?;
        if current_len != size {
            return Err(TransferError::FileMissing(format!(
                "{} changed on disk while it was being sent",
                source.display()
            )));
        }
        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|e| crate::transfer::io_error(&e, "seeking the file"))?;
        }

        let body = streaming_body(
            file,
            size - offset,
            state.clone(),
            session_id.to_string(),
            control.clone(),
            offset,
        );

        let url = format!("{base}/api/transfer/{remote_id}/{index}");
        let response = state
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("X-Offset", offset.to_string())
            .header("Content-Length", (size - offset).to_string())
            .header("Content-Type", "application/octet-stream")
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await;

        match response {
            Ok(res) if res.status().is_success() => return Ok(()),
            Ok(res) => {
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                let err = http_error(status, &text);
                // Authorisation and validation failures will not fix themselves.
                if matches!(
                    err,
                    TransferError::InvalidToken(_)
                        | TransferError::InvalidPath(_)
                        | TransferError::PermissionDenied(_)
                        | TransferError::NotEnoughSpace(_)
                        | TransferError::DuplicateName(_)
                ) {
                    return Err(err);
                }
                last_err = Some(err);
            }
            Err(err) => {
                if control.is_cancelled() {
                    return Err(TransferError::Interrupted("transfer cancelled".into()));
                }
                last_err = Some(TransferError::Interrupted(format!(
                    "connection lost while sending ({err})"
                )));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| TransferError::Interrupted("transfer failed".into())))
}

fn http_error(status: reqwest::StatusCode, body: &str) -> TransferError {
    // The server replies with our own `TransferError` JSON where it can.
    if let Ok(parsed) = serde_json::from_str::<TransferError>(body) {
        return parsed;
    }
    match status.as_u16() {
        401 | 403 => {
            TransferError::InvalidToken(format!("rejected by the other device ({status})"))
        }
        404 => TransferError::InvalidToken("the transfer session no longer exists".into()),
        409 => TransferError::DuplicateName(body.to_string()),
        413 => TransferError::Other("the other device refused the file size".into()),
        507 => TransferError::NotEnoughSpace("the other device is out of disk space".into()),
        _ => TransferError::Other(format!("the other device replied {status}")),
    }
}

async fn query_offset(
    state: &Arc<AppState>,
    base: &str,
    remote_id: &str,
    token: &str,
    index: usize,
) -> Result<u64, TransferError> {
    #[derive(serde::Deserialize)]
    struct StatusBody {
        received: u64,
    }
    let url = format!("{base}/api/transfer/{remote_id}/{index}/status");
    let res = state
        .http
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| {
            TransferError::Interrupted(format!("could not reach the other device ({e})"))
        })?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(http_error(status, &text));
    }
    let body: StatusBody = res
        .json()
        .await
        .map_err(|e| TransferError::Other(format!("unexpected reply ({e})")))?;
    Ok(body.received)
}

/// The heart of the performance requirement: a `Stream` that reads the file one
/// bounded chunk at a time. Memory use is `STREAM_CHUNK`, whatever the file size.
fn streaming_body(
    file: tokio::fs::File,
    remaining: u64,
    state: Arc<AppState>,
    session_id: String,
    control: Arc<Control>,
    start_offset: u64,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    struct Source {
        file: tokio::fs::File,
        remaining: u64,
        state: Arc<AppState>,
        session_id: String,
        control: Arc<Control>,
        sent_for_file: u64,
        last_publish: Instant,
        buf: Vec<u8>,
    }

    let source = Source {
        file,
        remaining,
        state,
        session_id,
        control,
        sent_for_file: start_offset,
        last_publish: Instant::now(),
        buf: vec![0u8; STREAM_CHUNK],
    };

    futures_util::stream::unfold(Some(source), |maybe| async move {
        let mut src = maybe?;
        if src.remaining == 0 {
            return None;
        }
        if !src.control.wait_if_paused().await {
            return Some((Err(std::io::Error::other("transfer cancelled")), None));
        }

        let want = src.remaining.min(STREAM_CHUNK as u64) as usize;
        match src.file.read(&mut src.buf[..want]).await {
            Ok(0) => None,
            Ok(n) => {
                src.remaining -= n as u64;
                src.sent_for_file += n as u64;

                {
                    let mut sessions = src.state.sessions.write().await;
                    if let Some(s) = sessions.get_mut(&src.session_id) {
                        s.meter.record(n as u64);
                        s.file_bytes_done = src.sent_for_file;
                    }
                }
                if src.last_publish.elapsed() >= PROGRESS_INTERVAL {
                    src.last_publish = Instant::now();
                    src.state.publish_session(&src.session_id).await;
                }

                let chunk = Bytes::copy_from_slice(&src.buf[..n]);
                Some((Ok(chunk), Some(src)))
            }
            Err(err) => Some((Err(err), None)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heartbeat_keeps_the_control_channel_open() {
        // A transfer of any length produces no control traffic of its own, so
        // the receiver's idle timeout must leave room for several missed
        // heartbeats before it gives up on the connection.
        assert!(
            CONTROL_HEARTBEAT * 5 < crate::server::ws::IDLE_TIMEOUT,
            "the heartbeat ({CONTROL_HEARTBEAT:?}) must be well inside the receiver's \
             idle timeout ({:?}), or long transfers get cut off",
            crate::server::ws::IDLE_TIMEOUT
        );
    }

    #[test]
    fn http_status_codes_map_to_useful_errors() {
        assert!(matches!(
            http_error(reqwest::StatusCode::UNAUTHORIZED, ""),
            TransferError::InvalidToken(_)
        ));
        assert!(matches!(
            http_error(reqwest::StatusCode::NOT_FOUND, ""),
            TransferError::InvalidToken(_)
        ));
        assert!(matches!(
            http_error(reqwest::StatusCode::CONFLICT, "exists"),
            TransferError::DuplicateName(_)
        ));
        assert!(matches!(
            http_error(reqwest::StatusCode::INSUFFICIENT_STORAGE, ""),
            TransferError::NotEnoughSpace(_)
        ));
    }

    #[test]
    fn structured_error_bodies_are_preserved() {
        let body = serde_json::to_string(&TransferError::InvalidPath("bad".into())).unwrap();
        assert!(matches!(
            http_error(reqwest::StatusCode::BAD_REQUEST, &body),
            TransferError::InvalidPath(_)
        ));
    }

    #[test]
    fn error_codes_are_stable_for_the_ui() {
        assert_eq!(
            error_code(&TransferError::DeviceOffline("".into())),
            "device-offline"
        );
        assert_eq!(
            error_code(&TransferError::PinRejected("".into())),
            "pin-rejected"
        );
        assert_eq!(
            error_code(&TransferError::ClientIsolation("".into())),
            "client-isolation"
        );
    }

    #[tokio::test]
    async fn streaming_body_emits_bounded_chunks_and_the_whole_file() {
        use futures_util::StreamExt;

        let dir = std::env::temp_dir().join(format!("sl-stream-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.bin");

        // Comfortably larger than one chunk so we exercise the loop.
        let size = STREAM_CHUNK * 3 + 1234;
        let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();

        let state = AppState::new(dir.clone());
        let file = tokio::fs::File::open(&path).await.unwrap();
        let control = Arc::new(Control::default());
        let stream = streaming_body(file, size as u64, state, "no-session".into(), control, 0);
        futures_util::pin_mut!(stream);

        let mut collected = Vec::new();
        let mut chunks = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            assert!(chunk.len() <= STREAM_CHUNK, "chunks stay bounded");
            collected.extend_from_slice(&chunk);
            chunks += 1;
        }
        assert_eq!(collected, data, "the whole file is delivered byte for byte");
        assert!(chunks >= 4, "a multi-chunk file is streamed, not slurped");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn streaming_body_stops_when_cancelled() {
        use futures_util::StreamExt;

        let dir = std::env::temp_dir().join(format!("sl-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.bin");
        let size = STREAM_CHUNK * 4;
        std::fs::write(&path, vec![7u8; size]).unwrap();

        let state = AppState::new(dir.clone());
        let file = tokio::fs::File::open(&path).await.unwrap();
        let control = Arc::new(Control::default());
        // Pause, then cancel: the stream must abort rather than hang.
        control.set_paused(true);
        control.cancel();

        let stream = streaming_body(file, size as u64, state, "no-session".into(), control, 0);
        futures_util::pin_mut!(stream);
        let first = stream.next().await.expect("one item");
        assert!(first.is_err(), "a cancelled transfer surfaces an error");

        std::fs::remove_dir_all(&dir).ok();
    }
}
