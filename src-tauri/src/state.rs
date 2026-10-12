//! `AppState` — the one thing every subsystem shares.

use crate::config::Settings;
use crate::discovery::registry::PeerRegistry;
use crate::events::{AppEvent, EventBus};
use crate::history::{history_path, History};
use crate::model::{DeviceInfo, LinkKind, NetworkStatus, PROTOCOL_VERSION};
use crate::net;
use crate::transfer::session::{random_token, SessionStore};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, RwLock};

/// How long a QR pairing token stays valid.
pub const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);
/// A browser client is forgotten after this long without a request.
pub const WEB_CLIENT_TTL: Duration = Duration::from_secs(60 * 30);

#[derive(Debug, Clone)]
pub struct PairingToken {
    pub token: String,
    pub created: Instant,
}

impl PairingToken {
    pub fn new() -> Self {
        Self {
            token: random_token(),
            created: Instant::now(),
        }
    }
    pub fn is_valid(&self) -> bool {
        self.created.elapsed() < PAIRING_TTL
    }
}

impl Default for PairingToken {
    fn default() -> Self {
        Self::new()
    }
}

/// A phone or laptop that joined through the QR code instead of mDNS.
#[derive(Debug, Clone)]
pub struct WebClient {
    pub id: String,
    pub token: String,
    pub device: DeviceInfo,
    pub addr: IpAddr,
    pub last_seen: Instant,
}

impl WebClient {
    pub fn token_matches(&self, candidate: &str) -> bool {
        use subtle::ConstantTimeEq;
        if candidate.len() != self.token.len() {
            return false;
        }
        self.token.as_bytes().ct_eq(candidate.as_bytes()).into()
    }
    pub fn is_expired(&self) -> bool {
        self.last_seen.elapsed() > WEB_CLIENT_TTL
    }
}

pub struct AppState {
    pub settings: RwLock<Settings>,
    pub settings_path: PathBuf,
    pub data_dir: PathBuf,

    pub peers: RwLock<PeerRegistry>,
    pub sessions: RwLock<SessionStore>,
    pub history: Mutex<History>,
    pub events: EventBus,

    /// The port the HTTP server actually bound (may differ from the requested
    /// one if it was taken).
    pub bound_port: AtomicU16,
    pub server_ready: AtomicBool,
    /// Tray toggle. When off we stop advertising and refuse incoming offers,
    /// without tearing the server down.
    pub sharing_enabled: AtomicBool,
    pub mdns_ok: AtomicBool,
    pub udp_ok: AtomicBool,
    /// Pulsed when the port or TLS flag changes and the listener must be
    /// rebuilt. Signal it through [`AppState::request_restart`].
    pub restart_server: Notify,
    /// Pulsed when our IP addresses change, so discovery re-announces.
    pub network_changed: Notify,

    pub pairing: RwLock<Option<PairingToken>>,
    pub web_clients: RwLock<HashMap<String, WebClient>>,

    /// LAN-only HTTP client used for outbound transfers.
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Arc<Self> {
        let settings_path = data_dir.join("settings.json");
        let mut settings = Settings::load(&settings_path);
        settings.normalise();
        // Persist immediately so a first run has a real device id on disk.
        let _ = settings.save(&settings_path);

        let port = settings.port;
        let self_id = settings.device_id.clone();
        let history = History::load(history_path(&data_dir));

        Arc::new(Self {
            settings: RwLock::new(settings),
            settings_path,
            data_dir,
            peers: RwLock::new(PeerRegistry::new(self_id)),
            sessions: RwLock::new(SessionStore::default()),
            history: Mutex::new(history),
            events: EventBus::default(),
            bound_port: AtomicU16::new(port),
            server_ready: AtomicBool::new(false),
            sharing_enabled: AtomicBool::new(true),
            mdns_ok: AtomicBool::new(false),
            udp_ok: AtomicBool::new(false),
            restart_server: Notify::new(),
            network_changed: Notify::new(),
            pairing: RwLock::new(None),
            web_clients: RwLock::new(HashMap::new()),
            http: build_http_client(),
        })
    }

    /// Ask the HTTP server to rebind (port or TLS changed).
    ///
    /// `notify_one` rather than `notify_waiters` so a request that arrives
    /// before the server starts waiting is still honoured — otherwise a
    /// settings change made right at startup would be silently dropped.
    pub fn request_restart(&self) {
        self.restart_server.notify_one();
    }

    pub fn sharing(&self) -> bool {
        self.sharing_enabled.load(Ordering::SeqCst)
    }

    pub fn set_sharing(&self, on: bool) {
        self.sharing_enabled.store(on, Ordering::SeqCst);
        self.network_changed.notify_waiters();
    }

    pub fn port(&self) -> u16 {
        self.bound_port.load(Ordering::SeqCst)
    }

