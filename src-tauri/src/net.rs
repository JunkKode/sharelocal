//! Local interface enumeration, subnet math and the "is this address on my
//! LAN?" predicate that gates every inbound request.

use crate::model::LinkKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Iface {
    pub name: String,
    pub ip: IpAddr,
    pub prefix: u8,
    pub link: LinkKind,
    pub loopback: bool,
}

impl Iface {
    /// True when `other` sits inside this interface's subnet.
    pub fn contains(&self, other: IpAddr) -> bool {
        match (self.ip, other) {
            (IpAddr::V4(a), IpAddr::V4(b)) => {
                if self.prefix == 0 || self.prefix > 32 {
                    return false;
                }
                let mask = u32::MAX
                    .checked_shl(32 - self.prefix as u32)
                    .unwrap_or(u32::MAX);
                (u32::from(a) & mask) == (u32::from(b) & mask)
            }
            (IpAddr::V6(a), IpAddr::V6(b)) => {
                if self.prefix == 0 || self.prefix > 128 {
                    return false;
                }
                let (a, b) = (u128::from(a), u128::from(b));
                let shift = 128 - self.prefix as u32;
                let mask = if shift >= 128 {
                    0
                } else {
                    u128::MAX.checked_shl(shift).unwrap_or(u128::MAX)
                };
                (a & mask) == (b & mask)
            }
            _ => false,
        }
    }
}

fn netmask_to_prefix_v4(mask: Ipv4Addr) -> u8 {
    u32::from(mask).count_ones() as u8
}

fn netmask_to_prefix_v6(mask: Ipv6Addr) -> u8 {
    u128::from(mask).count_ones() as u8
}

/// Enumerate usable interfaces. Loopback is included but flagged, because the
/// browser client legitimately connects over `127.0.0.1` during development.
pub fn interfaces() -> Vec<Iface> {
    let mut out = Vec::new();
    let Ok(addrs) = if_addrs::get_if_addrs() else {
        return out;
    };
    for a in addrs {
        let (ip, prefix) = match &a.addr {
            if_addrs::IfAddr::V4(v4) => (IpAddr::V4(v4.ip), netmask_to_prefix_v4(v4.netmask)),
            if_addrs::IfAddr::V6(v6) => (IpAddr::V6(v6.ip), netmask_to_prefix_v6(v6.netmask)),
        };
        out.push(Iface {
            link: link_kind_for(&a.name),
            loopback: a.is_loopback(),
            name: a.name,
            ip,
            prefix,
        });
    }
    out
}

/// Interfaces that are never the local network.
///
/// macOS in particular is full of these: `utunN` tunnels come and go with
/// VPNs, iCloud Private Relay and Handoff; `awdl0` and `llw0` are Apple's own
/// peer-to-peer radios (AirDrop's transport, not a LAN); `anpi`/`ap` are
/// internal. Treating them as real interfaces made the address list churn
/// every few seconds, which in turn wiped the discovered-device list.
fn is_virtual_interface(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "utun", "awdl", "llw", "anpi", "ap", "ipsec", "ppp", "gif", "stf", "tun", "tap", "vmnet",
        "vboxnet", "docker", "veth", "zt", "wg", "tailscale", "lo",
    ];
    let lower = name.to_ascii_lowercase();
    PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// True for an address we could actually hand to a peer to connect back on.
///
/// IPv6 link-local is excluded: `fe80::` is only meaningful together with a
/// scope id, which does not survive being written into a TXT record or a JSON
/// datagram, so advertising it just produces unreachable candidates.
fn is_contactable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(_) => is_lan_ip(ip),
        IpAddr::V6(v6) => is_lan_ip(ip) && (v6.segments()[0] & 0xffc0) != 0xfe80,
    }
}

/// Interfaces that can actually carry LAN traffic, best candidate first
/// (IPv4 private addresses before IPv6, Ethernet before Wi-Fi before unknown).
pub fn usable_interfaces() -> Vec<Iface> {
    let mut ifaces: Vec<Iface> = interfaces()
        .into_iter()
        .filter(|i| {
            !i.loopback && !is_virtual_interface(&i.name) && is_contactable(i.ip)
        })
        .collect();
    ifaces.sort_by_key(|i| {
        let family = if i.ip.is_ipv4() { 0 } else { 1 };
        let link = match i.link {
            LinkKind::Ethernet => 0,
            LinkKind::Wifi => 1,
            LinkKind::Unknown => 2,
        };
        (family, link)
    });
    ifaces
}

pub fn primary_interface(preferred: Option<&str>) -> Option<Iface> {
    let ifaces = usable_interfaces();
    if let Some(want) = preferred {
        if let Some(found) = ifaces.iter().find(|i| i.name == want && i.ip.is_ipv4()) {
            return Some(found.clone());
        }
        if let Some(found) = ifaces.iter().find(|i| i.name == want) {
            return Some(found.clone());
        }
    }
    ifaces.into_iter().next()
}

