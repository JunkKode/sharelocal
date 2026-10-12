//! mDNS/Bonjour advertising and browsing for `_sharelocal._tcp.local.`.
//!
//! TXT records carry the same `DeviceInfo` the UDP fallback sends, so both
//! backends produce identical peers.

use crate::model::{DeviceInfo, DiscoverySource, PROTOCOL_VERSION, SERVICE_TYPE};
use crate::state::AppState;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Build the TXT properties advertised for this device.
pub fn txt_properties(device: &DeviceInfo) -> HashMap<String, String> {
    HashMap::from([
        ("id".to_string(), device.id.clone()),
        ("name".to_string(), device.name.clone()),
        ("os".to_string(), device.os.clone()),
        ("kind".to_string(), device.kind.clone()),
        ("ver".to_string(), device.version.to_string()),
        ("port".to_string(), device.port.to_string()),
        (
            "tls".to_string(),
            if device.tls { "1".into() } else { "0".into() },
        ),
        (
            "pin".to_string(),
            if device.pin_required {
                "1".into()
            } else {
                "0".into()
            },
        ),
    ])
}

/// Rebuild a `DeviceInfo` from TXT records. Returns `None` when the record is
/// not a usable ShareLocal advertisement.
pub fn device_from_txt(
    lookup: impl Fn(&str) -> Option<String>,
    fallback_port: u16,
) -> Option<DeviceInfo> {
    let id = lookup("id")?;
    if id.trim().is_empty() {
        return None;
    }
    let version: u32 = lookup("ver").and_then(|v| v.parse().ok()).unwrap_or(0);
    if version != PROTOCOL_VERSION {
        return None;
    }
    let port = lookup("port")
        .and_then(|p| p.parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(fallback_port);

    Some(DeviceInfo {
        name: lookup("name")
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| id.clone()),
        os: lookup("os").unwrap_or_else(|| "unknown".into()),
        kind: lookup("kind").unwrap_or_else(|| "desktop".into()),
        id,
        version,
        port,
        tls: lookup("tls").as_deref() == Some("1"),
        pin_required: lookup("pin").as_deref() == Some("1"),
    })
}

/// A stable, DNS-safe host label derived from the device id.
fn host_label(device_id: &str) -> String {
    let cleaned: String = device_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(24)
        .collect();
    format!("sharelocal-{cleaned}")
}

/// Instance names must be unique on the link; append a short id so two
/// machines with the same computer name do not collide.
fn instance_name(device: &DeviceInfo) -> String {
    let short: String = device
        .id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(6)
        .collect();
    format!("{} ({})", device.name, short)
}

