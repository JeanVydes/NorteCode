//! The arguments of the delegation tools.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The arguments of `inspect_agent` (RFC 8.2.7).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InspectArgs {
    /// The child to inspect, by the id `spawn_agent` returned.
    pub child_session_id: String,
    /// What to look at: `status` (default), `transcript`, `events` or `jobs`.
    #[serde(default)]
    pub view: Option<String>,
    /// Only the events after this sequence.
    #[serde(default)]
    pub from_seq: Option<u64>,
    /// How many entries at most.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// The arguments of `message_agent` (RFC 8.2.8).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageArgs {
    /// The child to message.
    pub child_session_id: String,
    /// The message. A string, or an array of content parts (only text is
    /// carried here; a part that is not text is refused).
    pub content: serde_json::Value,
    /// How it is delivered: `steer` (default), `queue` or `interrupt`.
    #[serde(default)]
    pub delivery: Option<String>,
}

/// The arguments of `stop_agent` (RFC 8.2.7).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StopArgs {
    /// The child to stop.
    pub child_session_id: String,
    /// `graceful` (default) waits for its turn to end; `kill` cancels it.
    #[serde(default)]
    pub mode: Option<String>,
    /// Why it is being stopped.
    #[serde(default)]
    pub reason: Option<String>,
    /// Grace period before escalation to cancellation.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

/// The arguments of `wait_agent` (RFC 8.2.7).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitArgs {
    /// The child to wait for.
    pub child_session_id: String,
    /// `idle` (default) waits for its current turn to finish; `finished`
    /// waits for it to reach a terminal state.
    #[serde(default)]
    pub until: Option<String>,
    /// How long to wait, in seconds (default 300).
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

/// The arguments of `list_agents` (RFC 8.2.7).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListArgs {
    /// Only the children in these states.
    #[serde(default)]
    pub state: Option<Vec<String>>,
    /// How many at most.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// The arguments of `approve_child_call` (RFC 8.2.9).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApproveArgs {
    /// The child whose call is being approved.
    pub child_session_id: String,
    /// The tool call to approve or deny.
    pub tool_call_id: String,
    /// `approve` or `deny`.
    pub decision: String,
    /// Why.
    #[serde(default)]
    pub reason: Option<String>,
}

/// The arguments of `ask_delegator` (RFC 8.2.9).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AskDelegatorArgs {
    /// The question for the parent (or, when the parent is a human, for them).
    pub question: String,
    /// Closed choices, when the model offered any.
    #[serde(default)]
    pub options: Option<Vec<String>>,
    /// How long to wait, in seconds (default 60).
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

/// The arguments of `handoff_agent` (RFC 8.3).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HandoffArgs {
    /// The agent the rest of the turn is handed to.
    pub agent_id: String,
    /// Why.
    #[serde(default)]
    pub reason: Option<String>,
    /// How much of the context the new agent carries: `full`, `summary` or
    /// `none` (default `summary`).
    #[serde(default)]
    pub carry_context: Option<String>,
}

// ---------------------------------------------------------------------------
// The delegating tools (RFC 8.2.7-8.2.9, 8.3)
//
// Each tool is a thin declaration over the shared services the parent and
// the child both live in: the child registry (who is whose child, and
// whether its delegator is suspended) and the service (to read the child's
// phase and to steer it). `ask_delegator` reuses the question broker, which
// is how the child already asks a human.
// ---------------------------------------------------------------------------