    /// What we advertise about ourselves.
    pub async fn device_info(&self) -> DeviceInfo {
        let s = self.settings.read().await;
        DeviceInfo {
            id: s.device_id.clone(),
            name: s.device_name.clone(),
            os: Settings::os_name().into(),
            kind: Settings::device_kind().into(),
            version: PROTOCOL_VERSION,
            port: self.port(),
            tls: s.tls,
            pin_required: s.pin_required(),
        }
    }

    pub async fn save_settings(&self) -> std::io::Result<()> {
        let s = self.settings.read().await;
        s.save(&self.settings_path)
    }

    pub async fn download_dir(&self) -> PathBuf {
        self.settings.read().await.download_dir.clone()
    }

    pub fn publish(&self, event: AppEvent) {
        self.events.publish(event);
    }

    /// Push the current peer list to the UI.
    pub async fn publish_peers(&self) {
        let peers = self.peers.read().await.list();
        self.publish(AppEvent::PeersChanged { peers });
    }

    /// Push one session's state to the UI and to any interested WS client.
    pub async fn publish_session(&self, id: &str) {
        let view = self.sessions.read().await.get(id).map(|s| s.view());
        if let Some(session) = view {
            self.publish(AppEvent::SessionUpdated { session });
        }
    }

    pub async fn network_status(&self) -> NetworkStatus {
        let settings = self.settings.read().await;
        let iface = net::primary_interface(settings.preferred_interface.as_deref());
        let ifaces = net::usable_interfaces();
        let ipv6 = ifaces
            .iter()
            .find(|i| i.ip.is_ipv6())
            .map(|i| i.ip.to_string());
        let isolation = self.peers.read().await.isolation_suspected();

        NetworkStatus {
            online: iface.is_some(),
            interface: iface.as_ref().map(|i| i.name.clone()),
            ipv4: iface
                .as_ref()
                .filter(|i| i.ip.is_ipv4())
                .map(|i| i.ip.to_string())
                .or_else(|| {
                    ifaces
                        .iter()
                        .find(|i| i.ip.is_ipv4())
                        .map(|i| i.ip.to_string())
                }),
            ipv6,
            link: iface.as_ref().map(|i| i.link).unwrap_or(LinkKind::Unknown),
            port: self.port(),
            tls: settings.tls,
            mdns_ok: self.mdns_ok.load(Ordering::SeqCst),
            udp_ok: self.udp_ok.load(Ordering::SeqCst),
            isolation_suspected: isolation,
        }
    }

    pub async fn publish_network(&self) {
        let network = self.network_status().await;
        self.publish(AppEvent::NetworkChanged { network });
    }

    /// Mint (or refresh) the QR pairing token and return the URL to encode.
    pub async fn issue_pairing(&self) -> (String, String) {
        let token = PairingToken::new();
        let value = token.token.clone();
        *self.pairing.write().await = Some(token);

        let settings = self.settings.read().await;
        let scheme = if settings.tls { "https" } else { "http" };
        let host = net::primary_interface(settings.preferred_interface.as_deref())
            .map(|i| match i.ip {
                IpAddr::V6(v6) => format!("[{v6}]"),
                IpAddr::V4(v4) => v4.to_string(),
            })
            .unwrap_or_else(|| "127.0.0.1".into());
        let url = format!(
            "{scheme}://{host}:{}/?t={}",
            self.port(),
            urlencoding::encode(&value)
        );
        (url, value)
    }

    pub async fn revoke_pairing(&self) {
        *self.pairing.write().await = None;
    }

    /// Redeem a pairing token, registering the browser as a pinned peer.
    pub async fn redeem_pairing(
        &self,
        pairing_token: &str,
        device: DeviceInfo,
        addr: IpAddr,
    ) -> Option<WebClient> {
        {
            let guard = self.pairing.read().await;
            let current = guard.as_ref()?;
            if !current.is_valid() {
                return None;
            }
            use subtle::ConstantTimeEq;
            if current.token.len() != pairing_token.len() {
                return None;
            }
            let ok: bool = current
                .token
                .as_bytes()
                .ct_eq(pairing_token.as_bytes())
                .into();
            if !ok {
                return None;
            }
        }

        let client = WebClient {
            id: device.id.clone(),
            token: random_token(),
            device: device.clone(),
            addr,
            last_seen: Instant::now(),
        };
        self.web_clients
            .write()
            .await
            .insert(client.id.clone(), client.clone());
        let changed =
            self.peers
                .write()
                .await
                .observe(device, vec![addr], crate::model::DiscoverySource::Qr);
        if changed {
            self.publish_peers().await;
        }
        Some(client)
    }

    /// Authenticate a browser client by its bearer token.
    pub async fn authenticate_web(&self, client_id: &str, token: &str) -> Option<WebClient> {
        let mut clients = self.web_clients.write().await;
        let client = clients.get_mut(client_id)?;
        if client.is_expired() || !client.token_matches(token) {
            return None;
        }
        client.last_seen = Instant::now();
        Some(client.clone())
    }

