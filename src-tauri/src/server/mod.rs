//! The local HTTP server: data plane, control plane and the browser UI.
//!
//! It binds `0.0.0.0` so LAN peers can reach it, but [`lan_guard`] rejects any
//! connection whose source address is not private. There is no setting that
//! relaxes this — the app is not designed to be exposed to the internet.

pub mod protocol;
pub mod routes;
pub mod tls;
pub mod ws;

use crate::config::STREAM_CHUNK;
use crate::state::AppState;
use crate::transfer::session::Control;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use bytes::Bytes;
use include_dir::{include_dir, Dir};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

/// The built browser UI, compiled into the binary so a phone can load it with
/// no extra files on disk.
static WEB_UI: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../dist");

/// Ports tried in order before falling back to an ephemeral one.
const PORT_ATTEMPTS: u16 = 12;

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/api/info", get(routes::info))
        .route("/api/web/session", post(routes::web_join))
        .route("/api/web/offer", post(routes::web_offer))
        .route("/api/offer/{session_id}", get(routes::offer_status))
        .route(
            "/api/transfer/{session_id}/{index}/status",
            get(routes::transfer_status),
        )
        .route(
            "/api/transfer/{session_id}/{index}",
            put(routes::transfer_upload).get(routes::transfer_download),
        )
        .route(
            "/api/transfer/{session_id}",
            delete(routes::transfer_cancel),
        )
        .route("/ws", get(ws::upgrade))
        // File bodies are streamed and can be many gigabytes; the per-file
        // ceiling is enforced against the *declared* size in `receive_file`.
        .layer(DefaultBodyLimit::disable());

    Router::new()
        .merge(api)
        .fallback(serve_web_ui)
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn(lan_guard))
        .with_state(state)
}

/// Reject any request that did not come from a private address.
async fn lan_guard(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if !crate::net::is_lan_ip(addr.ip()) {
        return (
            StatusCode::FORBIDDEN,
            "ShareLocal only accepts connections from the local network.",
        )
            .into_response();
    }
    next.run(request).await
}

/// Conservative headers for the browser client. The UI is entirely self-hosted,
/// so the policy can forbid every external origin.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; \
             script-src 'self'; connect-src 'self' ws: wss:; frame-ancestors 'none'; \
             base-uri 'none'; form-action 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// Serve the embedded SPA, falling back to `index.html` for client routes.
async fn serve_web_ui(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // The lookup is against an in-memory tree, so traversal is impossible, but
    // reject the obvious attempts anyway for clarity.
    if path.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }

    let file = WEB_UI
        .get_file(path)
        .or_else(|| WEB_UI.get_file("index.html"));

    match file {
        Some(file) => {
            let mime = mime_guess::from_path(file.path())
                .first_or_octet_stream()
                .to_string();
            let cache = if path == "index.html" {
                "no-cache"
            } else {
                // Vite emits content-hashed asset names.
                "public, max-age=31536000, immutable"
            };
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, cache.to_string()),
                ],
                file.contents(),
            )
                .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            "ShareLocal's web interface has not been built. Run `npm run build`.",
        )
            .into_response(),
    }
}

