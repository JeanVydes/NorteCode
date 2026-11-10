//! What the service remembers about one session.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use norte_code_wire::{Budget, Phase as ProtocolPhase, Reasoning, SessionKey};

use crate::provider::ModelSpec as ProviderModelSpec;

use super::StoredSession;
use super::session_stats::StoredStats;
use super::steering::Steered;

pub(super) const MAX_HELD_DELEGATOR_MESSAGES: usize = 20;

/// The immutable conversation head reached when one turn finished.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(super) struct TurnBoundary {
    pub(super) turn: u64,
    pub(super) session: norte::SessionId,
    pub(super) head: Option<norte::NodeId>,
}

/// Persisted terminal result of a child delegation.
///
/// The two terminal states of the protocol's `ChildState`, with their wire
/// spelling derived rather than written out in three places: the persistence
/// layer, the loader and the reader all used to spell `"failed"` themselves.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ChildTerminalState {
    /// The child ended with an unrecoverable error.
    Failed,
    /// The child was stopped by its parent.
    Stopped,
}

/// Parent-side metadata recorded when a child session is created.
pub(super) struct ChildSessionMetadata {
    pub(super) parent: SessionKey,
    pub(super) tool_call_id: String,
    pub(super) task_summary: String,
    pub(super) lifetime: String,
    pub(super) agent_profile: String,
    pub(super) system_prompt: String,
    pub(super) cancel: Arc<norte::Cancellation>,
}

/// What the service remembers about a session.
#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct SessionRecord {
    pub(super) id: norte::SessionId,
    pub(super) title: Option<String>,
    pub(super) spec: Option<ProviderModelSpec>,
    /// Agent profile selected when this session was created.
    ///
    /// Persisted so a restored child uses the same profile for nested
    /// delegation rather than inheriting the service's default agent.
    pub(super) agent_profile: Option<String>,
    /// A model switch accepted while the session was busy. It replaces the
    /// current spec at the next idle turn boundary.
    ///
    /// The provider and the model are two fields: a model id may carry slashes
    /// of its own, so the pair is never stored as one string to be taken apart
    /// again.
    pub(super) pending_provider: Option<String>,
    /// The model id of the queued switch.
    pub(super) pending_model: Option<String>,
    pub(super) created_at: DateTime<Utc>,
    pub(super) phase: ProtocolPhase,
    /// Messages of this session already reported for usage.
    pub(super) billed: usize,
    /// Model requests reported for usage.
    pub(super) requests: u64,
    /// Provider-reported cost, summed.
    pub(super) cost: f64,
    /// Requests that carried no cost.
    pub(super) unpriced: u64,
    /// Input tokens, summed over reported requests.
    pub(super) input_tokens: u64,
    /// The input tokens of the last request: the context the session holds.
    ///
    /// The sum above is what was spent; this is what the last request carried,
    /// and the whole conversation is in it. It is what the footer's dial shows
    /// and what a plugin asking about the context is told.
    pub(super) context_used: u64,
    /// Highest RAEP turn id assigned to this logical session.
    pub(super) last_turn: u64,
    /// Durable turn-to-conversation boundaries, including compacted ancestors.
    pub(super) turn_boundaries: Vec<TurnBoundary>,
    /// History items retained after their turns leave the active model context.
    pub(super) rewound_history: Vec<(
        u64,
        norte_code_wire::MessageRole,
        norte_code_wire::ReplayedKind,
        String,
        i64,
    )>,
    /// Origin metadata for imported session history.
    pub(super) imported_from: Option<serde_json::Value>,
    /// Imported root session awaits its first controller attachment.
    pub(super) imported_suspended: bool,
    /// Output tokens, summed over reported requests.
    pub(super) output_tokens: u64,
    /// Prompt tokens the provider served from its cache, summed.
    pub(super) cache_read_tokens: u64,
    /// Prompt tokens the provider wrote to its cache, summed.
    pub(super) cache_write_tokens: u64,
    /// The session budget, when one was set.
    pub(super) budget: Option<Budget>,
    /// The tools the configuration withheld when this session was created.
    ///
    /// It is a snapshot and not a reading of the configuration, because the
    /// tool declarations are part of the prompt's prefix: a set that changed
    /// under a running session would invalidate the provider's cached prefix
    /// and re-bill the whole conversation. The list is fixed when the session
    /// is made, kept across a resume, and never consulted against the file
    /// again: a tool enabled then and disabled now stays with the session that
    /// already had it.
    pub(super) disabled_tools: Vec<String>,
    /// Tools this session refuses without asking again.
    pub(super) denied_tools: Vec<String>,
    /// Tools this session allows without asking again.
    pub(super) allowed_tools: Vec<String>,
    /// The Norte session this one was compacted from, when it was.
    pub(super) compacted_from: Option<norte::SessionId>,
    /// Whether a compaction is running, so a concurrent turn does not repeat it.
    pub(super) compacting: bool,
    /// The history length written by the last compaction. A session that has
    /// not grown past it has nothing new to summarise.
    pub(super) compaction_baseline: usize,
    /// The session-wide reasoning override, when one was set.
    pub(super) reasoning: Option<Reasoning>,
    /// A reasoning override for the next turn only.
    pub(super) pending_reasoning: Option<Reasoning>,
    /// A budget override for the next turn only.
    pub(super) pending_budget: Option<Budget>,
    /// Turn ids pinned verbatim through compaction, in the wire's spelling
    /// (`turn_2`). Persisted as `pinned_turn_ids` in the stored stats.
    pub(super) pinned_turn_ids: HashSet<String>,
    /// Messages queued for the next turn boundary, in arrival order.
    pub(super) pending_steers: Vec<Steered>,
    /// The parent session this one was spawned from (`spawn_agent`).
    ///
    /// `None` for a session created directly by the client. Persisted in
    /// `stats` so a reload still knows who a child's parent is; `list_agents`
    /// finds a parent's children by it.
    pub(super) parent_client: Option<SessionKey>,
    /// The tool call that created this child.
    pub(super) parent_tool_call_id: Option<String>,
    /// Terminal delegation state, if the child has ended.
    pub(super) child_terminal: Option<ChildTerminalState>,
    /// The delegation depth: 0 for a session the client made, one more than
    /// its parent's for a child (RFC 8.2.4). Anidación se limita con este
    /// número, no cortando la tool `spawn_agent` del hijo.
    pub(super) depth: u32,
    /// Whether the delegator (the human or the parent driving this child) is
    /// suspended (RFC 8.2.11): held messages wait and `stop_agent` fails.
    pub(super) delegator_suspended: bool,
    /// Messages the parent sent while the delegator was suspended, held
    /// until it is released.
    pub(super) delegator_queue: Vec<String>,
}

