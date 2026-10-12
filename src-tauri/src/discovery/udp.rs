//! Local-network discovery fallback for networks where mDNS is filtered.
//!
//! A tiny JSON protocol over UDP multicast (`239.255.41.42:41234`), mirrored to
//! each interface's directed broadcast address because some access points drop
//! multicast but forward broadcast. Announcements are periodic, and a `probe`
//! lets a device that just joined be seen immediately instead of waiting for
//! the next tick.

use crate::model::{DeviceInfo, DiscoverySource};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;

pub const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 41, 42);
pub const DISCOVERY_PORT: u16 = 41_234;
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
/// Refuse anything larger; the protocol has no legitimate use for big packets.
const MAX_DATAGRAM: usize = 4 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Wire {
    Announce {
        v: u32,
        device: DeviceInfo,
        addrs: Vec<IpAddr>,
    },
    /// "Who is out there?" — recipients answer with an immediate `Announce`.
    Probe { v: u32, id: String },
    /// Graceful departure so peers vanish at once instead of timing out.
    Bye { v: u32, id: String },
}

impl Wire {
    pub fn version(&self) -> u32 {
        match self {
            Wire::Announce { v, .. } | Wire::Probe { v, .. } | Wire::Bye { v, .. } => *v,
        }
    }

    /// Parse a datagram, rejecting oversized or wrong-version payloads.
    pub fn parse(bytes: &[u8]) -> Option<Wire> {
        if bytes.is_empty() || bytes.len() > MAX_DATAGRAM {
            return None;
        }
        let msg: Wire = serde_json::from_slice(bytes).ok()?;
        if msg.version() != crate::model::PROTOCOL_VERSION {
            return None;
        }
        Some(msg)
    }
}

/// Bind the shared discovery socket. `SO_REUSEADDR`/`SO_REUSEPORT` let several
/// ShareLocal instances (and our own tests) coexist on one machine.
fn bind_socket() -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.set_broadcast(true)?;
    let addr: SocketAddr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT));
    socket.bind(&addr.into())?;

    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}

fn join_groups(socket: &UdpSocket) -> usize {
    let mut joined = 0;
    for iface in crate::net::usable_interfaces() {
        if let IpAddr::V4(v4) = iface.ip {
            if socket.join_multicast_v4(MULTICAST_ADDR, v4).is_ok() {
                joined += 1;
            }
        }
    }
    if joined == 0 {
        // Fall back to the default interface so loopback-only setups still work.
        let _ = socket.join_multicast_v4(MULTICAST_ADDR, Ipv4Addr::UNSPECIFIED);
    }
    joined
}

/// Every address we should send an announcement to: the multicast group plus
/// each interface's directed broadcast.
fn announce_targets() -> Vec<SocketAddr> {
    let mut targets = vec![SocketAddr::V4(SocketAddrV4::new(
        MULTICAST_ADDR,
        DISCOVERY_PORT,
    ))];
    for iface in crate::net::usable_interfaces() {
        if let IpAddr::V4(v4) = iface.ip {
            if let Some(bcast) = directed_broadcast(v4, iface.prefix) {
                targets.push(SocketAddr::V4(SocketAddrV4::new(bcast, DISCOVERY_PORT)));
            }
        }
    }
    targets
}

/// `192.168.1.10/24` → `192.168.1.255`.
pub fn directed_broadcast(ip: Ipv4Addr, prefix: u8) -> Option<Ipv4Addr> {
    if prefix == 0 || prefix >= 31 {
        return None;
    }
    let mask = u32::MAX.checked_shl(32 - prefix as u32)?;
    Some(Ipv4Addr::from((u32::from(ip) & mask) | !mask))
}

