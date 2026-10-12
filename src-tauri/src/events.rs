//! The single event bus. Everything interesting that happens in the core is
//! published here; the Tauri UI and each connected WebSocket client subscribe.

use crate::model::{NetworkStatus, Peer, SessionView};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Who is allowed to see an event.
///
/// Browser clients share the bus with the local UI, so every event declares its
/// audience rather than relying on call sites to remember.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    /// Local UI only — may contain settings or filesystem paths.
    Local,
    /// Safe to relay to a LAN peer, subject to the per-session filter.
    Shared,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AppEvent {
    PeersChanged {
        peers: Vec<Peer>,
    },
    IncomingOffer {
        session: SessionView,
    },
    SessionUpdated {
        session: SessionView,
    },
    SessionRemoved {
        id: String,
    },
    NetworkChanged {
        network: NetworkStatus,
    },
    SettingsChanged,
    HistoryChanged,
    /// A user-facing message with a stable `code` the UI maps to copy.
    Notice {
        level: NoticeLevel,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

impl AppEvent {
    pub fn audience(&self) -> Audience {
        match self {
            AppEvent::IncomingOffer { .. }
            | AppEvent::SessionUpdated { .. }
            | AppEvent::SessionRemoved { .. }
            | AppEvent::Notice { .. } => Audience::Shared,
            AppEvent::PeersChanged { .. }
            | AppEvent::NetworkChanged { .. }
            | AppEvent::SettingsChanged
            | AppEvent::HistoryChanged => Audience::Local,
        }
    }

    /// The session an event concerns, used to keep one browser client from
    /// observing another client's transfers.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            AppEvent::IncomingOffer { session } | AppEvent::SessionUpdated { session } => {
                Some(&session.id)
            }
            AppEvent::SessionRemoved { id } => Some(id),
            _ => None,
        }
    }

    /// The Tauri event name this is emitted under.
    pub fn tauri_name(&self) -> &'static str {
        match self {
            AppEvent::PeersChanged { .. } => "peers-changed",
            AppEvent::IncomingOffer { .. } => "incoming-offer",
            AppEvent::SessionUpdated { .. } => "session-updated",
            AppEvent::SessionRemoved { .. } => "session-removed",
            AppEvent::NetworkChanged { .. } => "network-changed",
            AppEvent::SettingsChanged => "settings-changed",
            AppEvent::HistoryChanged => "history-changed",
            AppEvent::Notice { .. } => "notice",
        }
    }

    pub fn notice(level: NoticeLevel, code: &str, message: impl Into<String>) -> Self {
        AppEvent::Notice {
            level,
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Bounded fan-out. Slow subscribers lag rather than back-pressuring the
/// transfer loop — progress events are disposable by design.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<AppEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(512)
    }
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn publish(&self, event: AppEvent) {
        // An error here just means nobody is listening yet.
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_events_never_reach_remote_clients() {
        assert_eq!(AppEvent::SettingsChanged.audience(), Audience::Local);
        assert_eq!(
            AppEvent::PeersChanged { peers: vec![] }.audience(),
            Audience::Local
        );
        assert_eq!(
            AppEvent::notice(NoticeLevel::Info, "x", "y").audience(),
            Audience::Shared
        );
    }

    #[tokio::test]
    async fn subscribers_receive_published_events() {
        let bus = EventBus::new(8);
        let mut rx = bus.subscribe();
        bus.publish(AppEvent::SettingsChanged);
        let got = rx.recv().await.unwrap();
        assert!(matches!(got, AppEvent::SettingsChanged));
    }

    #[tokio::test]
    async fn publishing_with_no_subscribers_is_not_an_error() {
        let bus = EventBus::new(4);
        bus.publish(AppEvent::HistoryChanged);
    }
}
