//! Session bookkeeping: identity, bearer tokens, live progress, pause/cancel.
//!
//! A session is created by an *offer* and destroyed when the transfer reaches a
//! terminal state or its TTL expires. The token it carries is the only thing
//! that authorises a peer to touch the data plane.

use crate::model::{
    ConflictPolicy, DeviceInfo, Direction, FileMeta, PayloadKind, Progress, Role, SessionStatus,
    SessionView, TransferError,
};
use base64::Engine;
use rand::TryRngCore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

/// How long a session may sit unfinished before it is swept away.
pub const SESSION_TTL: Duration = Duration::from_secs(60 * 60 * 6);
/// How long an un-answered offer stays on screen before it auto-rejects.
pub const OFFER_TTL: Duration = Duration::from_secs(120);

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 256 bits from the OS CSPRNG, base64url encoded.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS RNG must be available");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn random_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Sliding-window throughput estimate. A plain "total / elapsed" average reads
/// badly on long transfers (it lags for minutes after a stall), so we keep a
/// short window of samples instead.
#[derive(Debug)]
pub struct SpeedMeter {
    window: Duration,
    samples: std::collections::VecDeque<(Instant, u64)>,
    total: u64,
    started: Instant,
}

impl Default for SpeedMeter {
    fn default() -> Self {
        Self::new(Duration::from_millis(3_000))
    }
}