/// Every non-loopback LAN address we currently hold, used to advertise
/// ourselves and to detect IP changes.
pub fn local_addresses() -> Vec<IpAddr> {
    usable_interfaces().into_iter().map(|i| i.ip).collect()
}

/// The security predicate: an address is acceptable only if it is loopback,
/// RFC1918 / CGNAT private, or link-local. Public addresses are always refused
/// so the file server can never serve the open internet.
pub fn is_lan_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // 100.64.0.0/10 carrier-grade NAT, also used by Tailscale-style
                // overlays that behave like a LAN.
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                // fe80::/10 link-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // fc00::/7 unique local
                || (v6.octets()[0] & 0xfe) == 0xfc
                // IPv4-mapped: judge by the embedded v4 address.
                || v6.to_ipv4_mapped().map(|m| is_lan_ip(IpAddr::V4(m))).unwrap_or(false)
        }
    }
}

/// True when `ip` falls inside one of our own interface subnets. Peers that
/// fail this are still listed, but flagged, because they are frequently
/// unreachable due to AP/client isolation.
pub fn same_subnet(ip: IpAddr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    interfaces().iter().any(|i| i.contains(ip))
}

/// Best-effort physical link classification.
///
/// Linux exposes `/sys/class/net/<if>/wireless`; macOS answers through
/// `networksetup`, which we query once and cache. Everything else falls back to
/// interface-name heuristics.
pub fn link_kind_for(name: &str) -> LinkKind {
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new(&format!("/sys/class/net/{name}/wireless")).exists() {
            return LinkKind::Wifi;
        }
        if std::path::Path::new(&format!("/sys/class/net/{name}")).exists() {
            let lower = name.to_ascii_lowercase();
            if lower.starts_with("wl") {
                return LinkKind::Wifi;
            }
            if lower.starts_with("en") || lower.starts_with("eth") {
                return LinkKind::Ethernet;
            }
        }
        LinkKind::Unknown
    }

    #[cfg(target_os = "macos")]
    {
        for (dev, kind) in macos_hardware_ports() {
            if dev == name {
                return *kind;
            }
        }
        LinkKind::Unknown
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let lower = name.to_ascii_lowercase();
        if lower.contains("wi-fi") || lower.contains("wifi") || lower.contains("wireless") {
            LinkKind::Wifi
        } else if lower.contains("ethernet") || lower.contains("lan") {
            LinkKind::Ethernet
        } else {
            LinkKind::Unknown
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_hardware_ports() -> &'static [(String, LinkKind)] {
    static PORTS: OnceLock<Vec<(String, LinkKind)>> = OnceLock::new();
    PORTS.get_or_init(|| {
        let mut out = Vec::new();
        let Ok(output) = std::process::Command::new("networksetup")
            .arg("-listallhardwareports")
            .output()
        else {
            return out;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let mut current = LinkKind::Unknown;
        for line in text.lines() {
            let line = line.trim();
            if let Some(port) = line.strip_prefix("Hardware Port:") {
                let port = port.trim().to_ascii_lowercase();
                current = if port.contains("wi-fi") || port.contains("airport") {
                    LinkKind::Wifi
                } else if port.contains("ethernet") || port.contains("lan") {
                    LinkKind::Ethernet
                } else {
                    LinkKind::Unknown
                };
            } else if let Some(dev) = line.strip_prefix("Device:") {
                out.push((dev.trim().to_string(), current));
            }
        }
        out
    })
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn unused_oncelock_marker() -> &'static OnceLock<()> {
    static X: OnceLock<()> = OnceLock::new();
    &X
}

/// A short, stable signature of the current addressing. When this changes we
/// re-advertise and reconnect, which is how roaming between Wi-Fi and Ethernet
/// (or a DHCP renewal) is handled.
///
/// Built only from addresses a peer could actually reach us on, so a VPN
/// coming up or an AirDrop radio waking does not read as "the network
/// changed" — that used to clear the peer list several times a minute.
pub fn network_signature() -> String {
    let mut addrs: Vec<String> = usable_interfaces()
        .iter()
        .map(|i| format!("{}:{}", i.name, i.ip))
        .collect();
    addrs.sort();
    addrs.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn lan_predicate_accepts_private_and_rejects_public() {
        assert!(is_lan_ip(v4("192.168.1.20")));
        assert!(is_lan_ip(v4("10.0.0.4")));
        assert!(is_lan_ip(v4("172.16.5.1")));
        assert!(is_lan_ip(v4("169.254.3.9")));
        assert!(is_lan_ip(v4("100.72.0.1")));
        assert!(is_lan_ip(v4("127.0.0.1")));

        assert!(!is_lan_ip(v4("8.8.8.8")));
        assert!(!is_lan_ip(v4("172.32.0.1")));
        assert!(!is_lan_ip(v4("100.128.0.1")));
        assert!(!is_lan_ip(v4("1.1.1.1")));
    }

    #[test]
    fn lan_predicate_handles_ipv6() {
        assert!(is_lan_ip(v4("::1")));
        assert!(is_lan_ip(v4("fe80::1")));
        assert!(is_lan_ip(v4("fd00::1")));
        assert!(!is_lan_ip(v4("2606:4700::1111")));
        // IPv4-mapped public address must still be refused.
        assert!(!is_lan_ip(v4("::ffff:8.8.8.8")));
        assert!(is_lan_ip(v4("::ffff:192.168.0.5")));
    }

    #[test]
    fn subnet_containment_v4() {
        let iface = Iface {
            name: "en0".into(),
            ip: v4("192.168.1.10"),
            prefix: 24,
            link: LinkKind::Wifi,
            loopback: false,
        };
        assert!(iface.contains(v4("192.168.1.1")));
        assert!(iface.contains(v4("192.168.1.255")));
        assert!(!iface.contains(v4("192.168.2.1")));
        assert!(!iface.contains(v4("10.0.0.1")));
    }

    #[test]
    fn subnet_containment_wide_and_degenerate_prefixes() {
        let wide = Iface {
            name: "en0".into(),
            ip: v4("10.1.2.3"),
            prefix: 8,
            link: LinkKind::Ethernet,
            loopback: false,
        };
        assert!(wide.contains(v4("10.250.9.9")));
        assert!(!wide.contains(v4("11.0.0.1")));

        let degenerate = Iface {
            prefix: 0,
            ..wide.clone()
        };
        assert!(!degenerate.contains(v4("10.1.2.4")));

        let host_route = Iface { prefix: 32, ..wide };
        assert!(host_route.contains(v4("10.1.2.3")));
        assert!(!host_route.contains(v4("10.1.2.4")));
    }

    #[test]
    fn subnet_containment_v6() {
        let iface = Iface {
            name: "en0".into(),
            ip: v4("fd00:abcd::1"),
            prefix: 64,
            link: LinkKind::Wifi,
            loopback: false,
        };
        assert!(iface.contains(v4("fd00:abcd::ff")));
        assert!(!iface.contains(v4("fd00:abce::1")));
        // Mixed families never match.
        assert!(!iface.contains(v4("192.168.1.1")));
    }
}

#[cfg(test)]
mod interface_filter_tests {
    use super::*;

    #[test]
    fn apple_and_vpn_interfaces_are_not_the_local_network() {
        // These appear and disappear constantly on macOS. Counting them made
        // the address signature churn, which cleared the peer list.
        for name in [
            "utun0", "utun5", "awdl0", "llw0", "anpi0", "ap1", "lo0", "ipsec1", "ppp0", "gif0",
            "stf0", "vmnet1", "vboxnet0", "docker0", "tun0", "tap0", "wg0",
        ] {
            assert!(is_virtual_interface(name), "{name} should be excluded");
        }
    }

    #[test]
    fn real_interfaces_are_kept() {
        for name in ["en0", "en1", "eth0", "wlan0", "enp3s0", "Ethernet", "Wi-Fi"] {
            assert!(!is_virtual_interface(name), "{name} should be usable");
        }
    }

    #[test]
    fn link_local_ipv6_is_not_advertised() {
        // fe80:: is meaningless without a scope id, which cannot survive a TXT
        // record, so handing it to a peer only produces failed connections.
        assert!(!is_contactable("fe80::1".parse().unwrap()));
        assert!(!is_contactable("fe80::8d:7bf8:4ec1:87bf".parse().unwrap()));

        assert!(is_contactable("192.168.1.10".parse().unwrap()));
        assert!(is_contactable("10.0.0.5".parse().unwrap()));
        assert!(is_contactable("fd00::1".parse().unwrap()));
        // IPv4 link-local is genuinely usable for an ad-hoc link.
        assert!(is_contactable("169.254.4.5".parse().unwrap()));
        assert!(!is_contactable("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn the_signature_only_covers_contactable_addresses() {
        let signature = network_signature();
        assert!(
            !signature.contains("fe80"),
            "link-local addresses must not destabilise the signature: {signature}"
        );
        for virt in ["utun", "awdl", "llw"] {
            assert!(
                !signature.contains(virt),
                "{virt} interfaces must not appear in the signature: {signature}"
            );
        }
    }

    #[test]
    fn the_signature_is_stable_across_repeated_reads() {
        let a = network_signature();
        let b = network_signature();
        assert_eq!(a, b, "back-to-back reads must agree");
    }

    #[test]
    fn inbound_link_local_peers_are_still_accepted() {
        // Filtering is about what we *advertise*; a peer connecting from a
        // link-local address is still on the LAN and must be allowed in.
        assert!(is_lan_ip("fe80::1".parse().unwrap()));
    }
}
