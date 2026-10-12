//! HTTP surface. Everything here is reachable only from the LAN (enforced by
//! [`super::guard`]) and everything touching files additionally requires a
//! session bearer token.

use crate::model::{
    DeviceInfo, Direction, FileMeta, Role, SessionStatus, TransferError, PROTOCOL_VERSION,
};
use crate::state::AppState;
use crate::transfer::session::Session;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;

/// Maps a `TransferError` onto an HTTP status, keeping the structured body so
/// the sender can react precisely.
pub struct ApiError(pub TransferError);

impl From<TransferError> for ApiError {
    fn from(e: TransferError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        use TransferError::*;
        let status = match &self.0 {
            InvalidToken(_) => StatusCode::UNAUTHORIZED,
            PinRejected(_) => StatusCode::FORBIDDEN,
            PermissionDenied(_) => StatusCode::FORBIDDEN,
            InvalidPath(_) => StatusCode::BAD_REQUEST,
            FileMissing(_) => StatusCode::NOT_FOUND,
            DuplicateName(_) => StatusCode::CONFLICT,
            NotEnoughSpace(_) => StatusCode::INSUFFICIENT_STORAGE,
            Rejected(_) => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_GATEWAY,
        };
        (status, Json(self.0)).into_response()
    }
}

/// Pull the bearer token out of `Authorization`, or the `token` query
/// parameter (browsers cannot set headers on a plain `<a download>`).
pub fn bearer(headers: &HeaderMap, fallback: Option<&str>) -> Result<String, TransferError> {
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = value.strip_prefix("Bearer ") {
            if !token.trim().is_empty() {
                return Ok(token.trim().to_string());
            }
        }
    }
    fallback
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.to_string())
        .ok_or_else(|| TransferError::InvalidToken("missing transfer token".into()))
}

// ---------------------------------------------------------------- /api/info

#[derive(Debug, Serialize)]
pub struct InfoResponse {
    #[serde(flatten)]
    device: DeviceInfo,
    protocol: u32,
    /// Whether a QR pairing token is currently outstanding.
    pairing_open: bool,
}

pub async fn info(State(state): State<Arc<AppState>>) -> Json<InfoResponse> {
    let device = state.device_info().await;
    let pairing_open = state
        .pairing
        .read()
        .await
        .as_ref()
        .map(|p| p.is_valid())
        .unwrap_or(false);
    Json(InfoResponse {
        device,
        protocol: PROTOCOL_VERSION,
        pairing_open,
    })
}

// -------------------------------------------------------- /api/web/session

#[derive(Debug, Deserialize)]
pub struct JoinRequest {
    pub pairing_token: String,
    pub name: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub kind: String,
    /// Client-generated id so a reload keeps the same identity.
    pub client_id: String,
}

#[derive(Debug, Serialize)]
pub struct JoinResponse {
    pub client_id: String,
    pub client_token: String,
    pub device: DeviceInfo,
    pub max_upload_bytes: Option<u64>,
}

/// Redeem a QR pairing token. This is how a phone joins without installing
/// anything: it becomes a pinned peer with its own bearer token.
pub async fn web_join(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<JoinRequest>,
) -> Result<Json<JoinResponse>, ApiError> {
    let name = req.name.trim();
    let name = if name.is_empty() { "Browser" } else { name };
    let device = DeviceInfo {
        id: sanitise_client_id(&req.client_id)?,
        name: name.chars().take(64).collect(),
        os: pick_label(&req.os, "browser"),
        kind: pick_label(&req.kind, "browser"),
        version: PROTOCOL_VERSION,
        port: 0,
        tls: false,
        pin_required: false,
    };

    let client = state
        .redeem_pairing(&req.pairing_token, device, addr.ip())
        .await
        .ok_or_else(|| {
            TransferError::InvalidToken(
                "this QR code has expired — show a new one on the desktop".into(),
            )
        })?;

    Ok(Json(JoinResponse {
        client_id: client.id,
        client_token: client.token,
        device: state.device_info().await,
        max_upload_bytes: None,
    }))
}