impl SpeedMeter {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            samples: std::collections::VecDeque::new(),
            total: 0,
            started: Instant::now(),
        }
    }

    pub fn record(&mut self, bytes: u64) {
        self.record_at(bytes, Instant::now());
    }

    pub fn record_at(&mut self, bytes: u64, at: Instant) {
        self.total = self.total.saturating_add(bytes);
        self.samples.push_back((at, bytes));
        while let Some((t, _)) = self.samples.front() {
            if at.duration_since(*t) > self.window {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Bytes per second over the window, or 0 before there is enough data.
    pub fn speed_bps(&self) -> f64 {
        self.speed_bps_at(Instant::now())
    }

    pub fn speed_bps_at(&self, now: Instant) -> f64 {
        let Some((oldest, _)) = self.samples.front() else {
            return 0.0;
        };
        let span = now.duration_since(*oldest).as_secs_f64();
        if span <= 0.001 {
            return 0.0;
        }
        // The oldest sample marks the start of the window; the bytes it carried
        // were transferred before it, so they are excluded from the numerator.
        let bytes: u64 = self.samples.iter().skip(1).map(|(_, b)| *b).sum();
        bytes as f64 / span
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn average_bps(&self) -> f64 {
        let elapsed = self.started.elapsed().as_secs_f64();
        if elapsed <= 0.001 {
            0.0
        } else {
            self.total as f64 / elapsed
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn eta_secs(&self, remaining: u64) -> Option<f64> {
        self.eta_secs_at(remaining, Instant::now())
    }

    pub fn eta_secs_at(&self, remaining: u64, now: Instant) -> Option<f64> {
        let speed = self.speed_bps_at(now);
        if speed < 1.0 {
            return None;
        }
        Some(remaining as f64 / speed)
    }
}

/// Shared flags a streaming loop polls between chunks.
#[derive(Debug, Default)]
pub struct Control {
    cancelled: AtomicBool,
    paused: AtomicBool,
    notify: Notify,
}

impl Control {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
        if !paused {
            self.notify.notify_waiters();
        }
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
    /// Block while paused; returns `false` if the session was cancelled.
    pub async fn wait_if_paused(&self) -> bool {
        while self.is_paused() && !self.is_cancelled() {
            self.notify.notified().await;
        }
        !self.is_cancelled()
    }
}

#[derive(Debug)]
pub struct Session {
    pub id: String,
    /// Bearer token for the data plane. Never leaves this process except in the
    /// direct reply to the peer that created the session.
    token: String,
    pub peer: DeviceInfo,
    pub files: Vec<FileMeta>,
    pub total_bytes: u64,
    pub direction: Direction,
    pub role: Role,
    pub status: SessionStatus,
    pub conflict: ConflictPolicy,
    pub created_ms: u64,
    pub created_at: Instant,
    pub error: Option<TransferError>,

    /// Outgoing: absolute source paths, index-aligned with `files`.
    pub source_paths: Vec<PathBuf>,
    /// Incoming: where each finished file landed.
    pub saved_paths: Vec<PathBuf>,
    /// Incoming: the download root this session may write into.
    pub dest_root: PathBuf,

    /// Outgoing only: the peer's identifiers for the same transfer. Our
    /// session id is local; the receiver minted its own.
    pub remote_id: Option<String>,
    pub remote_token: Option<String>,
    pub remote_base: Option<String>,

    pub file_index: usize,
    pub files_done: usize,
    /// Bytes completed in files before `file_index`.
    pub bytes_before_current: u64,
    pub file_bytes_done: u64,

    /// Files in this offer that already exist at the destination.
    pub conflicts: Vec<String>,

    pub kind: PayloadKind,
    /// Populated for text shares; travels with the offer, not over HTTP.
    pub text: Option<String>,

    pub meter: SpeedMeter,
    pub control: Arc<Control>,
    /// Woken whenever `status` changes, so an offer can be awaited.
    pub status_changed: Arc<Notify>,
}

impl Session {
    pub fn new(
        peer: DeviceInfo,
        files: Vec<FileMeta>,
        direction: Direction,
        role: Role,
        conflict: ConflictPolicy,
        dest_root: PathBuf,
    ) -> Self {
        let total_bytes = files.iter().map(|f| f.size).sum();
        Self {
            id: random_id(),
            token: random_token(),
            peer,
            files,
            total_bytes,
            direction,
            role,
            status: SessionStatus::Pending,
            conflict,
            created_ms: now_ms(),
            created_at: Instant::now(),
            error: None,
            source_paths: Vec::new(),
            saved_paths: Vec::new(),
            dest_root,
            remote_id: None,
            remote_token: None,
            remote_base: None,
            file_index: 0,
            files_done: 0,
            bytes_before_current: 0,
            file_bytes_done: 0,
            conflicts: Vec::new(),
            kind: PayloadKind::Files,
            text: None,
            meter: SpeedMeter::default(),
            control: Arc::new(Control::default()),
            status_changed: Arc::new(Notify::new()),
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Work out which of the offered files would collide with something
    /// already in the download folder.
    ///
    /// Done once, at offer time, so the accept prompt can ask about conflicts
    /// only when there genuinely are any. A file that is created between here
    /// and the write is still handled by the policy at finalise time — this is
    /// for the question we put to the user, not for correctness.
    pub fn detect_conflicts(&mut self) {
        if self.role != Role::Receiving {
            return;
        }
        self.conflicts = self
            .files
            .iter()
            .filter(|file| {
                crate::transfer::paths::sanitize_rel_path(&file.rel_path)
                    .ok()
                    .and_then(|rel| crate::transfer::paths::resolve_dest(&self.dest_root, &rel).ok())
                    .map(|dest| dest.exists())
                    .unwrap_or(false)
            })
            .map(|file| file.rel_path.clone())
            .collect();
    }

    /// Constant-time bearer check.
    pub fn token_matches(&self, candidate: &str) -> bool {
        use subtle::ConstantTimeEq;
        if candidate.len() != self.token.len() {
            return false;
        }
        self.token.as_bytes().ct_eq(candidate.as_bytes()).into()
    }

    pub fn set_status(&mut self, status: SessionStatus) {
        if self.status != status {
            self.status = status;
            self.status_changed.notify_waiters();
        }
    }

    pub fn fail(&mut self, err: TransferError) {
        self.error = Some(err);
        self.set_status(SessionStatus::Failed);
    }

    pub fn bytes_done(&self) -> u64 {
        self.bytes_before_current
            .saturating_add(self.file_bytes_done)
    }

    pub fn remaining(&self) -> u64 {
        self.total_bytes.saturating_sub(self.bytes_done())
    }

    pub fn progress(&self) -> Progress {
        Progress {
            file_index: self.file_index,
            files_done: self.files_done,
            bytes_done: self.bytes_done(),
            file_bytes_done: self.file_bytes_done,
            speed_bps: self.meter.speed_bps(),
            eta_secs: self.meter.eta_secs(self.remaining()),
        }
    }

    /// Advance to the next file, folding the finished one into the running total.
    pub fn complete_current_file(&mut self, saved: Option<PathBuf>) {
        if let Some(path) = saved {
            self.saved_paths.push(path);
        }
        let size = self.files.get(self.file_index).map(|f| f.size).unwrap_or(0);
        self.bytes_before_current = self.bytes_before_current.saturating_add(size);
        self.file_bytes_done = 0;
        self.files_done += 1;
        if self.file_index + 1 < self.files.len() {
            self.file_index += 1;
        }
    }

    pub fn display_name(&self) -> String {
        if self.kind == PayloadKind::Text {
            let text = self.text.as_deref().unwrap_or_default();
            let first_line = text.lines().next().unwrap_or("").trim();
            if first_line.is_empty() {
                return "Shared text".into();
            }
            let preview: String = first_line.chars().take(60).collect();
            return if first_line.chars().count() > 60 {
                format!("{preview}…")
            } else {
                preview
            };
        }
        match self.files.len() {
            0 => "(no files)".into(),
            1 => self.files[0].rel_path.clone(),
            n => {
                let first = self.files[0]
                    .rel_path
                    .split('/')
                    .next()
                    .unwrap_or(&self.files[0].rel_path)
                    .to_string();
                format!("{first} + {} more", n - 1)
            }
        }
    }

    /// The token-free projection handed to the UI and to peers.
    pub fn view(&self) -> SessionView {
        SessionView {
            id: self.id.clone(),
            peer: self.peer.clone(),
            files: self.files.clone(),
            total_bytes: self.total_bytes,
            direction: self.direction,
            role: self.role,
            status: self.status,
            progress: self.progress(),
            created_ms: self.created_ms,
            error: self.error.clone(),
            saved_paths: self.saved_paths.clone(),
            conflicts: self.conflicts.clone(),
            kind: self.kind,
            text: self.text.clone(),
        }
    }

    pub fn is_expired(&self) -> bool {
        match self.status {
            SessionStatus::Pending => self.created_at.elapsed() > OFFER_TTL,
            s if s.is_terminal() => self.created_at.elapsed() > Duration::from_secs(60),
            _ => self.created_at.elapsed() > SESSION_TTL,
        }
    }
}

#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: HashMap<String, Session>,
}

impl SessionStore {
    pub fn insert(&mut self, session: Session) -> String {
        let id = session.id.clone();
        self.sessions.insert(id.clone(), session);
        id
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.get_mut(id)
    }

    pub fn remove(&mut self, id: &str) -> Option<Session> {
        self.sessions.remove(id)
    }

    /// Look up a session *and* verify its bearer token in one step, so no call
    /// site can accidentally skip the check.
    pub fn authorise(&self, id: &str, token: &str) -> Result<&Session, TransferError> {
        let session = self
            .sessions
            .get(id)
            .ok_or_else(|| TransferError::InvalidToken("unknown transfer session".into()))?;
        if !session.token_matches(token) {
            return Err(TransferError::InvalidToken("invalid transfer token".into()));
        }
        if session.is_expired() {
            return Err(TransferError::InvalidToken(
                "transfer session expired".into(),
            ));
        }
        Ok(session)
    }

    pub fn authorise_mut(&mut self, id: &str, token: &str) -> Result<&mut Session, TransferError> {
        // Verify first through the shared path, then re-borrow mutably.
        self.authorise(id, token)?;
        Ok(self.sessions.get_mut(id).expect("just authorised"))
    }

    pub fn views(&self) -> Vec<SessionView> {
        let mut out: Vec<SessionView> = self.sessions.values().map(|s| s.view()).collect();
        // Newest first.
        out.sort_by_key(|s| std::cmp::Reverse(s.created_ms));
        out
    }

    pub fn active_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|s| {
                matches!(
                    s.status,
                    SessionStatus::Transferring | SessionStatus::Accepted
                )
            })
            .count()
    }

    /// Drop expired sessions, cancelling anything still running. Returns the
    /// ids that were removed so their `.part` files can be cleaned up.
    pub fn sweep(&mut self) -> Vec<String> {
        let expired: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.is_expired())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            if let Some(s) = self.sessions.get(id) {
                s.control.cancel();
            }
            self.sessions.remove(id);
        }
        expired
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str) -> DeviceInfo {
        DeviceInfo {
            id: format!("id-{name}"),
            name: name.into(),
            os: "linux".into(),
            kind: "desktop".into(),
            version: 1,
            port: 47654,
            tls: false,
            pin_required: false,
        }
    }

    fn session_with(files: Vec<(&str, u64)>) -> Session {
        let files = files
            .into_iter()
            .map(|(p, size)| FileMeta {
                rel_path: p.into(),
                size,
                mime: None,
                modified_ms: None,
            })
            .collect();
        Session::new(
            device("peer"),
            files,
            Direction::Push,
            Role::Receiving,
            ConflictPolicy::KeepBoth,
            std::env::temp_dir(),
        )
    }

    #[test]
    fn tokens_are_long_random_and_distinct() {
        let a = random_token();
        let b = random_token();
        assert_ne!(a, b);
        assert!(a.len() >= 43, "256 bits of base64url is 43 chars");
    }

    #[test]
    fn token_check_rejects_wrong_and_truncated_tokens() {
        let s = session_with(vec![("a.bin", 10)]);
        let good = s.token().to_string();
        assert!(s.token_matches(&good));
        assert!(!s.token_matches(""));
        assert!(!s.token_matches(&good[..good.len() - 1]));
        assert!(!s.token_matches(&format!("{good}x")));

        let mut flipped: Vec<char> = good.chars().collect();
        flipped[0] = if flipped[0] == 'A' { 'B' } else { 'A' };
        assert!(!s.token_matches(&flipped.into_iter().collect::<String>()));
    }

    #[test]
    fn store_authorise_enforces_the_token() {
        let mut store = SessionStore::default();
        let s = session_with(vec![("a.bin", 10)]);
        let token = s.token().to_string();
        let id = store.insert(s);

        assert!(store.authorise(&id, &token).is_ok());
        assert!(matches!(
            store.authorise(&id, "nope"),
            Err(TransferError::InvalidToken(_))
        ));
        assert!(matches!(
            store.authorise("no-such-session", &token),
            Err(TransferError::InvalidToken(_))
        ));
    }

    #[test]
    fn views_never_leak_the_token() {
        let s = session_with(vec![("a.bin", 10)]);
        let token = s.token().to_string();
        let json = serde_json::to_string(&s.view()).unwrap();
        assert!(
            !json.contains(&token),
            "session view must not carry the token"
        );
    }

    #[test]
    fn progress_accumulates_across_files() {
        let mut s = session_with(vec![("a.bin", 100), ("b.bin", 400)]);
        assert_eq!(s.total_bytes, 500);

        s.file_bytes_done = 50;
        assert_eq!(s.bytes_done(), 50);
        assert_eq!(s.remaining(), 450);

        s.complete_current_file(Some(PathBuf::from("/tmp/a.bin")));
        assert_eq!(s.bytes_done(), 100);
        assert_eq!(s.file_index, 1);
        assert_eq!(s.files_done, 1);

        s.file_bytes_done = 400;
        assert_eq!(s.bytes_done(), 500);
        assert_eq!(s.remaining(), 0);
    }

    #[test]
    fn speed_meter_uses_a_sliding_window() {
        let mut m = SpeedMeter::new(Duration::from_secs(2));
        let t0 = Instant::now();
        // 1 MB/s for four seconds; the window should report ~1 MB/s, not an
        // average diluted by the start.
        for i in 0..40 {
            m.record_at(100_000, t0 + Duration::from_millis(i * 100));
        }
        let speed = m.speed_bps_at(t0 + Duration::from_millis(3_900));
        assert!(
            speed > 700_000.0 && speed < 1_300_000.0,
            "speed was {speed}"
        );
        assert_eq!(m.total(), 4_000_000);
    }

    #[test]
    fn speed_meter_is_quiet_before_it_has_data() {
        let m = SpeedMeter::default();
        assert_eq!(m.speed_bps(), 0.0);
        assert_eq!(m.eta_secs(1_000), None);
    }

    #[test]
    fn eta_is_derived_from_windowed_speed() {
        let mut m = SpeedMeter::new(Duration::from_secs(10));
        let t0 = Instant::now();
        for i in 0..10 {
            m.record_at(1_000_000, t0 + Duration::from_millis(i * 100));
        }
        let eta = m
            .eta_secs_at(10_000_000, t0 + Duration::from_millis(900))
            .expect("speed is known");
        assert!(eta > 0.0 && eta < 60.0, "eta was {eta}");
    }

    #[tokio::test]
    async fn control_pause_and_cancel() {
        let c = Arc::new(Control::default());
        assert!(!c.is_cancelled());
        assert!(c.wait_if_paused().await);

        c.set_paused(true);
        let c2 = c.clone();
        let waiter = tokio::spawn(async move { c2.wait_if_paused().await });
        tokio::task::yield_now().await;
        c.set_paused(false);
        assert!(waiter.await.unwrap(), "resuming lets the loop continue");

        c.set_paused(true);
        let c3 = c.clone();
        let waiter = tokio::spawn(async move { c3.wait_if_paused().await });
        tokio::task::yield_now().await;
        c.cancel();
        assert!(!waiter.await.unwrap(), "cancelling wins over pausing");
    }

    #[test]
    fn sweep_removes_stale_pending_offers() {
        let mut store = SessionStore::default();
        let mut s = session_with(vec![("a.bin", 1)]);
        s.created_at = Instant::now() - (OFFER_TTL + Duration::from_secs(1));
        let id = store.insert(s);
        assert_eq!(store.len(), 1);

        let removed = store.sweep();
        assert_eq!(removed, vec![id]);
        assert!(store.is_empty());
    }

    #[test]
    fn display_name_summarises_multi_file_offers() {
        assert_eq!(session_with(vec![("a.bin", 1)]).display_name(), "a.bin");
        assert_eq!(
            session_with(vec![("trip/a.jpg", 1), ("trip/b.jpg", 1)]).display_name(),
            "trip + 1 more"
        );
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::*;
    use crate::model::{ConflictPolicy, DeviceInfo, Direction, FileMeta, Role, PROTOCOL_VERSION};

    fn peer() -> DeviceInfo {
        DeviceInfo {
            id: "peer".into(),
            name: "Peer".into(),
            os: "linux".into(),
            kind: "desktop".into(),
            version: PROTOCOL_VERSION,
            port: 1,
            tls: false,
            pin_required: false,
        }
    }

    fn incoming(root: &std::path::Path, names: &[&str]) -> Session {
        Session::new(
            peer(),
            names
                .iter()
                .map(|n| FileMeta {
                    rel_path: (*n).into(),
                    size: 10,
                    mime: None,
                    modified_ms: None,
                })
                .collect(),
            Direction::Push,
            Role::Receiving,
            ConflictPolicy::KeepBoth,
            root.to_path_buf(),
        )
    }

    #[test]
    fn a_fresh_file_reports_no_conflict() {
        let dir = std::env::temp_dir().join(format!("sl-conf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut s = incoming(&dir, &["ScreenRecording_09-07-2026.mov"]);
        s.detect_conflicts();
        assert!(
            s.conflicts.is_empty(),
            "a file that has never been received must not look like a conflict"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_existing_file_is_reported() {
        let dir = std::env::temp_dir().join(format!("sl-conf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.pdf"), b"old").unwrap();

        let mut s = incoming(&dir, &["report.pdf", "fresh.txt"]);
        s.detect_conflicts();
        assert_eq!(s.conflicts, vec!["report.pdf"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn conflicts_are_detected_inside_folders() {
        let dir = std::env::temp_dir().join(format!("sl-conf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("Trip/day1")).unwrap();
        std::fs::write(dir.join("Trip/day1/a.jpg"), b"old").unwrap();

        let mut s = incoming(&dir, &["Trip/day1/a.jpg", "Trip/day1/b.jpg"]);
        s.detect_conflicts();
        assert_eq!(s.conflicts, vec!["Trip/day1/a.jpg"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn outgoing_sessions_never_report_conflicts() {
        let dir = std::env::temp_dir().join(format!("sl-conf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.pdf"), b"old").unwrap();

        let mut s = Session::new(
            peer(),
            vec![FileMeta {
                rel_path: "report.pdf".into(),
                size: 10,
                mime: None,
                modified_ms: None,
            }],
            Direction::Push,
            Role::Sending,
            ConflictPolicy::KeepBoth,
            dir.clone(),
        );
        s.detect_conflicts();
        assert!(s.conflicts.is_empty(), "what the sender already has is irrelevant");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_hostile_path_is_not_reported_as_a_conflict() {
        let dir = std::env::temp_dir().join(format!("sl-conf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut s = incoming(&dir, &["../../../../etc/passwd"]);
        s.detect_conflicts();
        assert!(
            s.conflicts.is_empty(),
            "a path that cannot be sanitised must not probe the filesystem"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