pub async fn run(state: Arc<AppState>) {
    loop {
        match ServiceDaemon::new() {
            Ok(daemon) => {
                state.mdns_ok.store(true, Ordering::SeqCst);
                state.publish_network().await;
                serve(state.clone(), daemon).await;
                state.mdns_ok.store(false, Ordering::SeqCst);
                state.publish_network().await;
            }
            Err(err) => {
                state.mdns_ok.store(false, Ordering::SeqCst);
                eprintln!("[mdns] unavailable ({err}); the UDP fallback is carrying discovery");
                state.publish_network().await;
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn serve(state: Arc<AppState>, daemon: ServiceDaemon) {
    let browse = match daemon.browse(SERVICE_TYPE) {
        Ok(rx) => rx,
        Err(err) => {
            eprintln!("[mdns] browse failed: {err}");
            return;
        }
    };

    let advertiser = tokio::spawn(advertise_loop(state.clone(), daemon.clone()));
    let browser = tokio::spawn(browse_loop(state.clone(), browse));

    tokio::select! {
        _ = advertiser => {}
        _ = browser => {}
    }
    let _ = daemon.shutdown();
}

/// Register the service and re-register whenever our addressing changes.
async fn advertise_loop(state: Arc<AppState>, daemon: ServiceDaemon) {
    let mut registered: Option<String> = None;
    let mut last_signature = String::new();

    loop {
        let device = state.device_info().await;
        let signature = format!(
            "{}|{}|{}",
            crate::net::network_signature(),
            device.port,
            device.name
        );

        if signature != last_signature {
            if let Some(fullname) = registered.take() {
                let _ = daemon.unregister(&fullname);
            }
            let addrs: Vec<IpAddr> = crate::net::local_addresses();
            if state.sharing() && !addrs.is_empty() {
                match ServiceInfo::new(
                    SERVICE_TYPE,
                    &instance_name(&device),
                    &format!("{}.local.", host_label(&device.id)),
                    &addrs[..],
                    device.port,
                    txt_properties(&device),
                ) {
                    Ok(info) => {
                        let fullname = info.get_fullname().to_string();
                        match daemon.register(info) {
                            Ok(()) => {
                                registered = Some(fullname);
                                last_signature = signature;
                            }
                            Err(err) => eprintln!("[mdns] register failed: {err}"),
                        }
                    }
                    Err(err) => eprintln!("[mdns] bad service info: {err}"),
                }
            } else {
                last_signature = signature;
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(20)) => {}
            _ = state.network_changed.notified() => {}
        }
    }
}

async fn browse_loop(state: Arc<AppState>, browse: mdns_sd::Receiver<ServiceEvent>) {
    let self_id = state.settings.read().await.device_id.clone();
    // Remember which peer a service fullname belongs to so `ServiceRemoved`
    // can be acted on — the removal event carries no TXT records.
    let mut by_fullname: HashMap<String, String> = HashMap::new();

    while let Ok(event) = browse.recv_async().await {
        match event {
            ServiceEvent::ServiceResolved(resolved) => {
                let txt = resolved.txt_properties.clone();
                let Some(device) = device_from_txt(
                    |k| txt.get_property_val_str(k).map(|s| s.to_string()),
                    resolved.port,
                ) else {
                    continue;
                };
                if device.id == self_id {
                    continue;
                }
                by_fullname.insert(resolved.fullname.clone(), device.id.clone());

                let addrs: Vec<IpAddr> =
                    resolved.addresses.iter().map(|a| a.to_ip_addr()).collect();
                let changed =
                    state
                        .peers
                        .write()
                        .await
                        .observe(device, addrs, DiscoverySource::Mdns);
                if changed {
                    state.publish_peers().await;
                }
            }
            ServiceEvent::ServiceRemoved(_ty, fullname) => {
                if let Some(id) = by_fullname.remove(&fullname) {
                    if state.peers.write().await.forget(&id) {
                        state.publish_peers().await;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: "9f1c2a44-1111-2222-3333-444455556666".into(),
            name: "MacBook Pro".into(),
            os: "macos".into(),
            kind: "laptop".into(),
            version: PROTOCOL_VERSION,
            port: 47654,
            tls: false,
            pin_required: true,
        }
    }

    #[test]
    fn txt_records_round_trip_through_a_device() {
        let original = device();
        let props = txt_properties(&original);
        let rebuilt = device_from_txt(|k| props.get(k).cloned(), 0).expect("valid record");
        assert_eq!(rebuilt, original);
    }

    #[test]
    fn records_without_an_id_are_ignored() {
        let props: HashMap<String, String> =
            HashMap::from([("ver".into(), PROTOCOL_VERSION.to_string())]);
        assert!(device_from_txt(|k| props.get(k).cloned(), 47654).is_none());
    }

    #[test]
    fn other_protocol_versions_are_ignored() {
        let mut props = txt_properties(&device());
        props.insert("ver".into(), "99".into());
        assert!(
            device_from_txt(|k| props.get(k).cloned(), 47654).is_none(),
            "a future protocol version must not be treated as compatible"
        );
    }

    #[test]
    fn a_missing_port_falls_back_to_the_srv_record() {
        let mut props = txt_properties(&device());
        props.remove("port");
        let rebuilt = device_from_txt(|k| props.get(k).cloned(), 51000).unwrap();
        assert_eq!(rebuilt.port, 51000);
    }

    #[test]
    fn a_missing_name_falls_back_to_the_id() {
        let mut props = txt_properties(&device());
        props.insert("name".into(), "  ".into());
        let rebuilt = device_from_txt(|k| props.get(k).cloned(), 47654).unwrap();
        assert_eq!(rebuilt.name, rebuilt.id);
    }

    #[test]
    fn flags_default_to_off_when_absent() {
        let mut props = txt_properties(&device());
        props.remove("tls");
        props.remove("pin");
        let rebuilt = device_from_txt(|k| props.get(k).cloned(), 47654).unwrap();
        assert!(!rebuilt.tls);
        assert!(!rebuilt.pin_required);
    }

    #[test]
    fn host_labels_are_dns_safe() {
        let label = host_label(&device().id);
        assert!(label.starts_with("sharelocal-"));
        assert!(label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert!(label.len() <= 63, "DNS labels are limited to 63 octets");
    }

    #[test]
    fn instance_names_disambiguate_identical_computer_names() {
        let a = device();
        let mut b = device();
        b.id = "00000000-aaaa-bbbb-cccc-dddddddddddd".into();
        assert_ne!(instance_name(&a), instance_name(&b));
        assert!(instance_name(&a).starts_with("MacBook Pro"));
    }
}
