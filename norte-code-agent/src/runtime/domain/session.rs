use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use norte::id::{NodeId, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Durable position of a session in the turn loop. Unknown phases fail to
/// deserialize instead of being coerced to a default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Phase {
    /// Nothing in flight: the session is ready for a new turn.
    #[default]
    Idle,
    /// A turn is running.
    Streaming,
    /// The model asked for tool calls the host must run.
    AwaitingTool,
}

impl Phase {
    /// Whether the session is idle.
    #[must_use]
    pub fn is_idle(self) -> bool {
        matches!(self, Phase::Idle)
    }
}

/// Durable conversation state: where the active branch is, what the engine is
/// waiting for, the concurrency token and the host-visible extras.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The session's identifier.
    pub id: SessionId,
    /// The node the active branch ends at, if anything is committed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<NodeId>,
    /// What the engine is waiting for.
    pub phase: Phase,
    /// Incremented by exactly one per commit; the optimistic-concurrency token.
    pub version: u64,
    /// How many turns the session has run.
    pub turn: u64,
    /// Host-visible extras, which the engine never interprets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session last changed.
    pub updated_at: DateTime<Utc>,
}

impl Session {
    /// A new idle session, with nothing committed and no turn run.
    #[must_use]
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            id: SessionId::new(),
            head: None,
            phase: Phase::Idle,
            version: 0,
            turn: 0,
            metadata: BTreeMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Whether the session is idle.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.phase.is_idle()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_phase_fails_to_deserialize() {
        let result: Result<Phase, _> = serde_json::from_str("\"teleporting\"");
        assert!(result.is_err());
    }

    #[test]
    fn phases_round_trip() {
        for phase in [Phase::Idle, Phase::Streaming, Phase::AwaitingTool] {
            let json = serde_json::to_string(&phase).expect("serialize");
            let decoded: Phase = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, phase);
        }
    }
}
