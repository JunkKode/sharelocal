//! Device discovery: mDNS where it works, UDP multicast where it does not, and
//! a watcher that notices the machine changing networks.

pub mod mdns;
pub mod registry;
pub mod udp;

use crate::state::AppState;
use std::sync::Arc;
use std::time::Duration;

/// Start every discovery task. Each supervises itself and never returns.
pub fn spawn(state: Arc<AppState>) {
    {
        let state = state.clone();
        tokio::spawn(async move {
            if state.settings.read().await.discovery_mdns {
                mdns::run(state).await;
            }
        });
    }
    {
        let state = state.clone();
        tokio::spawn(async move {
            if state.settings.read().await.discovery_udp {
                udp::run(state).await;
            }
        });
    }
    tokio::spawn(reaper(state.clone()));
    tokio::spawn(network_watcher(state));
}

/// Ages peers out and sweeps dead sessions once a second.
async fn reaper(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        ticker.tick().await;

        if state.peers.write().await.sweep() {
            state.publish_peers().await;
        }

        let expired = state.sessions.write().await.sweep();
        for id in expired {
            crate::transfer::cleanup_parts(&state, &id).await;
            state.publish(crate::events::AppEvent::SessionRemoved { id });
        }

        let mut clients = state.web_clients.write().await;
        let stale: Vec<String> = clients
            .iter()
            .filter(|(_, c)| c.is_expired())
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            clients.remove(&id);
        }
    }
}

/// Detects IP changes (Wi-Fi ↔ Ethernet, DHCP renewal, VPN up/down) and pokes
/// the rest of the system so advertisements and the peer list stay truthful.
async fn network_watcher(state: Arc<AppState>) {
    let mut last = crate::net::network_signature();
    let mut ticker = tokio::time::interval(Duration::from_secs(3));

    loop {
        ticker.tick().await;
        let current = crate::net::network_signature();
        if current == last {
            continue;
        }
        last = current;

        // Peers learned on the old network are almost certainly unreachable.
        state.peers.write().await.clear();
        state.publish_peers().await;
        state.publish_network().await;
        state.network_changed.notify_waiters();

        state.publish(crate::events::AppEvent::notice(
            crate::events::NoticeLevel::Info,
            "network-changed",
            "Network changed — rediscovering nearby devices.",
        ));

        // Give interfaces a moment to settle, then ask everyone to speak up.
        let state2 = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(800)).await;
            udp::probe(&state2).await;
        });
    }
}
