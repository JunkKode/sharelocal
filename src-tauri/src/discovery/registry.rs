//! The peer table shared by every discovery backend.
//!
//! mDNS and the UDP fallback both funnel sightings in here; the registry
//! deduplicates by device id, tracks liveness with a TTL, and reports whether
//! anything actually changed so the UI is only woken when it matters.

use crate::model::{DeviceInfo, DiscoverySource, LinkKind, Peer, PeerStatus};
use crate::net;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Seen more recently than this → `Online`.
pub const PEER_TTL: Duration = Duration::from_secs(12);
/// Seen more recently than this → `Stale`; older peers are dropped.
pub const PEER_DROP: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
struct Entry {
    device: DeviceInfo,
    addresses: Vec<IpAddr>,
    source: DiscoverySource,
    last_seen: Instant,
    last_seen_ms: u64,
    rtt_ms: Option<f64>,
    last_speed_bps: Option<f64>,
    /// The status the UI was last told about. Kept so a silent transition
    /// (online → stale) still produces a change notification.
    reported: PeerStatus,
    /// Set for QR/manual peers so a TTL sweep cannot evict them while a
    /// browser client is mid-transfer.
    pinned: bool,
}

#[derive(Debug, Default)]
pub struct PeerRegistry {
    peers: HashMap<String, Entry>,
    self_id: String,
}

impl PeerRegistry {
    pub fn new(self_id: String) -> Self {
        Self {
            peers: HashMap::new(),
            self_id,
        }
    }

    pub fn set_self_id(&mut self, id: String) {
        self.self_id = id;
    }

    /// Record a sighting. Returns true when the visible peer list changed.
    pub fn observe(
        &mut self,
        device: DeviceInfo,
        addresses: Vec<IpAddr>,
        source: DiscoverySource,
    ) -> bool {
        self.observe_at(device, addresses, source, Instant::now())
    }

    pub fn observe_at(
        &mut self,
        device: DeviceInfo,
        mut addresses: Vec<IpAddr>,
        source: DiscoverySource,
        now: Instant,
    ) -> bool {
        // Never list ourselves, however we happen to hear the announcement.
        if device.id == self.self_id || device.id.is_empty() {
            return false;
        }
        // Only keep addresses we would actually be willing to talk to.
        addresses.retain(|ip| net::is_lan_ip(*ip));
        addresses.sort();
        addresses.dedup();
        if addresses.is_empty() {
            return false;
        }

        let pinned = matches!(source, DiscoverySource::Qr | DiscoverySource::Manual);
        match self.peers.get_mut(&device.id) {
            Some(existing) => {
                // Capture the status the UI currently believes *before*
                // refreshing liveness, otherwise a peer coming back from
                // Stale looks like no change at all.
                let was = existing.reported;
                let changed = existing.device != device || existing.addresses != addresses;
                existing.device = device;
                existing.addresses = addresses;
                existing.last_seen = now;
                existing.last_seen_ms = crate::transfer::session::now_ms();
                existing.pinned |= pinned;
                // A peer heard over mDNS is preferred to the UDP fallback for
                // display purposes, but either sighting refreshes liveness.
                if source == DiscoverySource::Mdns || existing.source == DiscoverySource::Udp {
                    existing.source = source;
                }
                let now_status = existing.status_at(now);
                existing.reported = now_status;
                changed || was != now_status
            }
            None => {
                self.peers.insert(
                    device.id.clone(),
                    Entry {
                        device,
                        addresses,
                        source,
                        last_seen: now,
                        last_seen_ms: crate::transfer::session::now_ms(),
                        rtt_ms: None,
                        last_speed_bps: None,
                        reported: PeerStatus::Online,
                        pinned,
                    },
                );
                true
            }
        }
    }

    /// Explicitly drop a peer, e.g. on an mDNS `ServiceRemoved`.
    pub fn forget(&mut self, device_id: &str) -> bool {
        self.peers.remove(device_id).is_some()
    }

