//! The Norte Code client protocol.
//!
//! Versioned serde types for the Norte Code client wire. The service maps
//! between these types and the engine, so a client does not compile the
//! engine and the wire can move on its own schedule. This crate still
//! depends on Norte for catalog capability mapping and adjacent adapters;
//! those engine types are kept out of the wire structs and mapped in one
//! place, so the crate is not engine-free until that dependency is removed.
//!
//! The two halves of the wire are [`ClientRequest`] and [`ServerEvent`]. Every
//! server event carries the request it answers and, when it is about one, the
//! session it belongs to.

#![forbid(unsafe_code)]

pub mod catalog;
pub mod commands;
pub mod config;
pub mod costs;
pub mod keys;
pub mod plans;
pub mod plugins;
pub mod skills;
pub mod sqlite;
pub mod store;
pub mod tools;
pub mod types;
pub mod wire;

pub use store::*;
pub use types::*;
pub use wire::*;

/// The protocol revision. It is bumped when a type changes shape in a way an
/// older client cannot read.
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips() {
        let request = ClientRequest {
            request_id: 3,
            kind: ClientRequestKind::SendMessage {
                session: Some(SessionKey::new("abc")),
                text: "hi".to_owned(),
                attachments: Vec::new(),
                budget: Some(Budget {
                    unit: BudgetUnit::Usd,
                    limit: 1.0,
                    action: BudgetAction::Stop,
                }),
                reasoning: Some(Reasoning::Effort {
                    effort: "high".to_owned(),
                }),
                replay: false,
            },
        };
        let json = serde_json::to_string(&request).expect("serialize");
        let back: ClientRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.request_id, 3);
    }

    #[test]
    fn an_event_round_trips() {
        let event = ServerEvent::new(
            1,
            Some(SessionKey::new("s")),
            ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Text {
                    index: 0,
                    text: "hi".to_owned(),
                },
            },
        );
        let json = serde_json::to_string(&event).expect("serialize");
        let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.session.expect("session").as_str(), "s");
    }

    #[test]
    fn a_steer_applied_event_round_trips() {
        let event = ServerEvent::new(
            1,
            Some(SessionKey::new("s")),
            ServerEventKind::SteerApplied {
                ids: vec!["one".to_owned()],
                messages: vec!["hello".to_owned()],
            },
        );
        let json = serde_json::to_string(&event).expect("serialize");
        let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            back.kind,
            ServerEventKind::SteerApplied { ids, messages }
                if ids == ["one"] && messages == ["hello"]
        ));
    }

    #[test]
    fn a_retry_event_round_trips() {
        let event = ServerEvent::new(
            1,
            Some(SessionKey::new("s")),
            ServerEventKind::RetryScheduled {
                turn: 3,
                attempt: 2,
                delay_ms: 1_500,
                reason: "rate limited".to_owned(),
            },
        );
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"event\":\"retry_scheduled\""));
        let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
        match back.kind {
            ServerEventKind::RetryScheduled {
                turn,
                attempt,
                delay_ms,
                reason,
            } => {
                assert_eq!((turn, attempt, delay_ms), (3, 2, 1_500));
                assert_eq!(reason, "rate limited");
            }
            other => panic!("expected a retry, got {other:?}"),
        }
    }

    #[test]
    fn a_retry_start_round_trips() {
        let event = ServerEvent::new(
            1,
            Some(SessionKey::new("s")),
            ServerEventKind::RetryStarted {
                turn: 3,
                attempt: 2,
            },
        );
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"event\":\"retry_started\""));
        let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            back.kind,
            ServerEventKind::RetryStarted {
                turn: 3,
                attempt: 2
            }
        ));
    }
}