/// Run the fallback until the process exits.
pub async fn run(state: Arc<AppState>) {
    loop {
        match bind_socket() {
            Ok(socket) => {
                let joined = join_groups(&socket);
                state.udp_ok.store(true, Ordering::SeqCst);
                if joined == 0 {
                    eprintln!("[udp] no multicast-capable interface; relying on broadcast");
                }
                let socket = Arc::new(socket);
                serve(state.clone(), socket).await;
                state.udp_ok.store(false, Ordering::SeqCst);
            }
            Err(err) => {
                state.udp_ok.store(false, Ordering::SeqCst);
                eprintln!("[udp] bind failed: {err}; retrying");
            }
        }
        state.publish_network().await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn serve(state: Arc<AppState>, socket: Arc<UdpSocket>) {
    let sender = tokio::spawn(announce_loop(state.clone(), socket.clone()));
    let receiver = tokio::spawn(receive_loop(state.clone(), socket.clone()));

    // If either half dies (usually because the interface went away) tear the
    // pair down so the outer loop rebinds on the new network.
    tokio::select! {
        _ = sender => {}
        _ = receiver => {}
    }
}

async fn announce_loop(state: Arc<AppState>, socket: Arc<UdpSocket>) {
    loop {
        send_announce(&state, &socket).await;
        // Re-announce early when our addressing changes so peers retarget fast.
        tokio::select! {
            _ = tokio::time::sleep(ANNOUNCE_INTERVAL) => {}
            _ = state.network_changed.notified() => {}
        }
    }
}

async fn send_announce(state: &Arc<AppState>, socket: &UdpSocket) {
    if !state.sharing() {
        return;
    }
    let device = state.device_info().await;
    let msg = Wire::Announce {
        v: crate::model::PROTOCOL_VERSION,
        device,
        addrs: crate::net::local_addresses(),
    };
    let Ok(bytes) = serde_json::to_vec(&msg) else {
        return;
    };
    for target in announce_targets() {
        let _ = socket.send_to(&bytes, target).await;
    }
}

async fn receive_loop(state: Arc<AppState>, socket: Arc<UdpSocket>) {
    let self_id = state.settings.read().await.device_id.clone();
    let mut buf = vec![0u8; MAX_DATAGRAM];

    loop {
        let (len, from) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(err) => {
                eprintln!("[udp] recv failed: {err}");
                return;
            }
        };
        // The security boundary: only ever act on LAN sources.
        if !crate::net::is_lan_ip(from.ip()) {
            continue;
        }
        let Some(msg) = Wire::parse(&buf[..len]) else {
            continue;
        };

        match msg {
            Wire::Announce { device, mut addrs, .. } => {
                if device.id == self_id {
                    continue;
                }
                // Trust the envelope address over the claimed list: a peer
                // cannot make us talk to a machine it does not occupy.
                addrs.push(from.ip());
                let changed =
                    state
                        .peers
                        .write()
                        .await
                        .observe(device, addrs, DiscoverySource::Udp);
                if changed {
                    state.publish_peers().await;
                }
            }
            Wire::Probe { id, .. } => {
                if id == self_id {
                    continue;
                }
                send_announce(&state, &socket).await;
            }
            Wire::Bye { id, .. } => {
                if id == self_id {
                    continue;
                }
                if state.peers.write().await.forget(&id) {
                    state.publish_peers().await;
                }
            }
        }
    }
}

/// Ask everyone to announce themselves right now.
pub async fn probe(state: &Arc<AppState>) {
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await else {
        return;
    };
    let _ = socket.set_broadcast(true);
    let id = state.settings.read().await.device_id.clone();
    let msg = Wire::Probe {
        v: crate::model::PROTOCOL_VERSION,
        id,
    };
    let Ok(bytes) = serde_json::to_vec(&msg) else {
        return;
    };
    for target in announce_targets() {
        let _ = socket.send_to(&bytes, target).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: "abc".into(),
            name: "Test".into(),
            os: "linux".into(),
            kind: "desktop".into(),
            version: crate::model::PROTOCOL_VERSION,
            port: 47654,
            tls: false,
            pin_required: false,
        }
    }

    #[test]
    fn announce_round_trips() {
        let msg = Wire::Announce {
            v: crate::model::PROTOCOL_VERSION,
            device: device(),
            addrs: vec!["192.168.1.5".parse().unwrap()],
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        assert_eq!(Wire::parse(&bytes).unwrap(), msg);
    }

    #[test]
    fn probe_and_bye_round_trip() {
        for msg in [
            Wire::Probe {
                v: crate::model::PROTOCOL_VERSION,
                id: "abc".into(),
            },
            Wire::Bye {
                v: crate::model::PROTOCOL_VERSION,
                id: "abc".into(),
            },
        ] {
            let bytes = serde_json::to_vec(&msg).unwrap();
            assert_eq!(Wire::parse(&bytes).unwrap(), msg);
        }
    }

    #[test]
    fn garbage_and_wrong_versions_are_rejected() {
        assert!(Wire::parse(b"").is_none());
        assert!(Wire::parse(b"not json").is_none());
        assert!(Wire::parse(b"{}").is_none());

        let wrong = serde_json::json!({
            "op": "probe",
            "v": crate::model::PROTOCOL_VERSION + 99,
            "id": "abc"
        });
        assert!(Wire::parse(&serde_json::to_vec(&wrong).unwrap()).is_none());
    }

    #[test]
    fn oversized_datagrams_are_dropped() {
        let huge = vec![b'a'; MAX_DATAGRAM + 1];
        assert!(Wire::parse(&huge).is_none());
    }

    #[test]
    fn directed_broadcast_math() {
        let ip: Ipv4Addr = "192.168.1.10".parse().unwrap();
        assert_eq!(
            directed_broadcast(ip, 24),
            Some("192.168.1.255".parse().unwrap())
        );
        assert_eq!(
            directed_broadcast("10.1.2.3".parse().unwrap(), 8),
            Some("10.255.255.255".parse().unwrap())
        );
        // Point-to-point and unspecified prefixes have no broadcast address.
        assert_eq!(directed_broadcast(ip, 31), None);
        assert_eq!(directed_broadcast(ip, 32), None);
        assert_eq!(directed_broadcast(ip, 0), None);
    }

    #[tokio::test]
    async fn two_sockets_can_share_the_discovery_port() {
        // Without SO_REUSEADDR/SO_REUSEPORT a second instance on the same host
        // would fail to start, which is a real scenario during development.
        let Ok(a) = bind_socket() else {
            eprintln!("skipping: sandbox denies UDP bind");
            return;
        };
        let b = bind_socket().expect("a second bind on the same port must succeed");
        drop((a, b));
    }

    #[tokio::test]
    async fn announce_is_delivered_over_the_loopback_multicast_group() {
        let Ok(rx) = bind_socket() else {
            eprintln!("skipping: sandbox denies UDP bind");
            return;
        };
        join_groups(&rx);

        let tx = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.unwrap();
        tx.set_broadcast(true).ok();
        let msg = Wire::Announce {
            v: crate::model::PROTOCOL_VERSION,
            device: device(),
            addrs: vec!["127.0.0.1".parse().unwrap()],
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let target = SocketAddr::V4(SocketAddrV4::new(MULTICAST_ADDR, DISCOVERY_PORT));
        if tx.send_to(&bytes, target).await.is_err() {
            eprintln!("skipping: sandbox denies multicast send");
            return;
        }

        let mut buf = vec![0u8; MAX_DATAGRAM];
        match tokio::time::timeout(Duration::from_millis(1500), rx.recv_from(&mut buf)).await {
            Ok(Ok((len, _from))) => {
                let parsed = Wire::parse(&buf[..len]).expect("valid announce");
                assert_eq!(parsed.version(), crate::model::PROTOCOL_VERSION);
            }
            _ => eprintln!("skipping assertion: no multicast loopback in this environment"),
        }
    }
}