    pub fn record_rtt(&mut self, device_id: &str, rtt_ms: f64) {
        if let Some(p) = self.peers.get_mut(device_id) {
            p.rtt_ms = Some(rtt_ms);
        }
    }

    pub fn record_speed(&mut self, device_id: &str, bps: f64) {
        if let Some(p) = self.peers.get_mut(device_id) {
            p.last_speed_bps = Some(bps);
        }
    }

    /// Age out peers. Returns true when the visible list changed.
    pub fn sweep(&mut self) -> bool {
        self.sweep_at(Instant::now())
    }

    pub fn sweep_at(&mut self, now: Instant) -> bool {
        let before = self.peers.len();
        self.peers
            .retain(|_, e| e.pinned || now.duration_since(e.last_seen) <= PEER_DROP);
        let mut changed = before != self.peers.len();

        // Ageing is silent — nothing calls back when a peer stops announcing —
        // so the transition itself has to be detected here.
        for entry in self.peers.values_mut() {
            let current = entry.status_at(now);
            if entry.reported != current {
                entry.reported = current;
                changed = true;
            }
        }
        changed
    }

    pub fn list(&self) -> Vec<Peer> {
        self.list_at(Instant::now())
    }

    pub fn list_at(&self, now: Instant) -> Vec<Peer> {
        let mut out: Vec<Peer> = self.peers.values().filter_map(|e| e.to_peer(now)).collect();
        // Online first, then by name, so the list does not jitter.
        out.sort_by(|a, b| {
            let rank = |s: PeerStatus| match s {
                PeerStatus::Online => 0,
                PeerStatus::Stale => 1,
                PeerStatus::Offline => 2,
            };
            rank(a.status).cmp(&rank(b.status)).then_with(|| {
                a.device
                    .name
                    .to_lowercase()
                    .cmp(&b.device.name.to_lowercase())
            })
        });
        out
    }

    pub fn get(&self, device_id: &str) -> Option<Peer> {
        self.peers
            .get(device_id)
            .and_then(|e| e.to_peer(Instant::now()))
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// True when peers exist but none share a subnet with us — the signature of
    /// AP/client isolation or a routed guest network.
    pub fn isolation_suspected(&self) -> bool {
        !self.peers.is_empty()
            && self
                .peers
                .values()
                .all(|e| !e.addresses.iter().any(|ip| net::same_subnet(*ip)))
    }

    pub fn clear(&mut self) {
        self.peers.clear();
    }
}

impl Entry {
    fn status_at(&self, now: Instant) -> PeerStatus {
        let age = now.duration_since(self.last_seen);
        if age <= PEER_TTL {
            PeerStatus::Online
        } else if age <= PEER_DROP {
            PeerStatus::Stale
        } else {
            PeerStatus::Offline
        }
    }

