//! What a session keeps in its stored `stats` field.
//!
//! The store treats this as opaque, which is right: it is the service's own
//! bookkeeping, not part of the conversation. It used to be built and read by
//! string key, so a field could be written under one name and read under
//! another, and one was written by nobody at all. It is a struct now, and
//! serializing it is the only way its shape reaches the store.

use std::collections::HashSet;

use norte_code_wire::{Budget, MessageRole, Reasoning, ReplayedKind, SessionKey};

use super::session_record::{ChildTerminalState, TurnBoundary};
use super::steering::Steered;

/// The counters and settings a session persists.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct StoredStats {
    /// Messages of this session already reported for usage.
    #[serde(default)]
    pub(super) billed: usize,
    /// Model requests reported for usage.
    #[serde(default)]
    pub(super) requests: u64,
    /// Provider-reported cost, summed.
    #[serde(default)]
    pub(super) cost: f64,
    /// Requests that carried no cost.
    #[serde(default)]
    pub(super) unpriced: u64,
    /// Input tokens, summed over reported requests.
    #[serde(default)]
    pub(super) input_tokens: u64,
    /// Output tokens, summed over reported requests.
    #[serde(default)]
    pub(super) output_tokens: u64,
    /// Prompt tokens the provider served from its cache, summed.
    #[serde(default)]
    pub(super) cache_read_tokens: u64,
    /// Prompt tokens the provider wrote to its cache, summed.
    #[serde(default)]
    pub(super) cache_write_tokens: u64,
    /// The input tokens of the last request: the context the session holds.
    #[serde(default)]
    pub(super) context_used: u64,
    /// Highest RAEP turn id assigned to this logical session.
    #[serde(default)]
    pub(super) last_turn: u64,
    /// Durable turn-to-conversation boundaries, including compacted ancestors.
    #[serde(default)]
    pub(super) turn_boundaries: Vec<TurnBoundary>,
    /// History items retained after their turns leave the active model context.
    #[serde(default)]
    pub(super) rewound_history: Vec<RewoundItem>,
    /// Origin metadata for imported session history.
    #[serde(default)]
    pub(super) imported_from: Option<serde_json::Value>,
    /// Imported root session awaits its first controller attachment.
    #[serde(default)]
    pub(super) imported_suspended: bool,
    /// The session budget, when one was set.
    #[serde(default)]
    pub(super) budget: Option<Budget>,
    /// Agent profile selected when this session was created.
    #[serde(default)]
    pub(super) agent_profile: Option<String>,
    /// The history length written by the last compaction.
    #[serde(default)]
    pub(super) compaction_baseline: usize,
    /// The session-wide reasoning override, when one was set.
    #[serde(default)]
    pub(super) reasoning: Option<Reasoning>,
    /// A reasoning override for the next turn only.
    #[serde(default)]
    pub(super) pending_reasoning: Option<Reasoning>,
    /// A budget override for the next turn only.
    #[serde(default)]
    pub(super) pending_budget: Option<Budget>,
    /// Messages queued for the next turn boundary, in arrival order.
    #[serde(default)]
    pub(super) pending_steers: Vec<Steered>,
    /// A model switch accepted while the session was busy.
    #[serde(default)]
    pub(super) pending_provider: Option<String>,
    /// The model id of the queued switch.
    #[serde(default)]
    pub(super) pending_model: Option<String>,
    /// The tools the configuration withheld when this session was created.
    #[serde(default)]
    pub(super) disabled_tools: Vec<String>,
    /// The parent session this one was spawned from.
    #[serde(default)]
    pub(super) parent_client: Option<SessionKey>,
    /// The tool call that created this child.
    #[serde(default)]
    pub(super) parent_tool_call_id: Option<String>,
    /// Terminal delegation state, if the child has ended.
    #[serde(default)]
    pub(super) child_state: Option<ChildTerminalState>,
    /// The delegation depth: 0 for a session the client made.
    #[serde(default)]
    pub(super) depth: u32,
    /// Whether the delegator is suspended.
    #[serde(default)]
    pub(super) delegator_suspended: bool,
    /// Messages held while the delegator is suspended.
    #[serde(default)]
    pub(super) delegator_queue: Vec<String>,
    /// Turn ids pinned verbatim through compaction.
    #[serde(default)]
    pub(super) pinned_turn_ids: HashSet<String>,
}

impl StoredStats {
    /// Reads the stats a stored session carries, including the shapes an older
    /// build wrote.
    ///
    /// # Errors
    ///
    /// Returns the deserialization error when the stored value is not an
    /// object this build understands.
    pub(super) fn read(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let mut stats: Self = serde_json::from_value(value.clone()).or_else(|error| {
            // A row written before `stats` was typed has the same keys; a
            // value that is missing is what the defaults are for, but a value
            // of the wrong type is a real error and travels.
            if value.is_object() {
                Err(error)
            } else {
                Ok(Self::default())
            }
        })?;
        stats.repair_legacy_steers();
        Ok(stats)
    }

    /// Gives the messages stored before ids existed one apiece.
    fn repair_legacy_steers(&mut self) {
        for steered in &mut self.pending_steers {
            if steered.id.is_empty() {
                steered.id = uuid::Uuid::new_v4().to_string();
            }
        }
    }

    /// Serializes the stats for the store.
    ///
    /// # Errors
    ///
    /// Returns the serialization error, which a closed struct cannot produce.
    pub(super) fn value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}

/// One message retained after its turn left the active context.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct RewoundItem {
    /// The turn it belonged to.
    pub(super) turn: u64,
    /// Who spoke.
    pub(super) role: MessageRole,
    /// What kind of content it is.
    pub(super) kind: ReplayedKind,
    /// The text.
    pub(super) text: String,
    /// When it was said, in Unix milliseconds.
    pub(super) timestamp: i64,
}

impl From<(u64, MessageRole, ReplayedKind, String, i64)> for RewoundItem {
    fn from(
        (turn, role, kind, text, timestamp): (u64, MessageRole, ReplayedKind, String, i64),
    ) -> Self {
        Self {
            turn,
            role,
            kind,
            text,
            timestamp,
        }
    }
}

impl From<RewoundItem> for (u64, MessageRole, ReplayedKind, String, i64) {
    fn from(item: RewoundItem) -> Self {
        (item.turn, item.role, item.kind, item.text, item.timestamp)
    }
}
