//! Shared harness for the end-to-end tests: two real ShareLocal instances in
//! one process, each with its own settings, download folder and HTTP server,
//! talking to each other over loopback exactly as two machines would over a
//! LAN.

#![allow(dead_code)]

use sharelocal::model::{
    ConflictPolicy, DeviceInfo, DiscoverySource, Peer, PeerStatus, SessionStatus, SessionView,
    PROTOCOL_VERSION,
};
use sharelocal::state::AppState;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Node {
    pub state: Arc<AppState>,
    pub dir: PathBuf,
    pub downloads: PathBuf,
    pub port: u16,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Reserve a free port by binding and immediately releasing it. Good enough
/// for tests; the server's own fallback covers the rare race.
async fn free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Boot a node with its HTTP server running. Discovery is left out so the
/// tests are deterministic and do not depend on the sandbox allowing mDNS.
pub async fn start_node(name: &str, auto_accept: bool) -> Node {
    let dir = std::env::temp_dir().join(format!("sl-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let downloads = dir.join("Downloads");
    std::fs::create_dir_all(&downloads).unwrap();

    let port = free_port().await;
    let state = AppState::new(dir.clone());
    {
        let mut settings = state.settings.write().await;
        settings.device_name = name.to_string();
        settings.download_dir = downloads.clone();
        settings.auto_accept = auto_accept;
        settings.port = port;
        settings.discovery_mdns = false;
        settings.discovery_udp = false;
    }

    let server_state = state.clone();
    tokio::spawn(async move { sharelocal::server::run(server_state).await });

    // Wait for the listener to be up before handing the node to a test.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state.server_ready.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "{name} server never became ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    Node { state, dir, downloads, port: 0 }.with_actual_port().await
}

impl Node {
    async fn with_actual_port(mut self) -> Self {
        self.port = self.state.port();
        self
    }

    /// A `Peer` record pointing at this node, as another device would hold it.
    pub async fn as_peer(&self) -> Peer {
        let device = self.state.device_info().await;
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        Peer {
            device,
            addresses: vec![ip],
            primary: ip,
            source: DiscoverySource::Manual,
            status: PeerStatus::Online,
            same_subnet: true,
            link: sharelocal::model::LinkKind::Ethernet,
            rtt_ms: None,
            last_speed_bps: None,
            last_seen_ms: 0,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/ws", self.port)
    }

    pub async fn sessions(&self) -> Vec<SessionView> {
        self.state.sessions.read().await.views()
    }

    pub async fn session(&self, id: &str) -> Option<SessionView> {
        self.state.sessions.read().await.get(id).map(|s| s.view())
    }

    /// Accept the first offer this node is waiting on, as the user would.
    pub async fn accept_pending(&self, conflict: ConflictPolicy) -> Option<String> {
        let mut sessions = self.state.sessions.write().await;
        let id = sessions
            .views()
            .into_iter()
            .find(|s| s.status == SessionStatus::Pending && s.role == sharelocal::model::Role::Receiving)
            .map(|s| s.id)?;
        if let Some(session) = sessions.get_mut(&id) {
            session.conflict = conflict;
            session.set_status(SessionStatus::Accepted);
        }
        Some(id)
    }

    pub async fn reject_pending(&self) -> Option<String> {
        let id = {
            let sessions = self.state.sessions.read().await;
            sessions
                .views()
                .into_iter()
                .find(|s| s.status == SessionStatus::Pending)
                .map(|s| s.id)?
        };
        sharelocal::transfer::conclude(&self.state, &id, SessionStatus::Rejected, None).await;
        Some(id)
    }

    /// Wait until this node has an offer awaiting an answer, then accept it.
    pub async fn auto_answer(&self, accept: bool, conflict: ConflictPolicy) {
        let state = self.state.clone();
        tokio::spawn(async move {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let pending = {
                    let sessions = state.sessions.read().await;
                    sessions
                        .views()
                        .into_iter()
                        .find(|s| {
                            s.status == SessionStatus::Pending
                                && s.role == sharelocal::model::Role::Receiving
                        })
                        .map(|s| s.id)
                };
                if let Some(id) = pending {
                    if accept {
                        let mut sessions = state.sessions.write().await;
                        if let Some(session) = sessions.get_mut(&id) {
                            session.conflict = conflict;
                            session.set_status(SessionStatus::Accepted);
                        }
                    } else {
                        sharelocal::transfer::conclude(&state, &id, SessionStatus::Rejected, None)
                            .await;
                    }
                    return;
                }
                if Instant::now() > deadline {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        });
    }
}

/// Poll until a session reaches a terminal state, or fail with what it was
/// last seen doing — far more useful than a bare timeout.
pub async fn await_terminal(node: &Node, session_id: &str, timeout: Duration) -> SessionView {
    let deadline = Instant::now() + timeout;
    let mut last: Option<SessionView> = None;
    loop {
        if let Some(view) = node.session(session_id).await {
            if view.status.is_terminal() {
                return view;
            }
            last = Some(view);
        }
        if Instant::now() > deadline {
            panic!(
                "session {session_id} did not finish in {timeout:?}; last state: {:?}",
                last.map(|v| (v.status, v.progress.bytes_done, v.error))
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn await_status(
    node: &Node,
    session_id: &str,
    want: SessionStatus,
    timeout: Duration,
) -> SessionView {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(view) = node.session(session_id).await {
            if view.status == want {
                return view;
            }
        }
        assert!(
            Instant::now() < deadline,
            "session {session_id} never reached {want:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Deterministic pseudo-random bytes, so a large fixture costs no entropy and
/// content can be verified exactly.
pub fn make_file(path: &Path, size: usize) -> Vec<u8> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut data = vec![0u8; size];
    let mut x: u32 = 0x1234_5678;
    for byte in data.iter_mut() {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *byte = (x >> 24) as u8;
    }
    std::fs::write(path, &data).unwrap();
    data
}

pub fn device(id: &str, name: &str) -> DeviceInfo {
    DeviceInfo {
        id: id.into(),
        name: name.into(),
        os: "linux".into(),
        kind: "desktop".into(),
        version: PROTOCOL_VERSION,
        port: 1,
        tls: false,
        pin_required: false,
    }
}