    fn to_peer(&self, now: Instant) -> Option<Peer> {
        // Prefer an address on one of our own subnets; fall back to the first.
        let primary = self
            .addresses
            .iter()
            .find(|ip| ip.is_ipv4() && net::same_subnet(**ip))
            .or_else(|| self.addresses.iter().find(|ip| ip.is_ipv4()))
            .or_else(|| self.addresses.first())
            .copied()?;
        let same_subnet = net::same_subnet(primary);
        Some(Peer {
            device: self.device.clone(),
            addresses: self.addresses.clone(),
            primary,
            source: self.source,
            status: self.status_at(now),
            same_subnet,
            link: link_for(primary),
            rtt_ms: self.rtt_ms,
            last_speed_bps: self.last_speed_bps,
            last_seen_ms: self.last_seen_ms,
        })
    }
}

/// The link type of the local interface we would reach `ip` through — the best
/// available proxy for "how fast is this connection".
fn link_for(ip: IpAddr) -> LinkKind {
    net::interfaces()
        .into_iter()
        .find(|i| i.contains(ip))
        .map(|i| i.link)
        .unwrap_or(LinkKind::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: format!("Device {id}"),
            os: "linux".into(),
            kind: "desktop".into(),
            version: 1,
            port: 47654,
            tls: false,
            pin_required: false,
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn discovers_and_lists_a_peer() {
        let mut r = PeerRegistry::new("self".into());
        assert!(r.observe(device("a"), vec![ip("192.168.1.5")], DiscoverySource::Mdns));
        assert_eq!(r.len(), 1);

        let peers = r.list();
        assert_eq!(peers[0].device.id, "a");
        assert_eq!(peers[0].status, PeerStatus::Online);
        assert_eq!(peers[0].primary, ip("192.168.1.5"));
    }

    #[test]
    fn never_lists_itself() {
        let mut r = PeerRegistry::new("self".into());
        assert!(!r.observe(
            device("self"),
            vec![ip("192.168.1.5")],
            DiscoverySource::Mdns
        ));
        assert!(r.is_empty());
    }

    #[test]
    fn ignores_public_addresses() {
        let mut r = PeerRegistry::new("self".into());
        assert!(!r.observe(device("a"), vec![ip("8.8.8.8")], DiscoverySource::Udp));
        assert!(
            r.is_empty(),
            "a peer must not be reachable over the internet"
        );
    }

    #[test]
    fn repeat_sightings_do_not_churn_the_ui() {
        let mut r = PeerRegistry::new("self".into());
        let t0 = Instant::now();
        assert!(r.observe_at(device("a"), vec![ip("10.0.0.2")], DiscoverySource::Mdns, t0));
        assert!(
            !r.observe_at(device("a"), vec![ip("10.0.0.2")], DiscoverySource::Mdns, t0),
            "an identical re-announcement is not a change"
        );
    }

    #[test]
    fn a_renamed_or_moved_peer_is_a_change() {
        let mut r = PeerRegistry::new("self".into());
        let t0 = Instant::now();
        r.observe_at(device("a"), vec![ip("10.0.0.2")], DiscoverySource::Mdns, t0);

        let mut renamed = device("a");
        renamed.name = "New Name".into();
        assert!(r.observe_at(renamed, vec![ip("10.0.0.2")], DiscoverySource::Mdns, t0));

        assert!(
            r.observe_at(device("a"), vec![ip("10.0.0.9")], DiscoverySource::Mdns, t0),
            "an IP change must be reported so transfers retarget"
        );
    }

    #[test]
    fn peers_go_stale_then_disappear() {
        let mut r = PeerRegistry::new("self".into());
        let t0 = Instant::now();
        r.observe_at(device("a"), vec![ip("10.0.0.2")], DiscoverySource::Mdns, t0);

        let stale_at = t0 + PEER_TTL + Duration::from_secs(1);
        assert_eq!(r.list_at(stale_at)[0].status, PeerStatus::Stale);
        assert!(r.sweep_at(stale_at), "online → stale is a visible change");
        assert_eq!(r.len(), 1, "a stale peer is still listed");

        let gone_at = t0 + PEER_DROP + Duration::from_secs(1);
        assert!(r.sweep_at(gone_at));
        assert!(
            r.is_empty(),
            "the peer is dropped once it is well past its TTL"
        );
    }

    #[test]
    fn a_returning_peer_comes_back_online() {
        let mut r = PeerRegistry::new("self".into());
        let t0 = Instant::now();
        r.observe_at(device("a"), vec![ip("10.0.0.2")], DiscoverySource::Mdns, t0);

        let later = t0 + PEER_TTL + Duration::from_secs(2);
        // The reaper ticks every second in the running app, so by the time a
        // peer has gone quiet the registry has already reported it as stale.
        assert!(r.sweep_at(later), "the sweep notices the peer went quiet");
        assert_eq!(r.list_at(later)[0].status, PeerStatus::Stale);
        assert!(
            r.observe_at(
                device("a"),
                vec![ip("10.0.0.2")],
                DiscoverySource::Mdns,
                later
            ),
            "coming back from stale is a change even if nothing else moved"
        );
        assert_eq!(r.list_at(later)[0].status, PeerStatus::Online);
    }

    #[test]
    fn explicit_removal_works() {
        let mut r = PeerRegistry::new("self".into());
        r.observe(device("a"), vec![ip("10.0.0.2")], DiscoverySource::Mdns);
        assert!(r.forget("a"));
        assert!(!r.forget("a"));
        assert!(r.is_empty());
    }

    #[test]
    fn qr_peers_are_pinned_against_the_ttl_sweep() {
        let mut r = PeerRegistry::new("self".into());
        let t0 = Instant::now();
        r.observe_at(
            device("phone"),
            vec![ip("192.168.1.44")],
            DiscoverySource::Qr,
            t0,
        );

        let much_later = t0 + PEER_DROP * 10;
        r.sweep_at(much_later);
        assert_eq!(
            r.len(),
            1,
            "a browser client mid-transfer must not be evicted"
        );
    }

    #[test]
    fn mdns_sighting_upgrades_a_udp_discovered_peer() {
        let mut r = PeerRegistry::new("self".into());
        let t0 = Instant::now();
        r.observe_at(device("a"), vec![ip("10.0.0.2")], DiscoverySource::Udp, t0);
        assert_eq!(r.list()[0].source, DiscoverySource::Udp);

        r.observe_at(device("a"), vec![ip("10.0.0.2")], DiscoverySource::Mdns, t0);
        assert_eq!(r.list()[0].source, DiscoverySource::Mdns);
    }

    #[test]
    fn addresses_are_deduplicated_and_sorted() {
        let mut r = PeerRegistry::new("self".into());
        r.observe(
            device("a"),
            vec![ip("10.0.0.5"), ip("10.0.0.2"), ip("10.0.0.5")],
            DiscoverySource::Mdns,
        );
        assert_eq!(r.list()[0].addresses, vec![ip("10.0.0.2"), ip("10.0.0.5")]);
    }

    #[test]
    fn ipv4_is_preferred_as_the_primary_address() {
        let mut r = PeerRegistry::new("self".into());
        r.observe(
            device("a"),
            vec![ip("fd00::5"), ip("192.168.1.7")],
            DiscoverySource::Mdns,
        );
        assert_eq!(r.list()[0].primary, ip("192.168.1.7"));
    }

    #[test]
    fn online_peers_sort_above_stale_ones() {
        let mut r = PeerRegistry::new("self".into());
        let t0 = Instant::now();
        r.observe_at(
            device("zzz"),
            vec![ip("10.0.0.3")],
            DiscoverySource::Mdns,
            t0,
        );
        r.observe_at(
            device("aaa"),
            vec![ip("10.0.0.4")],
            DiscoverySource::Mdns,
            t0 - PEER_TTL - Duration::from_secs(1),
        );
        let listed = r.list_at(t0);
        assert_eq!(listed[0].device.id, "zzz", "online peer first");
        assert_eq!(listed[1].status, PeerStatus::Stale);
    }

    #[test]
    fn peer_urls_are_built_for_both_families() {
        let mut r = PeerRegistry::new("self".into());
        r.observe(device("a"), vec![ip("192.168.1.7")], DiscoverySource::Mdns);
        let p = r.list().remove(0);
        assert_eq!(p.base_url(), "http://192.168.1.7:47654");
        assert_eq!(p.ws_url(), "ws://192.168.1.7:47654/ws");

        let mut tls_device = device("b");
        tls_device.tls = true;
        r.observe(tls_device, vec![ip("fd00::9")], DiscoverySource::Mdns);
        let p6 = r.get("b").unwrap();
        assert_eq!(p6.base_url(), "https://[fd00::9]:47654");
    }
}
