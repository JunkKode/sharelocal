//! The WebSocket control channel.
//!
//! Peers negotiate transfers here and browser clients receive live updates.
//! Only small JSON frames ever cross this connection — file bytes go over HTTP.

use super::protocol::ControlMsg;
use crate::events::{Audience, NoticeLevel};
use crate::model::{
    ConflictPolicy, Direction, PayloadKind, Role, SessionStatus, TransferError, MAX_TEXT_BYTES,
};
use crate::state::AppState;
use crate::transfer::session::Session;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// A control connection that goes silent for this long is dropped. It must
/// stay comfortably above the sender's heartbeat interval, or a long transfer
/// would be torn down mid-flight.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// How often the server pings a connection. Browsers answer a WebSocket ping
/// on their own, so this also keeps a phone that only listens from tripping
/// [`IDLE_TIMEOUT`].
pub const PING_EVERY: Duration = Duration::from_secs(15);
/// How often a connected browser's presence is refreshed in the peer list —
/// well inside `PEER_TTL`, so it never flickers to "stale".
pub const PRESENCE_EVERY: Duration = Duration::from_secs(4);
/// A browser that has said nothing (not even a pong) for this long is
/// treated as gone, even if the socket has not noticed yet.
pub const PRESENCE_GRACE: Duration = Duration::from_secs(40);

#[derive(Debug, Deserialize, Default)]
pub struct WsQuery {
    /// Present when a browser client connects; identifies which paired client
    /// this is so it can be shown its own sessions.
    pub client_id: Option<String>,
    pub token: Option<String>,
}

pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<WsQuery>,
) -> Response {
    ws.max_message_size(super::protocol::MAX_CONTROL_FRAME)
        .on_upgrade(move |socket| handle(socket, state, addr, query))
}

