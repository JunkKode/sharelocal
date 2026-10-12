//! The control-plane message set, shared by the WebSocket server, the outbound
//! sender and the browser client.
//!
//! Only small JSON frames travel here. File bytes always go over HTTP.

use crate::events::AppEvent;
use crate::model::{
    ConflictPolicy, DeviceInfo, Direction, FileMeta, PayloadKind, SessionStatus, SessionView,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ControlMsg {
    /// First frame from any client.
    Hello {
        device: DeviceInfo,
    },
    /// Server's reply to `Hello`.
    Welcome {
        device: DeviceInfo,
        pin_required: bool,
    },

    /// "I would like to send you these files."
    Offer {
        files: Vec<FileMeta>,
        total_bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin: Option<String>,
        direction: Direction,
        #[serde(default)]
        conflict: ConflictPolicy,
        #[serde(default)]
        kind: PayloadKind,
        /// Text shares carry their payload here rather than over HTTP: a
        /// snippet is far smaller than one round trip's worth of overhead.
        /// Capped at `MAX_TEXT_BYTES` by the receiver.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Answer to an `Offer`; `token` is only present once accepted.
    OfferStatus {
        session_id: String,
        status: SessionStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Sender → receiver, so the receiving UI can show live numbers even
    /// though the bytes are arriving on a different connection.
    Progress {
        session_id: String,
        bytes_done: u64,
        file_index: usize,
        speed_bps: f64,
    },
    Cancel {
        session_id: String,
    },
    Complete {
        session_id: String,
    },

    /// Server → browser client: a session it is party to changed.
    Session {
        session: SessionView,
    },
    /// Server → browser client: an app-level event it is allowed to see.
    Event {
        event: AppEvent,
    },

    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        code: String,
        message: String,
    },

    Ping {
        t: u64,
    },
    Pong {
        t: u64,
    },
}

/// Frames larger than this are refused before parsing.
pub const MAX_CONTROL_FRAME: usize = 512 * 1024;

impl ControlMsg {
    pub fn parse(text: &str) -> Option<ControlMsg> {
        if text.len() > MAX_CONTROL_FRAME {
            return None;
        }
        serde_json::from_str(text).ok()
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PROTOCOL_VERSION;

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: "a".into(),
            name: "A".into(),
            os: "macos".into(),
            kind: "laptop".into(),
            version: PROTOCOL_VERSION,
            port: 47654,
            tls: false,
            pin_required: false,
        }
    }

    #[test]
    fn every_variant_round_trips() {
        let msgs = vec![
            ControlMsg::Hello { device: device() },
            ControlMsg::Welcome {
                device: device(),
                pin_required: true,
            },
            ControlMsg::Offer {
                files: vec![FileMeta {
                    rel_path: "a/b.txt".into(),
                    size: 12,
                    mime: None,
                    modified_ms: None,
                }],
                total_bytes: 12,
                pin: Some("1234".into()),
                direction: Direction::Push,
                conflict: ConflictPolicy::KeepBoth,
                kind: PayloadKind::Files,
                text: None,
            },
            ControlMsg::OfferStatus {
                session_id: "s1".into(),
                status: SessionStatus::Accepted,
                token: Some("tok".into()),
                reason: None,
            },
            ControlMsg::Progress {
                session_id: "s1".into(),
                bytes_done: 5,
                file_index: 0,
                speed_bps: 1.5,
            },
            ControlMsg::Cancel {
                session_id: "s1".into(),
            },
            ControlMsg::Complete {
                session_id: "s1".into(),
            },
            ControlMsg::Error {
                session_id: None,
                code: "pin".into(),
                message: "nope".into(),
            },
            ControlMsg::Ping { t: 7 },
            ControlMsg::Pong { t: 7 },
        ];
        for msg in msgs {
            let text = msg.to_text();
            assert_eq!(ControlMsg::parse(&text).unwrap(), msg, "round trip: {text}");
        }
    }

    #[test]
    fn oversized_and_malformed_frames_are_refused() {
        assert!(ControlMsg::parse(&"x".repeat(MAX_CONTROL_FRAME + 1)).is_none());
        assert!(ControlMsg::parse("not json").is_none());
        assert!(ControlMsg::parse(r#"{"type":"nope"}"#).is_none());
        assert!(ControlMsg::parse("{}").is_none());
    }

    #[test]
    fn offer_status_omits_the_token_when_absent() {
        let msg = ControlMsg::OfferStatus {
            session_id: "s1".into(),
            status: SessionStatus::Pending,
            token: None,
            reason: None,
        };
        let text = msg.to_text();
        assert!(
            !text.contains("token"),
            "a pending offer must not carry a token"
        );
    }
}