/// Run the server, rebuilding the listener whenever the port or TLS setting
/// changes.
pub async fn run(state: Arc<AppState>) {
    // rustls needs one process-wide crypto provider. Installing it here (and
    // ignoring a second attempt) keeps the server, the WebSocket dialer and
    // reqwest on the same backend.
    let _ = rustls::crypto::ring::default_provider().install_default();

    loop {
        let (requested_port, use_tls) = {
            let s = state.settings.read().await;
            (s.port, s.tls)
        };

        let listener = match bind(requested_port).await {
            Some(l) => l,
            None => {
                state.publish(crate::events::AppEvent::notice(
                    crate::events::NoticeLevel::Error,
                    "port-unavailable",
                    format!(
                        "Could not open port {requested_port}. Try a different port in Settings."
                    ),
                ));
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        let actual = listener
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(requested_port);
        state.bound_port.store(actual, Ordering::SeqCst);
        state.server_ready.store(true, Ordering::SeqCst);
        state.publish_network().await;
        state.network_changed.notify_waiters();

        let app = router(state.clone());
        let shutdown = {
            let state = state.clone();
            async move { state.restart_server.notified().await }
        };

        if use_tls {
            serve_tls(state.clone(), listener, app).await;
        } else {
            let service = app.into_make_service_with_connect_info::<SocketAddr>();
            if let Err(err) = axum::serve(listener, service)
                .with_graceful_shutdown(shutdown)
                .await
            {
                eprintln!("[server] stopped: {err}");
            }
        }

        state.server_ready.store(false, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn serve_tls(state: Arc<AppState>, listener: tokio::net::TcpListener, app: Router) {
    let material = match tls::ensure_certificate(&state.data_dir) {
        Ok(m) => m,
        Err(err) => {
            state.publish(crate::events::AppEvent::notice(
                crate::events::NoticeLevel::Error,
                "tls-failed",
                format!("Could not create a TLS certificate ({err}); serving without TLS."),
            ));
            let service = app.into_make_service_with_connect_info::<SocketAddr>();
            let shutdown = async move { state.restart_server.notified().await };
            let _ = axum::serve(listener, service)
                .with_graceful_shutdown(shutdown)
                .await;
            return;
        }
    };

    let config = match axum_server::tls_rustls::RustlsConfig::from_pem(
        material.cert_pem.into_bytes(),
        material.key_pem.into_bytes(),
    )
    .await
    {
        Ok(c) => c,
        Err(err) => {
            eprintln!("[server] TLS config failed: {err}");
            return;
        }
    };

    let addr = match listener.local_addr() {
        Ok(a) => a,
        Err(_) => return,
    };
    // `axum_server` owns its own listener, so release ours first.
    drop(listener);

    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();
    let shutdown_state = state.clone();
    tokio::spawn(async move {
        shutdown_state.restart_server.notified().await;
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(2)));
    });

    if let Err(err) = axum_server::bind_rustls(addr, config)
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
    {
        eprintln!("[server] TLS server stopped: {err}");
    }
}

/// Try the configured port, then a small range after it, then any free port.
async fn bind(preferred: u16) -> Option<tokio::net::TcpListener> {
    for offset in 0..PORT_ATTEMPTS {
        let port = preferred.saturating_add(offset);
        if let Ok(listener) = tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
            return Some(listener);
        }
    }
    tokio::net::TcpListener::bind(("0.0.0.0", 0)).await.ok()
}

/// Stream a file out to a downloading peer, updating session progress as it
/// goes. Like the upload path, memory use is one chunk regardless of size.
#[allow(clippy::too_many_arguments)]
pub fn download_stream(
    file: tokio::fs::File,
    remaining: u64,
    state: Arc<AppState>,
    session_id: String,
    index: usize,
    control: Arc<Control>,
    start_offset: u64,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    struct Source {
        file: tokio::fs::File,
        remaining: u64,
        state: Arc<AppState>,
        session_id: String,
        index: usize,
        control: Arc<Control>,
        sent: u64,
        last_publish: Instant,
        buf: Vec<u8>,
        /// Guards the completion bookkeeping, which must run exactly once.
        settled: bool,
    }

    /// Fold the finished file into the session and conclude if it was the last.
    async fn settle(src: &mut Source) {
        if src.settled {
            return;
        }
        src.settled = true;
        let all_done = {
            let mut sessions = src.state.sessions.write().await;
            match sessions.get_mut(&src.session_id) {
                Some(s) => {
                    s.complete_current_file(None);
                    s.files_done >= s.files.len()
                }
                None => false,
            }
        };
        if all_done {
            crate::transfer::conclude(
                &src.state,
                &src.session_id,
                crate::model::SessionStatus::Completed,
                None,
            )
            .await;
        } else {
            src.state.publish_session(&src.session_id).await;
        }
    }

    let source = Source {
        file,
        remaining,
        state,
        session_id,
        index,
        control,
        sent: start_offset,
        last_publish: Instant::now(),
        buf: vec![0u8; STREAM_CHUNK],
        settled: false,
    };

    futures_util::stream::unfold(Some(source), |maybe| async move {
        let mut src = maybe?;
        if src.remaining == 0 {
            settle(&mut src).await;
            return None;
        }
        if !src.control.wait_if_paused().await {
            return Some((Err(std::io::Error::other("transfer cancelled")), None));
        }

        let want = src.remaining.min(STREAM_CHUNK as u64) as usize;
        match src.file.read(&mut src.buf[..want]).await {
            Ok(0) => {
                // The file ended earlier than its declared size; still settle
                // so the session does not hang in `Transferring` forever.
                settle(&mut src).await;
                None
            }
            Ok(n) => {
                src.remaining -= n as u64;
                src.sent += n as u64;
                {
                    let mut sessions = src.state.sessions.write().await;
                    if let Some(s) = sessions.get_mut(&src.session_id) {
                        s.meter.record(n as u64);
                        s.file_index = src.index;
                        s.file_bytes_done = src.sent;
                    }
                }
                let chunk = Bytes::copy_from_slice(&src.buf[..n]);

                // Settle on the *last* chunk rather than waiting for another
                // poll: once Content-Length bytes have been handed over, hyper
                // stops polling the body and that poll would never arrive.
                if src.remaining == 0 {
                    settle(&mut src).await;
                } else if src.last_publish.elapsed() >= Duration::from_millis(200) {
                    src.last_publish = Instant::now();
                    src.state.publish_session(&src.session_id).await;
                }

                Some((Ok(chunk), Some(src)))
            }
            Err(err) => Some((Err(err), None)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bind_falls_back_when_the_port_is_taken() {
        // Must be the same wildcard address `bind` uses; holding only
        // 127.0.0.1 would not actually conflict with a 0.0.0.0 bind.
        let Ok(taken) = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await else {
            return;
        };
        let port = taken.local_addr().unwrap().port();

        let listener = bind(port).await.expect("a listener is always produced");
        assert_ne!(
            listener.local_addr().unwrap().port(),
            port,
            "a busy port must not be reused"
        );
    }

    #[test]
    fn the_web_ui_is_embedded() {
        assert!(
            WEB_UI.get_file("index.html").is_some(),
            "the browser client must be compiled into the binary"
        );
    }
}