fn pick_label(value: &str, fallback: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(24)
        .collect();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

fn sanitise_client_id(raw: &str) -> Result<String, TransferError> {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect();
    if cleaned.len() < 8 {
        return Err(TransferError::Other("invalid client id".into()));
    }
    Ok(format!("web-{cleaned}"))
}

// ------------------------------------------------------------ /api/offer

#[derive(Debug, Deserialize)]
pub struct OfferRequest {
    pub files: Vec<FileMeta>,
    #[serde(default)]
    pub pin: Option<String>,
    pub client_id: String,
    #[serde(default)]
    pub kind: crate::model::PayloadKind,
    /// Set for text shares; the snippet travels with the offer.
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OfferResponse {
    pub session_id: String,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// The browser client's equivalent of the WebSocket `Offer` frame. Kept as a
/// plain POST so a phone can start an upload with one round trip.
pub async fn web_offer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<OfferRequest>,
) -> Result<Json<OfferResponse>, ApiError> {
    let token = bearer(&headers, None)?;
    let client = state
        .authenticate_web(&req.client_id, &token)
        .await
        .ok_or_else(|| TransferError::InvalidToken("this browser session has expired".into()))?;

    {
        let settings = state.settings.read().await;
        if !settings.pin_matches(req.pin.as_deref()) {
            return Err(TransferError::PinRejected("incorrect transfer PIN".into()).into());
        }
    }
    if req.files.is_empty() {
        return Err(TransferError::Other("no files in the offer".into()).into());
    }

    let is_text = req.kind == crate::model::PayloadKind::Text;
    if is_text {
        let body = req.text.as_deref().unwrap_or("");
        if body.trim().is_empty() {
            return Err(TransferError::Other("the shared text was empty".into()).into());
        }
        if body.len() > crate::model::MAX_TEXT_BYTES {
            return Err(TransferError::Other(format!(
                "shared text is limited to {} KB",
                crate::model::MAX_TEXT_BYTES / 1024
            ))
            .into());
        }
    } else {
        // Only text may skip the data plane; a file offer must name real paths.
        for file in &req.files {
            crate::transfer::paths::sanitize_rel_path(&file.rel_path)?;
        }
    }

    let (auto_accept, conflict, download_dir, max_concurrent) = {
        let s = state.settings.read().await;
        (
            s.auto_accept,
            s.conflict_policy,
            s.download_dir.clone(),
            s.max_concurrent,
        )
    };
    if state.sessions.read().await.active_count() >= max_concurrent {
        return Err(TransferError::Other("too many transfers are already running".into()).into());
    }

    let mut session = Session::new(
        client.device.clone(),
        req.files,
        Direction::Push,
        Role::Receiving,
        conflict,
        download_dir,
    );
    session.kind = req.kind;
    session.text = req.text;
    session.detect_conflicts();
    if auto_accept {
        session.set_status(SessionStatus::Accepted);
    }
    let response = OfferResponse {
        session_id: session.id.clone(),
        status: session.status,
        token: auto_accept.then(|| session.token().to_string()),
    };
    let view = session.view();
    let id = state.sessions.write().await.insert(session);

    if auto_accept {
        if is_text {
            // Nothing to upload: the snippet came with the offer.
            crate::transfer::finish_text_receive(&state, &id).await;
        } else {
            state.publish_session(&id).await;
        }
    } else {
        state.publish(crate::events::AppEvent::IncomingOffer { session: view });
    }
    Ok(Json(response))
}

/// Poll an offer the desktop user has not answered yet.
pub async fn offer_status(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Result<Json<OfferResponse>, ApiError> {
    let token = bearer(&headers, q.token.as_deref())?;
    let sessions = state.sessions.read().await;
    // An unaccepted session has no token the peer could know, so a browser
    // polls with its *client* token instead; fall back to that.
    let session = match sessions.get(&session_id) {
        Some(s) => s,
        None => return Err(TransferError::InvalidToken("unknown session".into()).into()),
    };
    let authorised = session.token_matches(&token) || {
        drop(sessions);
        let sessions = state.sessions.read().await;
        let s = sessions.get(&session_id);
        match s {
            Some(s) => state.authenticate_web(&s.peer.id, &token).await.is_some(),
            None => false,
        }
    };
    if !authorised {
        return Err(TransferError::InvalidToken("invalid token".into()).into());
    }

    let sessions = state.sessions.read().await;
    let session = sessions
        .get(&session_id)
        .ok_or_else(|| TransferError::InvalidToken("unknown session".into()))?;
    Ok(Json(OfferResponse {
        session_id: session.id.clone(),
        status: session.status,
        token: (session.status == SessionStatus::Accepted).then(|| session.token().to_string()),
    }))
}

// --------------------------------------------------------- /api/transfer

#[derive(Debug, Deserialize, Default)]
pub struct TokenQuery {
    pub token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub received: u64,
    pub size: u64,
    pub status: SessionStatus,
}

/// How many bytes the receiver already holds — the basis for resuming.
pub async fn transfer_status(
    State(state): State<Arc<AppState>>,
    Path((session_id, index)): Path<(String, usize)>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Result<Json<StatusResponse>, ApiError> {
    let token = bearer(&headers, q.token.as_deref())?;
    let (root, size, status) = {
        let sessions = state.sessions.read().await;
        let session = sessions.authorise(&session_id, &token)?;
        let size = session
            .files
            .get(index)
            .map(|f| f.size)
            .ok_or_else(|| TransferError::Other("file index out of range".into()))?;
        (session.dest_root.clone(), size, session.status)
    };
    let received = crate::transfer::received_bytes(&root, &session_id, index).await;
    Ok(Json(StatusResponse {
        received: received.min(size),
        size,
        status,
    }))
}

/// Receive one file. The body is streamed straight to disk.
pub async fn transfer_upload(
    State(state): State<Arc<AppState>>,
    Path((session_id, index)): Path<(String, usize)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<serde_json::Value>, ApiError> {
    let token = bearer(&headers, None)?;
    {
        let sessions = state.sessions.read().await;
        let session = sessions.authorise(&session_id, &token)?;
        if session.role != Role::Receiving {
            return Err(
                TransferError::InvalidToken("this session does not accept uploads".into()).into(),
            );
        }
        if !matches!(
            session.status,
            SessionStatus::Accepted | SessionStatus::Transferring | SessionStatus::Paused
        ) {
            return Err(TransferError::Rejected(format!(
                "session is {:?} and cannot receive data",
                session.status
            ))
            .into());
        }
    }

    let offset: u64 = headers
        .get("x-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let stream = body.into_data_stream();
    let saved = crate::transfer::receive_file(&state, &session_id, index, offset, stream).await;

    match saved {
        Ok(path) => {
            let finished = {
                let mut sessions = state.sessions.write().await;
                match sessions.get_mut(&session_id) {
                    Some(s) => {
                        s.complete_current_file(Some(path.clone()));
                        s.files_done >= s.files.len()
                    }
                    None => false,
                }
            };
            if finished {
                crate::transfer::conclude(&state, &session_id, SessionStatus::Completed, None)
                    .await;
            } else {
                state.publish_session(&session_id).await;
            }
            Ok(Json(serde_json::json!({ "ok": true })))
        }
        Err(err) => {
            // Interruptions keep their part file so the sender can resume;
            // anything else is terminal for this session.
            let terminal = !matches!(err, TransferError::Interrupted(_));
            if terminal {
                crate::transfer::conclude(
                    &state,
                    &session_id,
                    SessionStatus::Failed,
                    Some(err.clone()),
                )
                .await;
            } else {
                state.publish_session(&session_id).await;
            }
            Err(err.into())
        }
    }
}

/// Serve one file of an outgoing (`Pull`) session — how a browser downloads.
pub async fn transfer_download(
    State(state): State<Arc<AppState>>,
    Path((session_id, index)): Path<(String, usize)>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Result<Response, ApiError> {
    let token = bearer(&headers, q.token.as_deref())?;
    let (path, meta, control) = {
        let sessions = state.sessions.read().await;
        let session = sessions.authorise(&session_id, &token)?;
        if session.role != Role::Sending {
            return Err(TransferError::InvalidToken(
                "this session does not serve downloads".into(),
            )
            .into());
        }
        let path = session
            .source_paths
            .get(index)
            .cloned()
            .ok_or_else(|| TransferError::FileMissing("file index out of range".into()))?;
        let meta = session
            .files
            .get(index)
            .cloned()
            .ok_or_else(|| TransferError::FileMissing("file index out of range".into()))?;
        (path, meta, session.control.clone())
    };

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| crate::transfer::io_error(&e, "opening the file"))?;
    let total = file
        .metadata()
        .await
        .map(|m| m.len())
        .map_err(|e| crate::transfer::io_error(&e, "reading the file"))?;

    // Honour a simple `Range: bytes=start-` so a paused download can resume.
    let range_start = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range_start)
        .filter(|s| *s < total);

    let mut file = file;
    if let Some(start) = range_start {
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| crate::transfer::io_error(&e, "seeking the file"))?;
    }
    let start = range_start.unwrap_or(0);
    let length = total - start;

    {
        let mut sessions = state.sessions.write().await;
        if let Some(s) = sessions.get_mut(&session_id) {
            s.file_index = index;
            s.file_bytes_done = start;
            s.bytes_before_current = s.files.iter().take(index).map(|f| f.size).sum();
            s.set_status(SessionStatus::Transferring);
        }
    }
    state.publish_session(&session_id).await;

    let stream = super::download_stream(
        file,
        length,
        state.clone(),
        session_id.clone(),
        index,
        control,
        start,
    );

    let filename = meta
        .rel_path
        .rsplit('/')
        .next()
        .unwrap_or(&meta.rel_path)
        .to_string();
    let mut response = Response::builder()
        .status(if range_start.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(
            header::CONTENT_TYPE,
            meta.mime
                .unwrap_or_else(|| "application/octet-stream".into()),
        )
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CONTENT_DISPOSITION,
            format!(
                "attachment; filename*=UTF-8''{}",
                urlencoding::encode(&filename)
            ),
        );
    if let Some(start) = range_start {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, total.saturating_sub(1), total),
        );
    }
    response
        .body(Body::from_stream(stream))
        .map_err(|e| ApiError(TransferError::Other(e.to_string())))
}

/// `bytes=1024-` → `Some(1024)`. Multi-range requests are not supported.
pub fn parse_range_start(value: &str) -> Option<u64> {
    let spec = value.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let start = spec.split('-').next()?;
    if start.is_empty() {
        return None;
    }
    start.parse().ok()
}

/// Cancel a session from the peer's side.
pub async fn transfer_cancel(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let token = bearer(&headers, q.token.as_deref())?;
    {
        let sessions = state.sessions.read().await;
        let session = sessions.authorise(&session_id, &token)?;
        session.control.cancel();
    }
    crate::transfer::conclude(&state, &session_id, SessionStatus::Cancelled, None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn bearer_token_extraction() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc123"),
        );
        assert_eq!(bearer(&headers, None).unwrap(), "abc123");

        // Query fallback for browser downloads.
        assert_eq!(
            bearer(&HeaderMap::new(), Some("qs-token")).unwrap(),
            "qs-token"
        );
        assert!(bearer(&HeaderMap::new(), None).is_err());
        assert!(bearer(&HeaderMap::new(), Some("  ")).is_err());
    }

    #[test]
    fn non_bearer_authorization_is_refused() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic abc123"),
        );
        assert!(bearer(&headers, None).is_err());

        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer "));
        assert!(bearer(&headers, None).is_err());
    }

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range_start("bytes=1024-"), Some(1024));
        assert_eq!(parse_range_start("bytes=0-511"), Some(0));
        assert_eq!(
            parse_range_start("bytes=-500"),
            None,
            "suffix ranges unsupported"
        );
        assert_eq!(
            parse_range_start("bytes=0-1,5-9"),
            None,
            "multi-range unsupported"
        );
        assert_eq!(parse_range_start("items=1-"), None);
        assert_eq!(parse_range_start("garbage"), None);
    }

    #[test]
    fn client_ids_are_sanitised_and_namespaced() {
        assert_eq!(sanitise_client_id("abcdefgh").unwrap(), "web-abcdefgh");
        assert_eq!(
            sanitise_client_id("../../etc/passwd-aaaa").unwrap(),
            "web-etcpasswd-aaaa",
            "path characters are stripped before the id is ever used"
        );
        assert!(sanitise_client_id("short").is_err());
    }

    #[test]
    fn labels_fall_back_when_hostile_or_empty() {
        assert_eq!(pick_label("", "browser"), "browser");
        assert_eq!(pick_label("<script>", "browser"), "script");
        assert_eq!(pick_label("android", "browser"), "android");
    }

    #[test]
    fn error_status_mapping() {
        let cases = [
            (
                TransferError::InvalidToken("".into()),
                StatusCode::UNAUTHORIZED,
            ),
            (TransferError::PinRejected("".into()), StatusCode::FORBIDDEN),
            (
                TransferError::InvalidPath("".into()),
                StatusCode::BAD_REQUEST,
            ),
            (TransferError::FileMissing("".into()), StatusCode::NOT_FOUND),
            (
                TransferError::DuplicateName("".into()),
                StatusCode::CONFLICT,
            ),
            (
                TransferError::NotEnoughSpace("".into()),
                StatusCode::INSUFFICIENT_STORAGE,
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(ApiError(err).into_response().status(), expected);
        }
    }
}
