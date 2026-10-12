//! Wire + UI types shared by the Rust core, the Tauri UI and the browser client.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;

pub const PROTOCOL_VERSION: u32 = 1;
pub const SERVICE_TYPE: &str = "_sharelocal._tcp.local.";

/// Identity a device advertises about itself. Contains no filesystem paths and
/// nothing secret — it is broadcast over mDNS and handed to any LAN peer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    /// `macos` | `windows` | `linux` | `ios` | `android` | `browser`
    pub os: String,
    /// `laptop` | `desktop` | `phone` | `tablet` | `browser`
    pub kind: String,
    pub version: u32,
    pub port: u16,
    pub tls: bool,
    pub pin_required: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    Wifi,
    Ethernet,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiscoverySource {
    Mdns,
    Udp,
    Qr,
    Manual,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum PeerStatus {
    Online,
    Stale,
    Offline,
}

/// A discovered device plus everything the UI needs to render a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Peer {
    #[serde(flatten)]
    pub device: DeviceInfo,
    pub addresses: Vec<IpAddr>,
    pub primary: IpAddr,
    pub source: DiscoverySource,
    pub status: PeerStatus,
    /// False when the peer's address is not inside any of our interface
    /// subnets — usually a routed VLAN or an AP-isolation situation.
    pub same_subnet: bool,
    pub link: LinkKind,
    /// Last measured control-channel round trip, milliseconds.
    pub rtt_ms: Option<f64>,
    /// Throughput observed on the most recent transfer with this peer, bytes/s.
    pub last_speed_bps: Option<f64>,
    pub last_seen_ms: u64,
}

impl Peer {
    pub fn base_url(&self) -> String {
        let scheme = if self.device.tls { "https" } else { "http" };
        match self.primary {
            IpAddr::V6(v6) => format!("{scheme}://[{v6}]:{}", self.device.port),
            IpAddr::V4(v4) => format!("{scheme}://{v4}:{}", self.device.port),
        }
    }

    pub fn ws_url(&self) -> String {
        let scheme = if self.device.tls { "wss" } else { "ws" };
        match self.primary {
            IpAddr::V6(v6) => format!("{scheme}://[{v6}]:{}/ws", self.device.port),
            IpAddr::V4(v4) => format!("{scheme}://{v4}:{}/ws", self.device.port),
        }
    }
}

/// One file inside an offer. `rel_path` is a *relative* POSIX-style path so
/// folder structure survives the hop; it is sanitised on the receiving side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    pub rel_path: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// Optional last-modified, milliseconds since epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_ms: Option<u64>,
}

/// What a transfer carries.
///
/// Text is a separate kind rather than "a small file" because the receiving
/// side treats it completely differently: it is shown inline and copied to the
/// clipboard, not opened in an application.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PayloadKind {
    #[default]
    Files,
    Text,
}

/// The largest text snippet that may travel inside a control frame. Text this
/// small is sent with the offer instead of over the data plane; anything
/// larger is a file, and should be sent as one.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// The offering side pushes bytes with `PUT`.
    Push,
    /// The offering side exposes bytes for the peer to `GET`.
    Pull,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Sending,
    Receiving,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    /// Rename to `name (2).ext`.
    #[default]
    KeepBoth,
    Replace,
    Cancel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionStatus {
    Pending,
    Accepted,
    Rejected,
    Transferring,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

impl SessionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            SessionStatus::Completed
                | SessionStatus::Cancelled
                | SessionStatus::Failed
                | SessionStatus::Rejected
        )
    }
}

/// Live progress numbers. Recomputed on the fly; never persisted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub file_index: usize,
    pub files_done: usize,
    pub bytes_done: u64,
    pub file_bytes_done: u64,
    pub speed_bps: f64,
    pub eta_secs: Option<f64>,
}

/// The projection of a session that the UI and remote peers are allowed to see.
/// Notably it never carries the session token or absolute local paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub peer: DeviceInfo,
    pub files: Vec<FileMeta>,
    pub total_bytes: u64,
    pub direction: Direction,
    pub role: Role,
    pub status: SessionStatus,
    pub progress: Progress,
    pub created_ms: u64,
    #[serde(default)]
    pub kind: PayloadKind,
    /// The shared text, for `PayloadKind::Text` transfers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TransferError>,
    /// Only populated locally (never sent to a peer): where files landed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved_paths: Vec<PathBuf>,
    /// Incoming offers only: the `rel_path`s that already exist in the
    /// download folder. Empty means nothing will be overwritten, so the UI has
    /// no reason to ask about conflicts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
}

/// Machine-readable failure reasons so the UI can show a specific message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "code", content = "detail")]
pub enum TransferError {
    DeviceOffline(String),
    ConnectionRefused(String),
    NetworkChanged(String),
    Interrupted(String),
    NotEnoughSpace(String),
    PermissionDenied(String),
    FileMissing(String),
    DuplicateName(String),
    InvalidToken(String),
    InvalidPath(String),
    PinRejected(String),
    Rejected(String),
    ClientIsolation(String),
    Other(String),
}

impl TransferError {
    pub fn message(&self) -> &str {
        use TransferError::*;
        match self {
            DeviceOffline(m) | ConnectionRefused(m) | NetworkChanged(m) | Interrupted(m)
            | NotEnoughSpace(m) | PermissionDenied(m) | FileMissing(m) | DuplicateName(m)
            | InvalidToken(m) | InvalidPath(m) | PinRejected(m) | Rejected(m)
            | ClientIsolation(m) | Other(m) => m,
        }
    }
}

/// Snapshot of the local network situation shown in the header and Settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkStatus {
    pub online: bool,
    pub interface: Option<String>,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub link: LinkKind,
    pub port: u16,
    pub tls: bool,
    pub mdns_ok: bool,
    pub udp_ok: bool,
    /// Set when peers are visible but unreachable — the classic AP-isolation
    /// symptom — so the UI can explain it instead of silently failing.
    pub isolation_suspected: bool,
}

/// A completed transfer, stored in `history.json`. File *contents* are never
/// recorded — only metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub at_ms: u64,
    pub peer_name: String,
    pub peer_id: String,
    pub role: Role,
    pub file_count: usize,
    pub display_name: String,
    pub total_bytes: u64,
    pub duration_ms: u64,
    pub avg_speed_bps: f64,
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved_paths: Vec<PathBuf>,
    #[serde(default)]
    pub kind: PayloadKind,
    /// Shared text is short and the whole point is to read it again later, so
    /// unlike file contents it is kept. File *contents* are still never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