/// Builds the record of a stored session.
pub(super) fn record_from(
    entry: StoredSession,
    spec: Option<ProviderModelSpec>,
    phase: ProtocolPhase,
) -> SessionRecord {
    let stats = StoredStats::read(&entry.stats).unwrap_or_else(|error| {
        tracing::warn!(
            session = %entry.key,
            %error,
            "stored session stats are not readable; starting from zero"
        );
        StoredStats::default()
    });
    SessionRecord {
        id: entry.north,
        title: entry.title,
        spec,
        agent_profile: stats.agent_profile,
        pending_provider: stats.pending_provider,
        pending_model: stats.pending_model,
        created_at: entry.created_at,
        phase,
        billed: stats.billed,
        requests: stats.requests,
        cost: stats.cost,
        unpriced: stats.unpriced,
        input_tokens: stats.input_tokens,
        context_used: stats.context_used,
        last_turn: stats.last_turn,
        turn_boundaries: stats.turn_boundaries,
        rewound_history: stats.rewound_history.into_iter().map(Into::into).collect(),
        imported_from: stats.imported_from,
        imported_suspended: stats.imported_suspended,
        output_tokens: stats.output_tokens,
        cache_read_tokens: stats.cache_read_tokens,
        cache_write_tokens: stats.cache_write_tokens,
        budget: stats.budget,
        disabled_tools: stats.disabled_tools,
        denied_tools: entry.denied_tools,
        allowed_tools: entry.allowed_tools,
        compacted_from: entry.compacted_from,
        compacting: false,
        compaction_baseline: stats.compaction_baseline,
        reasoning: stats.reasoning,
        pending_reasoning: stats.pending_reasoning,
        pending_budget: stats.pending_budget,
        pinned_turn_ids: stats.pinned_turn_ids,
        pending_steers: stats.pending_steers,
        parent_client: stats.parent_client,
        parent_tool_call_id: stats.parent_tool_call_id,
        child_terminal: stats.child_state,
        depth: stats.depth,
        delegator_suspended: stats.delegator_suspended,
        delegator_queue: stats.delegator_queue,
    }
}

pub(super) fn imported_history_nodes(
    session_id: norte::SessionId,
    mut parent: Option<norte::NodeId>,
    items: Vec<super::ImportedHistoryItem>,
) -> (
    Vec<crate::runtime::Node>,
    Option<norte::NodeId>,
    Vec<TurnBoundary>,
    u64,
) {
    let mut nodes = Vec::with_capacity(items.len());
    let mut boundaries = Vec::new();
    let mut current_source_turn: Option<String> = None;
    let mut current_turn = 0_u64;
    let mut current_turn_head = None;
    for (index, item) in items.into_iter().enumerate() {
        let source_turn = item.turn_id.clone().unwrap_or_else(|| {
            if item.role == "user" || current_source_turn.is_none() {
                format!("imported-{index}")
            } else {
                current_source_turn.clone().unwrap_or_default()
            }
        });
        if current_source_turn.as_deref() != Some(source_turn.as_str()) {
            if let Some(head) = current_turn_head.take() {
                boundaries.push(TurnBoundary {
                    turn: current_turn,
                    session: session_id,
                    head: Some(head),
                });
            }
            current_turn = current_turn.saturating_add(1);
            current_source_turn = Some(source_turn);
        }
        let message = if item.role == "user" {
            norte::Message::user(item.content)
        } else {
            norte::Message::assistant(item.content)
        };
        let mut node = crate::runtime::Node::message(message, parent);
        node.metadata.insert(
            "provenance".to_owned(),
            serde_json::json!({
                "kind": "imported",
                "origin": item.origin,
                "source_timestamp": item.timestamp,
            }),
        );
        if let Some(timestamp) = item.timestamp
            && let Some(created_at) = DateTime::from_timestamp_millis(timestamp)
        {
            node.created_at = created_at;
        }
        parent = Some(node.id);
        current_turn_head = Some(node.id);
        nodes.push(node);
    }
    if let Some(head) = current_turn_head {
        boundaries.push(TurnBoundary {
            turn: current_turn,
            session: session_id,
            head: Some(head),
        });
    }
    (nodes, parent, boundaries, current_turn)
}