async fn handle(socket: WebSocket, state: Arc<AppState>, addr: SocketAddr, query: WsQuery) {
    // A paired browser client sees events for its own sessions; an unpaired
    // LAN peer only ever sees replies to what it asks for.
    let web_client = match (&query.client_id, &query.token) {
        (Some(id), Some(token)) => state.authenticate_web(id, token).await,
        _ => None,
    };

    let (sink, mut stream) = socket.split();
    let sink = Arc::new(Mutex::new(sink));
    // Sessions this connection is party to, used to filter the event feed.
    let owned: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    let forwarder = spawn_event_forwarder(
        state.clone(),
        sink.clone(),
        owned.clone(),
        web_client.as_ref().map(|c| c.id.clone()),
    );

    // When the connection last showed signs of life, in ms since `opened`.
    let opened = std::time::Instant::now();
    let last_heard = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let heartbeat = spawn_heartbeat(
        state.clone(),
        sink.clone(),
        web_client.as_ref().map(|c| c.id.clone()),
        opened,
        last_heard.clone(),
    );

    loop {
        let next = tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await;
        if matches!(next, Ok(Some(Ok(_)))) {
            last_heard.store(
                opened.elapsed().as_millis() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        let msg = match next {
            Err(_) => break,
            Ok(None) => break,
            Ok(Some(Err(_))) => break,
            Ok(Some(Ok(msg))) => msg,
        };

        let text = match msg {
            Message::Text(text) => text.to_string(),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => break,
            // Binary frames are not part of the control protocol; a peer trying
            // to push file data down this channel is refused.
            Message::Binary(_) => {
                let _ = send(
                    &sink,
                    ControlMsg::Error {
                        session_id: None,
                        code: "protocol".into(),
                        message: "binary frames are not accepted on the control channel".into(),
                    },
                )
                .await;
                continue;
            }
        };

        let Some(parsed) = ControlMsg::parse(&text) else {
            let _ = send(
                &sink,
                ControlMsg::Error {
                    session_id: None,
                    code: "protocol".into(),
                    message: "unrecognised control message".into(),
                },
            )
            .await;
            continue;
        };

        if !dispatch(&state, &sink, &owned, parsed, addr).await {
            break;
        }
    }

    forwarder.abort();
    heartbeat.abort();
    if let Some(client) = web_client {
        // The phone closed its tab; keep the pairing (it may reload) but stop
        // claiming it is present.
        let _ = client;
    }
}

/// Ping the connection, and while a paired browser keeps answering, keep it
/// listed as online. When it goes quiet, it simply ages out of the list.
fn spawn_heartbeat(
    state: Arc<AppState>,
    sink: Arc<Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    web_client_id: Option<String>,
    opened: std::time::Instant,
    last_heard: Arc<std::sync::atomic::AtomicU64>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Some(id) = &web_client_id {
            state.touch_web_client(id).await;
        }
        let mut tick = tokio::time::interval(PRESENCE_EVERY);
        let mut since_ping = Duration::ZERO;
        loop {
            tick.tick().await;
            since_ping += PRESENCE_EVERY;
            if since_ping >= PING_EVERY {
                since_ping = Duration::ZERO;
                if sink.lock().await.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            let Some(id) = &web_client_id else { continue };
            let heard = Duration::from_millis(last_heard.load(std::sync::atomic::Ordering::Relaxed));
            let silent_for = opened.elapsed().saturating_sub(heard);
            if silent_for <= PRESENCE_GRACE {
                state.touch_web_client(id).await;
            }
        }
    })
}

/// Handle one inbound frame. Returns false to close the connection.
async fn dispatch(
    state: &Arc<AppState>,
    sink: &Arc<Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    owned: &Arc<Mutex<HashSet<String>>>,
    msg: ControlMsg,
    addr: SocketAddr,
) -> bool {
    match msg {
        ControlMsg::Hello { .. } => {
            let device = state.device_info().await;
            let pin_required = state.settings.read().await.pin_required();
            send(
                sink,
                ControlMsg::Welcome {
                    device,
                    pin_required,
                },
            )
            .await
        }

        ControlMsg::Ping { t } => send(sink, ControlMsg::Pong { t }).await,

        ControlMsg::Offer {
            files,
            pin,
            direction,
            conflict,
            kind,
            text,
            ..
        } => {
            handle_offer(
                state, sink, owned, files, pin, direction, conflict, kind, text, addr,
            )
            .await
        }

        ControlMsg::Cancel { session_id } => {
            let known = owned.lock().await.contains(&session_id);
            if known {
                {
                    let sessions = state.sessions.read().await;
                    if let Some(s) = sessions.get(&session_id) {
                        s.control.cancel();
                    }
                }
                crate::transfer::conclude(state, &session_id, SessionStatus::Cancelled, None).await;
            }
            true
        }

        ControlMsg::Complete { session_id } => {
            // Advisory: the receiver already concludes when the last file
            // lands. Only act if we are still mid-flight.
            let should_finish = {
                let sessions = state.sessions.read().await;
                sessions
                    .get(&session_id)
                    .map(|s| s.files_done >= s.files.len() && !s.status.is_terminal())
                    .unwrap_or(false)
            };
            if should_finish && owned.lock().await.contains(&session_id) {
                crate::transfer::conclude(state, &session_id, SessionStatus::Completed, None).await;
            }
            true
        }

        // Progress reports from the sender are informational; our own byte
        // counter is authoritative, so they are accepted and ignored.
        ControlMsg::Progress { .. } | ControlMsg::Pong { .. } => true,

        // Frames only a server sends. Receiving one means the peer is confused.
        ControlMsg::Welcome { .. }
        | ControlMsg::OfferStatus { .. }
        | ControlMsg::Session { .. }
        | ControlMsg::Event { .. }
        | ControlMsg::Error { .. } => true,
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_offer(
    state: &Arc<AppState>,
    sink: &Arc<Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    owned: &Arc<Mutex<HashSet<String>>>,
    files: Vec<crate::model::FileMeta>,
    pin: Option<String>,
    direction: Direction,
    conflict: ConflictPolicy,
    kind: PayloadKind,
    text: Option<String>,
    addr: SocketAddr,
) -> bool {
    // 0. The tray switch: while sharing is off nothing is accepted.
    if !state.sharing() {
        return send(
            sink,
            ControlMsg::Error {
                session_id: None,
                code: "sharing-off".into(),
                message: "the other device has paused sharing".into(),
            },
        )
        .await;
    }

    // 1. PIN, before anything is allocated.
    {
        let settings = state.settings.read().await;
        if !settings.pin_matches(pin.as_deref()) {
            return send(
                sink,
                ControlMsg::Error {
                    session_id: None,
                    code: "pin-rejected".into(),
                    message: "incorrect transfer PIN".into(),
                },
            )
            .await;
        }
    }

    // 2. Shape of the offer.
    if files.is_empty() {
        return send(
            sink,
            ControlMsg::Error {
                session_id: None,
                code: "empty-offer".into(),
                message: "the offer contained no files".into(),
            },
        )
        .await;
    }
    if direction != Direction::Push {
        return send(
            sink,
            ControlMsg::Error {
                session_id: None,
                code: "unsupported".into(),
                message: "this device only accepts pushed transfers".into(),
            },
        )
        .await;
    }
    // 2b. A text share must actually carry text, and not an essay.
    if kind == PayloadKind::Text {
        let body = text.as_deref().unwrap_or("");
        if body.is_empty() {
            return send(
                sink,
                ControlMsg::Error {
                    session_id: None,
                    code: "empty-offer".into(),
                    message: "the shared text was empty".into(),
                },
            )
            .await;
        }
        if body.len() > MAX_TEXT_BYTES {
            return send(
                sink,
                ControlMsg::Error {
                    session_id: None,
                    code: "text-too-long".into(),
                    message: format!(
                        "shared text is limited to {} KB — send it as a file instead",
                        MAX_TEXT_BYTES / 1024
                    ),
                },
            )
            .await;
        }
    }

    // 3. Every path must be safe *before* the user is even asked.
    for file in &files {
        if let Err(err) = crate::transfer::paths::sanitize_rel_path(&file.rel_path) {
            return send(
                sink,
                ControlMsg::Error {
                    session_id: None,
                    code: "invalid-path".into(),
                    message: err.message().to_string(),
                },
            )
            .await;
        }
    }

    let (auto_accept, download_dir, max_concurrent, our_conflict) = {
        let s = state.settings.read().await;
        (
            s.auto_accept,
            s.download_dir.clone(),
            s.max_concurrent,
            s.conflict_policy,
        )
    };
    if state.sessions.read().await.active_count() >= max_concurrent {
        return send(
            sink,
            ControlMsg::Error {
                session_id: None,
                code: "busy".into(),
                message: format!("already running {max_concurrent} transfers"),
            },
        )
        .await;
    }

    // The receiver's own conflict policy wins; the sender's is a hint only.
    let _ = conflict;
    let peer = state
        .peers
        .read()
        .await
        .list()
        .into_iter()
        .find(|p| p.addresses.contains(&addr.ip()))
        .map(|p| p.device)
        .unwrap_or_else(|| crate::model::DeviceInfo {
            id: format!("lan-{}", addr.ip()),
            name: addr.ip().to_string(),
            os: "unknown".into(),
            kind: "desktop".into(),
            version: crate::model::PROTOCOL_VERSION,
            port: addr.port(),
            tls: false,
            pin_required: false,
        });

    let mut session = Session::new(
        peer,
        files,
        Direction::Push,
        Role::Receiving,
        our_conflict,
        download_dir,
    );
    session.kind = kind;
    session.text = text;
    session.detect_conflicts();
    let is_text = kind == PayloadKind::Text;
    let session_id = session.id.clone();
    let token = session.token().to_string();
    let status_changed = session.status_changed.clone();
    if auto_accept {
        session.set_status(SessionStatus::Accepted);
    }
    let view = session.view();
    state.sessions.write().await.insert(session);
    owned.lock().await.insert(session_id.clone());

    if auto_accept {
        // Tell the sender first: for a text share the session is about to
        // become terminal, and the sender needs the acceptance to see it
        // succeed rather than time out.
        let accepted = send(
            sink,
            ControlMsg::OfferStatus {
                session_id: session_id.clone(),
                status: SessionStatus::Accepted,
                token: Some(token),
                reason: None,
            },
        )
        .await;

        if is_text {
            crate::transfer::finish_text_receive(state, &session_id).await;
        } else {
            state.publish_session(&session_id).await;
        }
        return accepted;
    }

    // Ask the user, and tell the sender we are waiting.
    state.publish(crate::events::AppEvent::IncomingOffer { session: view });
    if !send(
        sink,
        ControlMsg::OfferStatus {
            session_id: session_id.clone(),
            status: SessionStatus::Pending,
            token: None,
            reason: None,
        },
    )
    .await
    {
        return false;
    }

    // Wait for the answer on a task so this connection keeps serving frames
    // (a `Cancel` from the sender must still be honoured while we wait).
    let task_state = state.clone();
    let task_sink = sink.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + crate::transfer::session::OFFER_TTL;
        loop {
            let current = {
                let sessions = task_state.sessions.read().await;
                match sessions.get(&session_id) {
                    Some(s) => s.status,
                    None => SessionStatus::Cancelled,
                }
            };
            match current {
                SessionStatus::Pending => {}
                SessionStatus::Accepted => {
                    let _ = send(
                        &task_sink,
                        ControlMsg::OfferStatus {
                            session_id,
                            status: SessionStatus::Accepted,
                            token: Some(token),
                            reason: None,
                        },
                    )
                    .await;
                    return;
                }
                other => {
                    let _ = send(
                        &task_sink,
                        ControlMsg::OfferStatus {
                            session_id,
                            status: SessionStatus::Rejected,
                            token: None,
                            reason: Some(match other {
                                SessionStatus::Rejected => "declined on the other device".into(),
                                _ => format!("{other:?}"),
                            }),
                        },
                    )
                    .await;
                    return;
                }
            }

            if tokio::time::Instant::now() >= deadline {
                task_state.publish(crate::events::AppEvent::notice(
                    NoticeLevel::Warn,
                    "offer-timeout",
                    "An incoming transfer request timed out.",
                ));
                crate::transfer::conclude(
                    &task_state,
                    &session_id,
                    SessionStatus::Rejected,
                    Some(TransferError::Rejected("no answer".into())),
                )
                .await;
                let _ = send(
                    &task_sink,
                    ControlMsg::OfferStatus {
                        session_id,
                        status: SessionStatus::Rejected,
                        token: None,
                        reason: Some("no answer".into()),
                    },
                )
                .await;
                return;
            }
            tokio::select! {
                _ = status_changed.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(400)) => {}
            }
        }
    });

    true
}