    /// A paired browser is still connected: keep it listed as online.
    ///
    /// Desktop peers stay fresh by re-announcing over mDNS/UDP; a browser
    /// cannot announce, so its open control connection stands in for that.
    /// Without this a phone showed as offline 30 s after scanning the code,
    /// however long its page stayed open.
    pub async fn touch_web_client(&self, client_id: &str) {
        let device = {
            let mut clients = self.web_clients.write().await;
            let Some(client) = clients.get_mut(client_id) else { return };
            client.last_seen = Instant::now();
            (client.device.clone(), client.addr)
        };
        let changed = self
            .peers
            .write()
            .await
            .observe(device.0, vec![device.1], crate::model::DiscoverySource::Qr);
        if changed {
            self.publish_peers().await;
        }
    }

    pub async fn forget_web_client(&self, client_id: &str) {
        self.web_clients.write().await.remove(client_id);
        let changed = self.peers.write().await.forget(client_id);
        if changed {
            self.publish_peers().await;
        }
    }
}

/// The outbound client. Deliberately: no proxies (a corporate proxy must never
/// see LAN traffic), generous timeouts for slow first bytes but no overall
/// timeout, and self-signed certificates accepted because the only TLS peers
/// are LAN devices presenting their own generated certificate.
fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(6))
        .pool_idle_timeout(Duration::from_secs(30))
        .tcp_nodelay(true)
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_state() -> (Arc<AppState>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("sl-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (AppState::new(dir.clone()), dir)
    }

    #[tokio::test]
    async fn device_info_reflects_settings() {
        let (state, dir) = temp_state();
        {
            let mut s = state.settings.write().await;
            s.device_name = "Studio Mac".into();
            s.pin = Some("1234".into());
        }
        let info = state.device_info().await;
        assert_eq!(info.name, "Studio Mac");
        assert!(info.pin_required);
        assert_eq!(info.version, PROTOCOL_VERSION);
        // The PIN itself is never part of the advertised identity.
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("1234"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn pairing_token_round_trip() {
        let (state, dir) = temp_state();
        let (url, token) = state.issue_pairing().await;
        assert!(url.contains("/?t="));

        let device = DeviceInfo {
            id: "phone-1".into(),
            name: "iPhone".into(),
            os: "ios".into(),
            kind: "phone".into(),
            version: PROTOCOL_VERSION,
            port: 0,
            tls: false,
            pin_required: false,
        };
        let addr: IpAddr = "192.168.1.44".parse().unwrap();

        assert!(state
            .redeem_pairing("wrong-token", device.clone(), addr)
            .await
            .is_none());

        let client = state
            .redeem_pairing(&token, device.clone(), addr)
            .await
            .expect("valid token is redeemed");
        assert_eq!(client.id, "phone-1");
        assert_eq!(state.peers.read().await.len(), 1);

        assert!(state
            .authenticate_web("phone-1", &client.token)
            .await
            .is_some());
        assert!(state.authenticate_web("phone-1", "nope").await.is_none());
        assert!(state
            .authenticate_web("ghost", &client.token)
            .await
            .is_none());

        state.forget_web_client("phone-1").await;
        assert!(state
            .authenticate_web("phone-1", &client.token)
            .await
            .is_none());
        assert!(state.peers.read().await.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn expired_pairing_token_is_refused() {
        let (state, dir) = temp_state();
        let (_url, token) = state.issue_pairing().await;
        {
            let mut guard = state.pairing.write().await;
            if let Some(p) = guard.as_mut() {
                p.created = Instant::now() - (PAIRING_TTL + Duration::from_secs(1));
            }
        }
        let device = DeviceInfo {
            id: "phone-2".into(),
            name: "Android".into(),
            os: "android".into(),
            kind: "phone".into(),
            version: PROTOCOL_VERSION,
            port: 0,
            tls: false,
            pin_required: false,
        };
        assert!(state
            .redeem_pairing(&token, device, "10.0.0.9".parse().unwrap())
            .await
            .is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn revoking_pairing_blocks_new_joins() {
        let (state, dir) = temp_state();
        let (_url, token) = state.issue_pairing().await;
        state.revoke_pairing().await;
        let device = DeviceInfo {
            id: "phone-3".into(),
            name: "Pixel".into(),
            os: "android".into(),
            kind: "phone".into(),
            version: PROTOCOL_VERSION,
            port: 0,
            tls: false,
            pin_required: false,
        };
        assert!(state
            .redeem_pairing(&token, device, "10.0.0.9".parse().unwrap())
            .await
            .is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn settings_persist_across_restarts() {
        let dir = std::env::temp_dir().join(format!("sl-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let first = AppState::new(dir.clone());
        let id = first.settings.read().await.device_id.clone();
        first.settings.write().await.device_name = "Persisted".into();
        first.save_settings().await.unwrap();
        drop(first);

        let second = AppState::new(dir.clone());
        assert_eq!(second.settings.read().await.device_name, "Persisted");
        assert_eq!(
            second.settings.read().await.device_id,
            id,
            "device identity must be stable across restarts"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