/// Relay bus events this connection is entitled to see.
fn spawn_event_forwarder(
    state: Arc<AppState>,
    sink: Arc<Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    owned: Arc<Mutex<HashSet<String>>>,
    web_client_id: Option<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = state.events.subscribe();
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            };
            if event.audience() != Audience::Shared {
                continue;
            }
            // Session events only go to the connection that owns them, or to
            // the paired browser client the session belongs to.
            if let Some(session_id) = event.session_id() {
                let mine = owned.lock().await.contains(session_id);
                let theirs = match &web_client_id {
                    Some(client_id) => state
                        .sessions
                        .read()
                        .await
                        .get(session_id)
                        .map(|s| &s.peer.id == client_id)
                        .unwrap_or(false),
                    None => false,
                };
                if !mine && !theirs {
                    continue;
                }
                if theirs {
                    owned.lock().await.insert(session_id.to_string());
                }
            } else if web_client_id.is_none() {
                // Broadcast notices are only useful to a paired UI.
                continue;
            }

            if !send(&sink, ControlMsg::Event { event }).await {
                return;
            }
        }
    })
}

async fn send(
    sink: &Arc<Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    msg: ControlMsg,
) -> bool {
    let text = msg.to_text();
    sink.lock()
        .await
        .send(Message::Text(text.into()))
        .await
        .is_ok()
}
