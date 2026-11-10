//! The session record and the lifecycle around it.
//!
//! A session is the durable half of the service: what it is, what it has
//! cost, what it is allowed to do and what it still owes the model. The turn
//! machinery lives in the parent module; this one owns the state the turns
//! read and write.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};

use norte_code_protocol::costs::BudgetBook;
use norte_code_wire::{
    Budget, Phase as ProtocolPhase, Reasoning, ReasoningScope, ServerEvent, ServerEventKind,
    SessionInfo, SessionKey,
};

use crate::provider::ModelSpec;

use super::EventStream;

use super::Steered;
use super::mapping::to_phase;
use super::{Service, ServiceError, Stats, StoredSession};
use crate::provider::ModelSpec as ProviderModelSpec;

const MAX_HELD_DELEGATOR_MESSAGES: usize = 20;

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
#[allow(clippy::too_many_lines)]
fn record_from(
    entry: StoredSession,
    spec: Option<ProviderModelSpec>,
    phase: ProtocolPhase,
) -> SessionRecord {
    let stats = &entry.stats;
    let number = |key: &str| stats.get(key).and_then(serde_json::Value::as_u64);
    SessionRecord {
        id: entry.north,
        title: entry.title,
        spec,
        agent_profile: stat(stats, "agent_profile"),
        pending_provider: stat(stats, "pending_provider"),
        pending_model: stat(stats, "pending_model"),
        created_at: entry.created_at,
        phase,
        billed: number("billed")
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0),
        requests: number("requests").unwrap_or(0),
        cost: stats
            .get("cost")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0),
        unpriced: number("unpriced").unwrap_or(0),
        input_tokens: number("input_tokens").unwrap_or(0),
        // A session stored before the context was kept has none, and the
        // footer falls back to what it can measure on its own.
        context_used: number("context_used").unwrap_or(0),
        last_turn: number("last_turn").unwrap_or(0),
        turn_boundaries: stat(stats, "turn_boundaries").unwrap_or_default(),
        rewound_history: stat(stats, "rewound_history").unwrap_or_default(),
        imported_from: stat(stats, "imported_from"),
        imported_suspended: stats
            .get("imported_suspended")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        output_tokens: number("output_tokens").unwrap_or(0),
        cache_read_tokens: number("cache_read_tokens").unwrap_or(0),
        cache_write_tokens: number("cache_write_tokens").unwrap_or(0),
        budget: stat(stats, "budget"),
        // A session stored before this existed has no list, and an absent list
        // withholds nothing: it keeps every tool it was made with, which is the
        // same rule the field follows everywhere else.
        disabled_tools: stats
            .get("disabled_tools")
            .and_then(serde_json::Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        denied_tools: entry.denied_tools.clone(),
        allowed_tools: entry.allowed_tools.clone(),
        compacted_from: entry.compacted_from,
        compacting: false,
        compaction_baseline: number("compaction_baseline")
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0),
        reasoning: stat(stats, "reasoning"),
        pending_reasoning: stat(stats, "pending_reasoning"),
        pending_budget: stat(stats, "pending_budget"),
        pinned_turn_ids: stats
            .get("pinned_turn_ids")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        parent_client: stat(stats, "parent_client"),
        parent_tool_call_id: stat(stats, "parent_tool_call_id"),
        child_terminal: stats
            .get("child_state")
            .and_then(|state| serde_json::from_value::<ChildTerminalState>(state.clone()).ok())
            .or_else(|| {
                stats
                    .get("child_terminal")
                    .and_then(serde_json::Value::as_bool)
                    .filter(|terminal| *terminal)
                    .map(|_| {
                        if stats
                            .get("child_failed")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                        {
                            ChildTerminalState::Failed
                        } else {
                            ChildTerminalState::Stopped
                        }
                    })
            }),
        pending_steers: stat(stats, "pending_steers")
            .map(|steers: Vec<Steered>| {
                // A message stored before the names existed has none, and two
                // of them would look like the same one.
                steers
                    .into_iter()
                    .map(|steered| {
                        if steered.id.is_empty() {
                            steered.with_id(uuid::Uuid::new_v4().to_string())
                        } else {
                            steered
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        depth: number("depth")
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0),
        delegator_suspended: stats
            .get("delegator_suspended")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        delegator_queue: stat(stats, "delegator_queue").unwrap_or_default(),
    }
}

/// One value of the stored statistics.
fn stat<T: serde::de::DeserializeOwned>(stats: &serde_json::Value, key: &str) -> Option<T> {
    stats
        .get(key)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

fn imported_history_nodes(
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

impl Service {
    /// Reads the sessions of the directory and repairs the ones a hard kill
    /// left behind.
    pub(super) async fn load_sessions(&self) -> Result<(), ServiceError> {
        let stored = self.directory.sessions().await?;
        let catalog = self.catalog();
        let mut records = Vec::with_capacity(stored.len());
        let mut repaired = Vec::new();
        for entry in stored {
            let key = SessionKey::new(entry.key.clone());
            self.plans
                .ensure_session_folder(&key)
                .await
                .map_err(ServiceError::Plan)?;
            let spec =
                // A session saved on a provider the catalogue does not list has
                // to reopen on it: going straight to the catalogue dropped every
                // such session onto the default model, so a `exa` session came
                // back as whatever the config's default was.
                match self.default_spec_for(
                    &self.lock_config(),
                    &catalog,
                    &entry.provider,
                    &entry.model,
                ) {
                    Ok(spec) => spec,
                    Err(error) => {
                        tracing::warn!(
                            session = %entry.key,
                            provider = %entry.provider,
                            model = %entry.model,
                            %error,
                            "stored session model is unavailable; skipping"
                        );
                        continue;
                    }
                };
            let (phase, needs_repair) = self.read_phase(&entry).await;
            self.approval_rules.load_session(
                entry.north,
                &entry.denied_tools,
                &entry.allowed_tools,
            );
            let record = record_from(entry, Some(spec), phase);
            records.push((key.clone(), record));
            if needs_repair {
                repaired.push(key);
            }
        }
        {
            let mut sessions = self.lock_sessions();
            for (key, record) in records {
                sessions.insert(key, record);
            }
        }
        for key in &repaired {
            self.persist(key).await?;
        }
        self.restore_child_registry();
        Ok(())
    }

    /// Puts a session no turn is streaming any more back to `Idle`.
    ///
    /// `Phase::Streaming` means "a turn is in flight": the engine commits it
    /// before it asks the model, and only a committed answer lowers it. A turn
    /// that ends without one: the provider failed with the retries spent, or
    /// the process was killed: leaves the phase behind, and nothing visible
    /// breaks, which is what makes it hard to see: the next turn starts from
    /// `Idle | Streaming` either way. What does break is that `/model` and
    /// compaction apply only at an idle boundary, so every later switch queues
    /// behind a boundary the session can no longer reach.
    ///
    /// Best effort: a store that cannot be written is logged and not raised,
    /// because the caller is already holding the failure this is called after.
    /// Returns whether the session was actually settled.
    pub(super) async fn settle_streaming_to_idle(&self, nid: norte::SessionId) -> bool {
        let Ok(session) = self.store.load(nid).await else {
            return false;
        };
        if session.phase != crate::runtime::Phase::Streaming {
            return false;
        }
        let commit = crate::runtime::Commit {
            session_id: nid,
            expected_version: Some(session.version),
            nodes: Vec::new(),
            // No nodes, so no head: `Commit::validate` requires a head to be one
            // of the commit's own nodes, and `None` means "leave it where it
            // is", which is what this wants: settling a phase does not move
            // the conversation.
            head: None,
            phase: crate::runtime::Phase::Idle,
            turn: session.turn,
            metadata: None,
        };
        match self.store.commit(commit).await {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(
                    session = %nid,
                    %error,
                    "could not settle a session whose turn ended to idle"
                );
                false
            }
        }
    }

    /// The phase the session is in, and whether it needs a repair.
    ///
    /// Norte's store is the authority on phase: a parked tool or approval
    /// committed it, while the directory snapshot may predate the park.
    /// `streaming` is the exception: no process is streaming a session it just
    /// loaded, so a hard kill left it behind. Put the store back to idle and
    /// rewrite the directory row.
    async fn read_phase(&self, entry: &StoredSession) -> (ProtocolPhase, bool) {
        let stored_phase = entry.phase;
        let phase = match self.store.load(entry.north).await {
            Ok(session) if session.phase == crate::runtime::Phase::Streaming => {
                // The store outlived the streaming that wrote it, so the turn
                // is not coming back in this process: settle it and tell the
                // caller to rewrite the directory row from the same fact.
                let _ = self.settle_streaming_to_idle(entry.north).await;
                return (ProtocolPhase::Idle, true);
            }
            Ok(session) => to_phase(session.phase),
            Err(error) => {
                tracing::warn!(
                    session = %entry.key,
                    %error,
                    "could not read the session phase; using the directory copy"
                );
                match stored_phase {
                    ProtocolPhase::AwaitingTool => ProtocolPhase::AwaitingTool,
                    _ => ProtocolPhase::Idle,
                }
            }
        };
        // A directory row that says `streaming` is stale whether or not the
        // store agreed, so it is rewritten either way.
        (phase, stored_phase == ProtocolPhase::Streaming)
    }

    /// Writes one session back to the directory.
    pub(super) async fn persist(&self, key: &SessionKey) -> Result<(), ServiceError> {
        let stored = self.lock_sessions().get(key).map(|record| StoredSession {
            key: key.to_string(),
            north: record.id,
            title: record.title.clone(),
            model: record
                .spec
                .as_ref()
                .map(|s| s.model.clone())
                .unwrap_or_default(),
            provider: record
                .spec
                .as_ref()
                .map(|s| s.provider.clone())
                .unwrap_or_default(),
            created_at: record.created_at,
            phase: record.phase,
            stats: serde_json::json!({
                "billed": record.billed,
                "requests": record.requests,
                "cost": record.cost,
                "unpriced": record.unpriced,
                "input_tokens": record.input_tokens,
                "context_used": record.context_used,
                "last_turn": record.last_turn,
                "turn_boundaries": record.turn_boundaries,
                "rewound_history": record.rewound_history,
                "imported_from": record.imported_from,
                "imported_suspended": record.imported_suspended,
                "output_tokens": record.output_tokens,
                "cache_read_tokens": record.cache_read_tokens,
                "cache_write_tokens": record.cache_write_tokens,
                "budget": record.budget,
                "agent_profile": record.agent_profile,
                "compaction_baseline": record.compaction_baseline,
                "reasoning": record.reasoning,
                "pending_reasoning": record.pending_reasoning,
                "pending_steers": record.pending_steers,
                "pending_provider": record.pending_provider,
                "pending_model": record.pending_model,
                "disabled_tools": record.disabled_tools,
                "parent_client": record.parent_client,
                "parent_tool_call_id": record.parent_tool_call_id,
                "child_state": record.child_terminal,
                "depth": record.depth,
                "delegator_suspended": record.delegator_suspended,
                "delegator_queue": record.delegator_queue,
            }),
            denied_tools: record.denied_tools.clone(),
            allowed_tools: record.allowed_tools.clone(),
            compacted_from: record.compacted_from,
            // Where the process is working, which is where the tools read and
            // write: a session is about a tree, and resuming it from another
            // directory would point every tool at the wrong one.
            workdir: Some(self.workspace_root.display().to_string()),
        });
        if let Some(stored) = stored {
            self.plans
                .ensure_session_folder(&SessionKey::new(stored.key.clone()))
                .await
                .map_err(ServiceError::Plan)?;
            self.directory.save_session(&stored).await?;
        }
        Ok(())
    }

    /// Starts a new session with the default model and the system prompt.
    pub(super) async fn create_session(
        &self,
        title: Option<String>,
    ) -> Result<SessionKey, ServiceError> {
        let spec = self
            .default_spec
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        self.create_session_with_settings(
            title,
            &self.system_prompt,
            spec,
            self.configured_disabled_tools(),
            Some(self.config().name),
        )
        .await
    }

    /// Creates a new, suspended session containing imported plain-text
    /// user/assistant history.
    ///
    /// Imported items retain their roles and carry provenance in node
    /// metadata. System prompts, tool calls, and media are intentionally not
    /// accepted by this path.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported roles, oversized input, or failures
    /// creating/persisting the session history.
    pub async fn import_plain_history(
        &self,
        title: Option<String>,
        items: Vec<super::ImportedHistoryItem>,
        imported_from: serde_json::Value,
        usage_total: Option<serde_json::Value>,
    ) -> Result<SessionKey, ServiceError> {
        const MAX_ITEMS: usize = 100_000;
        const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;
        let text_bytes = items
            .iter()
            .try_fold(0_usize, |total, item| total.checked_add(item.content.len()));
        if items.len() > MAX_ITEMS || text_bytes.is_none_or(|size| size > MAX_TEXT_BYTES) {
            return Err(ServiceError::History(
                "import history exceeds the item or text limit".to_owned(),
            ));
        }
        if items
            .iter()
            .any(|item| item.role != "user" && item.role != "assistant")
        {
            return Err(ServiceError::History(
                "import history supports only user and assistant roles".to_owned(),
            ));
        }

        let key = self.create_session(title).await?;
        let session_id = self
            .lock_sessions()
            .get(&key)
            .map(|record| record.id)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        let current = self.store.load(session_id).await?;
        let (nodes, parent, boundaries, current_turn) =
            imported_history_nodes(session_id, current.head, items);
        // The commit's head must be one of its own nodes. A bundle that
        // carries no history has nothing to commit, and committing an empty
        // node list with the session's existing head is rejected by the store
        //: so an export without messages could never be imported back.
        let existing_messages = match current.head {
            Some(head) => self.store.ancestors(head).await?.len(),
            None => 0,
        };
        let billed = existing_messages.saturating_add(nodes.len());
        if !nodes.is_empty() {
            self.store
                .commit(crate::runtime::Commit {
                    session_id,
                    expected_version: Some(current.version),
                    nodes,
                    head: parent,
                    phase: crate::runtime::Phase::Idle,
                    turn: current.turn,
                    metadata: None,
                })
                .await?;
        }
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.turn_boundaries = boundaries;
            record.last_turn = current_turn;
            record.imported_from = Some(imported_from);
            record.imported_suspended = true;
            record.billed = billed;
            if let Some(usage) = usage_total {
                record.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
                record.output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
                record.cache_read_tokens = usage["cached_tokens"].as_u64().unwrap_or(0);
                record.cost = usage["cost"]["amount"].as_f64().unwrap_or(0.0);
                record.requests = usage["extra"]["requests"].as_u64().unwrap_or(0);
                record.unpriced = usage["extra"]["unpriced_requests"].as_u64().unwrap_or(0);
            }
        }
        self.persist(&key).await?;
        Ok(key)
    }

    /// Returns the usage counters in the portable RAEP usage shape.
    #[must_use]
    pub fn session_usage_total(&self, key: &SessionKey) -> Option<serde_json::Value> {
        let sessions = self.lock_sessions();
        let record = sessions.get(key)?;
        Some(serde_json::json!({
            "input_tokens": record.input_tokens,
            "output_tokens": record.output_tokens,
            "cached_tokens": record.cache_read_tokens.saturating_add(record.cache_write_tokens),
            "cost": {
                "amount": record.cost.max(0.0),
                "currency": "USD",
            },
            "extra": {
                "requests": record.requests,
                "unpriced_requests": record.unpriced,
            },
        }))
    }

    /// Returns importer provenance stored with a portable session.
    #[must_use]
    pub fn session_imported_from(&self, key: &SessionKey) -> Option<serde_json::Value> {
        self.lock_sessions()
            .get(key)
            .and_then(|record| record.imported_from.clone())
    }

    /// Whether a portable session is waiting for its first controller attach.
    #[must_use]
    pub fn session_imported_suspended(&self, key: &SessionKey) -> bool {
        self.lock_sessions()
            .get(key)
            .is_some_and(|record| record.imported_suspended)
    }

    /// Activates an imported session when its first controller attaches.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is unknown or its directory record
    /// cannot be updated.
    #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
    pub(super) async fn export_session(
        &self,
        _request_id: u64,
        _session: &SessionKey,
        _include: Option<norte_code_wire::ExportInclude>,
        _quiesce: Option<norte_code_wire::ExportQuiesce>,
    ) -> Result<crate::EventStream, crate::ServiceError> {
        Err(crate::ServiceError::Unsupported(
            "session export not implemented",
        ))
    }

    #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
    pub(super) async fn import_session(
        &self,
        _request_id: u64,
        _bundle: serde_json::Value,
        _decryption_jwk: Option<serde_json::Value>,
    ) -> Result<crate::EventStream, crate::ServiceError> {
        Err(crate::ServiceError::Unsupported(
            "session import not implemented",
        ))
    }

    /// Returns whether the imported session was activated.
    ///
    /// # Errors
    /// Returns an error when the session is unknown.
    pub async fn activate_imported_session(&self, key: &SessionKey) -> Result<bool, ServiceError> {
        let was_suspended = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            let was_suspended = record.imported_suspended;
            record.imported_suspended = false;
            was_suspended
        };
        if was_suspended {
            self.persist(key).await?;
        }
        Ok(was_suspended)
    }

    #[allow(clippy::too_many_lines)]
    async fn create_session_with_settings(
        &self,
        title: Option<String>,
        system_prompt: &str,
        stored_spec: Option<ProviderModelSpec>,
        disabled_tools: Vec<String>,
        agent_profile: Option<String>,
    ) -> Result<SessionKey, ServiceError> {
        // A run with no configured model still needs an engine to create the
        // session, so the first catalogue model is borrowed for that alone. The
        // session itself carries no spec (`None`): it is not a model the person
        // chose, and a message sent on it is refused until one is.
        let engine_spec = stored_spec.clone().unwrap_or_else(|| {
            // Fallback for a session created without a configured model: borrow
            // the first catalogue entry to build the engine, never to serve a
            // request.
            let catalog = self.catalog();
            let provider = catalog
                .providers()
                .keys()
                .next()
                .cloned()
                .unwrap_or_default();
            let model = catalog
                .models_of(&provider)
                .next()
                .map(|m| m.id.clone())
                .unwrap_or_default();
            // Best effort: resolve through the catalogue; if it fails the engine
            // build below will surface the error.
            self.default_spec_for(&self.lock_config(), &catalog, &provider, &model)
                .unwrap_or_else(|_| ProviderModelSpec {
                    provider: provider.clone(),
                    model: model.clone(),
                    endpoint: String::new(),
                    api_key: norte_code_protocol::config::ApiKey::new(String::new()),
                    schema: norte_code_wire::catalog::WireSchema::OpenAI,
                    auth: None,
                    capabilities: norte_code_protocol::catalog::capabilities(
                        &norte_code_wire::catalog::Model::default(),
                        norte_code_wire::catalog::WireSchema::OpenAI,
                    ),
                    headers: std::collections::BTreeMap::new(),
                })
        });
        // A session can outlive the rest of the engine: it is nothing but a
        // row and a system message. When no model is configured the borrowed
        // spec cannot build an engine, so fall back to a direct store commit
        // and there is no history appended.
        let engine = self.engine(&engine_spec, None).ok();
        let session = if let Some(engine) = &engine {
            let session = engine.create().await?;
            let _ = engine
                .append_history(
                    session.id,
                    vec![norte::Message::system(system_prompt.to_owned())],
                )
                .await;
            session
        } else {
            let session = crate::runtime::Session::new();
            let _ = self
                .store
                .commit(crate::runtime::Commit {
                    session_id: session.id,
                    expected_version: None,
                    nodes: Vec::new(),
                    head: None,
                    phase: crate::runtime::Phase::Idle,
                    turn: 0,
                    metadata: None,
                })
                .await;
            session
        };
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let record = SessionRecord {
            id: session.id,
            title,
            spec: stored_spec,
            pending_provider: None,
            pending_model: None,
            created_at: Utc::now(),
            phase: ProtocolPhase::Idle,
            billed: 0,
            requests: 0,
            cost: 0.0,
            unpriced: 0,
            input_tokens: 0,
            context_used: 0,
            last_turn: 0,
            turn_boundaries: Vec::new(),
            rewound_history: Vec::new(),
            imported_from: None,
            imported_suspended: false,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            budget: self.configured_session_budget(),
            // Frozen here, at the one moment the prefix is not yet cached: the
            // tool declarations are part of it, so the set a session starts
            // with is the set it keeps.
            disabled_tools,
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            compacted_from: None,
            compacting: false,
            compaction_baseline: 0,
            reasoning: None,
            pending_reasoning: None,
            pending_budget: None,
            pinned_turn_ids: std::collections::HashSet::new(),
            pending_steers: Vec::new(),
            parent_client: None,
            parent_tool_call_id: None,
            child_terminal: None,
            agent_profile,
            depth: 0,
            delegator_suspended: false,
            delegator_queue: Vec::new(),
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
        Ok(key)
    }

    /// Copies a session into a new one, under a new key.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn fork_session(
        &self,
        from: &SessionKey,
        title: Option<String>,
        from_turn_id: Option<&str>,
        inclusive: bool,
        carry_usage: bool,
    ) -> Result<(SessionKey, Option<String>), ServiceError> {
        let (
            source,
            spec,
            agent_profile,
            disabled_tools,
            last_turn,
            boundaries,
            source_phase,
            usage,
            budget,
        ) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(from)
                .ok_or_else(|| ServiceError::UnknownSession(from.to_string()))?;
            (
                record.id,
                record.spec.clone(),
                record.agent_profile.clone(),
                record.disabled_tools.clone(),
                record.last_turn,
                record.turn_boundaries.clone(),
                record.phase,
                (
                    record.billed,
                    record.requests,
                    record.cost,
                    record.unpriced,
                    record.input_tokens,
                    record.context_used,
                    record.output_tokens,
                    record.cache_read_tokens,
                    record.cache_write_tokens,
                ),
                record.budget.clone(),
            )
        };
        let spec = spec.ok_or_else(|| ServiceError::UnknownSession(from.to_string()))?;
        let requested_turn = match from_turn_id {
            Some(id) => Some(
                id.strip_prefix("turn_")
                    .unwrap_or(id)
                    .parse::<u64>()
                    .map_err(|_| ServiceError::History("TURN_NOT_FOUND".to_owned()))?,
            ),
            None => boundaries.iter().map(|boundary| boundary.turn).max(),
        };
        let requested_turn = requested_turn.ok_or_else(|| {
            ServiceError::History("TURN_NOT_FOUND: no terminal turn is available".to_owned())
        })?;
        let target = boundaries
            .iter()
            .find(|boundary| boundary.turn == requested_turn)
            .ok_or_else(|| {
                if requested_turn > last_turn && !matches!(source_phase, ProtocolPhase::Idle) {
                    ServiceError::History("TURN_NOT_ACTIVE".to_owned())
                } else {
                    ServiceError::History("TURN_NOT_FOUND".to_owned())
                }
            })?;
        let included_boundary = if inclusive {
            Some(target)
        } else {
            boundaries
                .iter()
                .filter(|boundary| boundary.turn < requested_turn)
                .max_by_key(|boundary| boundary.turn)
        };
        let effective_turn = included_boundary.map_or(0, |boundary| boundary.turn);
        let first_boundary = boundaries.iter().min_by_key(|boundary| boundary.turn);
        let source_session = included_boundary
            .or(first_boundary)
            .map_or(source, |boundary| boundary.session);
        let source_state = self.store.load(source_session).await?;
        let mut source_nodes = if let Some(head) = included_boundary.and_then(|b| b.head) {
            self.store.ancestors(head).await?
        } else if let Some(head) = first_boundary.and_then(|boundary| boundary.head) {
            self.store
                .ancestors(head)
                .await?
                .into_iter()
                .take(1)
                .collect()
        } else {
            Vec::new()
        };
        let new_id = norte::SessionId::new();
        let mut parent = None;
        let mut nodes = Vec::with_capacity(source_nodes.len());
        let mut remapped = Vec::with_capacity(source_nodes.len());
        for old_id in source_nodes.drain(..) {
            let old_node = self.store.node(old_id).await?;
            let node = crate::runtime::Node::message(old_node.message, parent);
            parent = Some(node.id);
            remapped.push((old_id, node.id));
            nodes.push(node);
        }
        let turn_boundaries = boundaries
            .iter()
            .filter(|boundary| boundary.turn <= effective_turn)
            .filter_map(|boundary| {
                let head = boundary.head?;
                let new_head = remapped
                    .iter()
                    .find_map(|(old, new)| (*old == head).then_some(*new))?;
                Some(TurnBoundary {
                    turn: boundary.turn,
                    session: new_id,
                    head: Some(new_head),
                })
            })
            .collect();
        let imported = self
            .store
            .commit(crate::runtime::Commit {
                session_id: new_id,
                expected_version: None,
                nodes,
                head: parent,
                phase: crate::runtime::Phase::Idle,
                turn: effective_turn,
                metadata: Some(source_state.metadata),
            })
            .await
            .map_err(ServiceError::from)?;
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let (
            billed,
            requests,
            cost,
            unpriced,
            input_tokens,
            context_used,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        ) = if carry_usage {
            usage
        } else {
            (0, 0, 0.0, 0, 0, 0, 0, 0, 0)
        };
        let record = SessionRecord {
            id: imported.id,
            title,
            spec: Some(spec),
            agent_profile,
            pending_provider: None,
            pending_model: None,
            created_at: Utc::now(),
            phase: ProtocolPhase::Idle,
            billed,
            requests,
            cost,
            unpriced,
            input_tokens,
            context_used,
            last_turn: effective_turn,
            turn_boundaries,
            rewound_history: Vec::new(),
            imported_from: None,
            imported_suspended: false,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            budget: if carry_usage {
                budget
            } else {
                self.configured_session_budget()
            },
            // A fork copies the conversation, so it copies the tool set the
            // conversation was held in: a branch that quietly gained a tool the
            // source did not have would answer the same history differently.
            disabled_tools,
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            // A fork is a new branch, not a compaction of the source.
            compacted_from: None,
            compacting: false,
            compaction_baseline: 0,
            reasoning: None,
            pending_reasoning: None,
            pending_budget: None,
            pinned_turn_ids: std::collections::HashSet::new(),
            pending_steers: Vec::new(),
            parent_client: None,
            parent_tool_call_id: None,
            child_terminal: None,
            depth: 0,
            delegator_suspended: false,
            delegator_queue: Vec::new(),
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
        self.plans
            .copy_session(source, imported.id)
            .await
            .map_err(ServiceError::Plan)?;
        Ok((key, Some(format!("turn_{requested_turn}"))))
    }

    /// Moves a session's active head to a terminal turn while retaining the
    /// removed text history for explicit `include_rewound` reads.
    ///
    /// # Errors
    ///
    /// Returns an error when the turn is unknown, the session is blocked, the
    /// turn contains a tool call whose side effects cannot be reported, or
    /// durable storage fails.
    #[allow(clippy::too_many_lines)]
    pub async fn rewind_session(
        &self,
        key: &SessionKey,
        to_turn_id: &str,
        inclusive: bool,
        cancel_pending: bool,
        restore_workspace: bool,
    ) -> Result<Vec<String>, ServiceError> {
        if restore_workspace {
            return Err(ServiceError::History("SNAPSHOT_NOT_FOUND".to_owned()));
        }
        let to_turn = to_turn_id
            .strip_prefix("turn_")
            .unwrap_or(to_turn_id)
            .parse::<u64>()
            .map_err(|_| ServiceError::History("TURN_NOT_FOUND".to_owned()))?;
        let (session_id, phase, boundaries) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (record.id, record.phase, record.turn_boundaries.clone())
        };
        if phase != ProtocolPhase::Idle {
            if !cancel_pending || !self.cancel_turn(key) {
                return Err(ServiceError::History(
                    "REWIND_BLOCKED: active turn".to_owned(),
                ));
            }
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if self
                        .require_record(key)
                        .is_ok_and(|(_, _, phase)| phase == ProtocolPhase::Idle)
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| {
                ServiceError::History("REWIND_BLOCKED: turn cancellation timed out".to_owned())
            })?;
        }
        if self.jobs.any_running(session_id) {
            if !cancel_pending {
                return Err(ServiceError::History(
                    "REWIND_BLOCKED: running jobs".to_owned(),
                ));
            }
            self.jobs.cancel_session(session_id);
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while self.jobs.any_running(session_id) {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| {
                ServiceError::History("REWIND_BLOCKED: job cancellation timed out".to_owned())
            })?;
        }
        if self
            .children
            .of(key)
            .iter()
            .any(|child| !child.is_terminal())
        {
            return Err(ServiceError::History(
                "REWIND_BLOCKED: non-terminal child session".to_owned(),
            ));
        }
        let target = boundaries
            .iter()
            .find(|boundary| boundary.turn == to_turn)
            .ok_or_else(|| ServiceError::History("TURN_NOT_FOUND".to_owned()))?;
        let rewound: Vec<_> = boundaries
            .iter()
            .filter(|boundary| boundary.turn > to_turn || (inclusive && boundary.turn == to_turn))
            .cloned()
            .collect();
        if rewound.is_empty() {
            return Ok(Vec::new());
        }
        let rewound_ids: Vec<String> = rewound
            .iter()
            .map(|boundary| format!("turn_{}", boundary.turn))
            .collect();
        let history = self.session_history_with_turns(key).await?;
        let archived: Vec<_> = history
            .into_iter()
            .filter_map(|(turn, role, kind, text, timestamp)| {
                turn.filter(|turn| rewound_ids.contains(&format!("turn_{turn}")))
                    .map(|turn| (turn, role, kind, text, timestamp))
            })
            .collect();
        // Tool calls in the removed turns are reported, not refused.
        //
        // Refusing forever was the old rule, on the theory that a tool's
        // effects cannot be undone and the person should not silently lose
        // them. But no tool declares its side effects yet, so the rule blocked
        // every rewind that crossed a tool call and nothing recorded what was
        // skipped. The rewind now proceeds and names the calls it removed; the
        // host passes them back in `HISTORY_REWOUND.side_effects` so the reader
        // sees what happened outside the conversation: a file written, a
        // command run: and can undo it themselves.
        let kept_turn = boundaries
            .iter()
            .filter(|boundary| boundary.turn < to_turn || (!inclusive && boundary.turn == to_turn))
            .map(|boundary| boundary.turn)
            .max()
            .unwrap_or(0);
        let kept = boundaries
            .iter()
            .find(|boundary| boundary.turn == kept_turn);
        let source_head = kept.and_then(|boundary| boundary.head).or(target.head);
        let mut old_ids = match source_head {
            Some(head) => self.store.ancestors(head).await?,
            None => Vec::new(),
        };
        if kept.is_none() {
            old_ids.truncate(1);
        }
        let mut parent = None;
        let mut nodes = Vec::with_capacity(old_ids.len());
        let mut remapped = Vec::with_capacity(old_ids.len());
        for old_id in old_ids {
            let old = self.store.node(old_id).await?;
            let node = crate::runtime::Node::message(old.message, parent);
            parent = Some(node.id);
            remapped.push((old_id, node.id));
            nodes.push(node);
        }
        let current = self.store.load(session_id).await?;
        self.store
            .commit(crate::runtime::Commit {
                session_id,
                expected_version: Some(current.version),
                nodes,
                head: parent,
                phase: crate::runtime::Phase::Idle,
                turn: kept_turn,
                metadata: Some(current.metadata),
            })
            .await?;
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.turn_boundaries = boundaries
                .iter()
                .filter(|boundary| boundary.turn <= kept_turn)
                .filter_map(|boundary| {
                    let old_head = boundary.head?;
                    let head = remapped
                        .iter()
                        .find_map(|(old, new)| (*old == old_head).then_some(*new))?;
                    Some(TurnBoundary {
                        turn: boundary.turn,
                        session: session_id,
                        head: Some(head),
                    })
                })
                .collect();
            record.rewound_history.extend(archived);
            record.compacted_from = None;
        }
        self.persist(key).await?;
        Ok(rewound_ids)
    }

    /// The tool calls that ran in the turns `turn_ids`, as `(tool_call_id,
    /// tool_name, arguments)`.
    ///
    /// A rewind archives the text of the turns it removes; this answers which
    /// tools ran there so the client can tell the person what happened outside
    /// the conversation. Nothing here claims to know the effects themselves -
    /// no tool declares them yet: only that the call happened.
    #[must_use]
    pub fn rewound_tool_calls(
        &self,
        key: &SessionKey,
        turn_ids: &[String],
    ) -> Vec<norte_code_wire::SideEffect> {
        let sessions = self.lock_sessions();
        let Some(record) = sessions.get(key) else {
            return Vec::new();
        };
        record
            .rewound_history
            .iter()
            .filter(|(turn, _, kind, _, _)| {
                *kind == norte_code_wire::ReplayedKind::ToolCall
                    && turn_ids.contains(&format!("turn_{turn}"))
            })
            .map(|(_, _, _, text, _)| {
                let (name, arguments) = text.split_once(' ').unwrap_or((text.as_str(), ""));
                norte_code_wire::SideEffect {
                    tool_call_id: String::new(),
                    tool_name: name.trim().to_owned(),
                    side_effects: arguments.trim().to_owned(),
                }
            })
            .collect()
    }

    /// Creates a child session for a subagent with frozen settings.
    ///
    /// The child keeps the delegation tools: nesting is bounded by `depth`
    /// (RFC 8.2.4) against [`MAX_DELEGATION_DEPTH`], not by cutting the tool
    /// from the child. The `parent` is kept on the record (`parent_client`)
    /// so `list_agents` finds a parent's children; the RAEP `spawn_agent`
    /// tool is the same tool, so its children are kept the same way.
    pub(super) async fn create_session_child(
        &self,
        metadata: ChildSessionMetadata,
        spec: ModelSpec,
        disabled_tools: Vec<String>,
        budget: Arc<Mutex<BudgetBook>>,
    ) -> Result<SessionKey, ServiceError> {
        let ChildSessionMetadata {
            parent,
            tool_call_id,
            task_summary,
            lifetime,
            agent_profile,
            system_prompt,
            cancel,
        } = metadata;
        let key = self
            .create_session_with_settings(
                None,
                &system_prompt,
                Some(spec.clone()),
                disabled_tools.clone(),
                Some(agent_profile.clone()),
            )
            .await?;
        {
            let mut sessions = self.lock_sessions();
            let depth = sessions.get(&parent).map_or(0, |record| record.depth) + 1;
            if let Some(record) = sessions.get_mut(&key) {
                record.parent_client = Some(parent.clone());
                record.parent_tool_call_id = Some(tool_call_id.clone());
                record.child_terminal = None;
                record.agent_profile = Some(agent_profile.clone());
                record.depth = depth;
            }
        }
        self.persist(&key).await?;
        self.lock_budget_states().insert(key.clone(), budget);
        self.children.register(
            parent.clone(),
            super::delegator::Child::new(key.clone(), tool_call_id.clone()),
        );
        self.children.set_cancel(&parent, &key, Arc::clone(&cancel));
        self.emit_delegation(ServerEvent::new(
            0,
            Some(parent),
            ServerEventKind::ChildSessionStarted {
                child_session_id: key.as_str().to_owned(),
                agent_id: agent_profile,
                tool_call_id,
                lifetime,
                task_summary: task_summary.chars().take(512).collect(),
            },
        ));
        Ok(key)
    }

    /// The delegation depth of `key`: 0 for a session the client made.
    pub(super) fn depth_of(&self, key: &SessionKey) -> u32 {
        self.lock_sessions()
            .get(key)
            .map_or(0, |record| record.depth)
    }

    /// Whether `key`'s delegator is suspended (RFC 8.2.11).
    pub fn delegator_suspended(&self, key: &SessionKey) -> bool {
        self.lock_sessions()
            .get(key)
            .is_some_and(|record| record.delegator_suspended)
    }

    /// Suspends or releases `key`'s delegator (RFC 8.2.11), returning the
    /// new state and how many messages were held. Releasing flushes the
    /// queue into the session's steering: a held message is a message that
    /// still has to reach the child. The updated state is persisted.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::UnknownSession`] when `key` is not known or a
    /// persistence error when the updated state cannot be saved.
    pub async fn set_delegator_suspended(
        &self,
        key: &SessionKey,
        suspended: bool,
    ) -> Result<(bool, usize), ServiceError> {
        let service = self;
        let (parent, nid, held) = {
            let mut sessions = service.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.delegator_suspended = suspended;
            let held = if suspended {
                Vec::new()
            } else {
                std::mem::take(&mut record.delegator_queue)
            };
            (record.parent_client.clone(), record.id, held)
        };
        let queued = held.len();
        if let Some(parent) = parent {
            let _ = service.children.set_suspended(&parent, key, suspended);
        }
        for text in held {
            service
                .steering
                .push(nid, Steered::new(uuid::Uuid::new_v4().to_string(), text));
        }
        service.persist(key).await?;
        Ok((suspended, queued))
    }

    /// Holds a message for `key` while its delegator is suspended (RFC
    /// 8.2.11). Returns how many messages are held now.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::UnknownSession`] when `key` is not known,
    /// [`ServiceError::Delegation`] when the held-message limit is reached, or
    /// a persistence error when the queue cannot be saved.
    pub async fn hold_delegator_message(
        &self,
        key: &SessionKey,
        text: String,
    ) -> Result<usize, ServiceError> {
        let (parent, held) = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            if record.delegator_queue.len() >= MAX_HELD_DELEGATOR_MESSAGES {
                return Err(ServiceError::Delegation(format!(
                    "DELEGATOR_SUSPENDED: at most {MAX_HELD_DELEGATOR_MESSAGES} messages may be queued"
                )));
            }
            record.delegator_queue.push(text.clone());
            (record.parent_client.clone(), record.delegator_queue.len())
        };
        if let Some(parent) = parent {
            let _ = self.children.hold(&parent, key, text);
        }
        self.persist(key).await?;
        Ok(held)
    }

    /// The sessions the client can see.
    /// The sessions the store can still open.
    ///
    /// A record can outlive its stored session: a delete that removed the
    /// row but not the in-memory record, or a database restored from an older
    /// copy. Such a record lists fine and then fails on the first attach with
    /// a store error naming an id nobody sent, which is a ghost the client
    /// cannot clear. They are pruned here, once, and never listed again.
    pub async fn list_live_sessions(&self) -> Vec<SessionInfo> {
        let records = {
            let sessions = self.lock_sessions();
            sessions
                .iter()
                .map(|(key, record)| (key.clone(), record.id))
                .collect::<Vec<_>>()
        };
        let mut dead = Vec::new();
        for (key, id) in &records {
            // Only a store that says the session does not exist proves the
            // record is an orphan. Any other failure: a busy database, a
            // decode problem: is a reason to keep the session, not to delete
            // it.
            if let Err(error) = self.store.load(*id).await
                && error.to_string().contains("not found")
            {
                dead.push(key.clone());
            }
        }
        if !dead.is_empty() {
            {
                let mut sessions = self.lock_sessions();
                for key in &dead {
                    sessions.remove(key);
                }
            }
            for key in &dead {
                tracing::warn!(session = %key, "dropped a session with no stored state");
                if let Err(error) = self.directory.delete_session(key.as_str()).await {
                    tracing::warn!(session = %key, %error, "could not drop the orphaned session row");
                }
            }
        }
        self.list_sessions()
    }

    pub(super) fn list_sessions(&self) -> Vec<SessionInfo> {
        self.lock_sessions()
            .iter()
            .map(|(key, record)| SessionInfo {
                session: key.clone(),
                title: record.title.clone(),
                provider: record.spec.as_ref().map(|s| s.provider.clone()),
                model: record.spec.as_ref().map(|s| s.model.clone()),
                created_at: Some(record.created_at.to_rfc3339()),
                phase: record.phase,
                reasoning: record.reasoning.clone(),
                parent: record.parent_client.clone(),
            })
            .collect()
    }

    /// Gives the session a new title.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::UnknownSession`] when the session key is not known.
    pub async fn rename(&self, key: &SessionKey, title: &str) -> Result<(), ServiceError> {
        {
            let mut lock = self.lock_sessions();
            let record = lock
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.title = Some(title.to_owned());
        }
        self.persist(key).await
    }

    /// Names the session after the first message it holds, truncated.
    ///
    /// The automatic title is the first user message cut to 32 characters, set
    /// once; this is the same rule, asked again for a session that has no title
    /// yet. Nothing is sent to a model.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Name`] when the session is unknown, its history
    /// cannot be read, or it holds no user message to name it after.
    pub async fn retitle_session(&self, key: &SessionKey) -> Result<String, ServiceError> {
        let (nid, spec, _) = self.require_record(key)?;
        let spec = spec.ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        let history = self.engine(&spec, Some(key))?.history(nid).await?;
        let first = history
            .iter()
            .find(|message| message.role == norte::Role::User)
            .map(norte::Message::text)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| {
                ServiceError::Name("the session has no message to be named after".to_owned())
            })?;
        let title: String = first.trim().chars().take(32).collect::<String>();
        let title = title.trim_end().to_owned();
        if title.is_empty() {
            return Err(ServiceError::Name(
                "the first message names nothing".to_owned(),
            ));
        }
        self.rename(key, &title).await?;
        Ok(title)
    }

    /// Forgets the session and its row.
    pub(super) async fn delete(&self, key: &SessionKey) -> Result<(), ServiceError> {
        let parent = self
            .lock_sessions()
            .get(key)
            .and_then(|record| record.parent_client.clone());
        if !self.lock_sessions().contains_key(key) {
            return Err(ServiceError::UnknownSession(key.to_string()));
        }
        self.plans
            .remove_session_folder(key)
            .await
            .map_err(ServiceError::Plan)?;
        self.lock_sessions()
            .remove(key)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        if let Some(parent) = parent {
            self.children.remove(&parent, key);
        }
        self.directory.delete_session(key.as_str()).await?;
        Ok(())
    }

    /// The record of a session, with the model and the phase it is in.
    pub(super) fn require_record(
        &self,
        key: &SessionKey,
    ) -> Result<(norte::SessionId, Option<ProviderModelSpec>, ProtocolPhase), ServiceError> {
        self.lock_sessions()
            .get(key)
            .map(|record| (record.id, record.spec.clone(), record.phase))
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))
    }

    /// The session a client named, which may be a prefix of its id.
    ///
    /// The footer names a session by the first eight characters of its id, and
    /// that is not enough to ask for it back: `resume 801605a1` answered
    /// "session `801605a1` is not known" while the footer on screen showed
    /// exactly those eight. A name that matches one session resolves to it; one
    /// that matches several is an error naming them, because silently taking
    /// the first would continue the wrong conversation; and one that matches
    /// none stays unknown, so the message people already know is unchanged.
    pub(super) fn resolve_key(&self, key: &SessionKey) -> Result<SessionKey, ServiceError> {
        let sessions = self.lock_sessions();
        if sessions.contains_key(key) {
            return Ok(key.clone());
        }
        let prefix = key.as_str();
        let matches: Vec<&SessionKey> = sessions
            .keys()
            .filter(|candidate| candidate.as_str().starts_with(prefix))
            .collect();
        match matches.as_slice() {
            [only] => Ok((*only).clone()),
            [] => Err(ServiceError::UnknownSession(prefix.to_owned())),
            many => Err(ServiceError::AmbiguousSession {
                prefix: prefix.to_owned(),
                matches: many.iter().map(ToString::to_string).collect(),
            }),
        }
    }

    /// The sessions the service holds.
    pub(super) fn lock_sessions(&self) -> MutexGuard<'_, BTreeMap<SessionKey, SessionRecord>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The children of `parent`: sessions whose `parent_client` is `parent`,
    /// oldest first. Used by `list_agents` and the delegator tools, which
    /// address a child by the id `spawn_agent` returned.
    pub fn children_of(&self, parent: &SessionKey) -> Vec<super::ChildSessionSummary> {
        let sessions = self.lock_sessions();
        let mut children: Vec<_> = sessions
            .iter()
            .filter(|(_, record)| record.parent_client.as_ref() == Some(parent))
            .map(|(key, record)| super::ChildSessionSummary {
                session: key.clone(),
                title: record.title.clone(),
                state: match record.child_terminal {
                    Some(super::sessions::ChildTerminalState::Failed) => {
                        super::ChildSessionState::Failed
                    }
                    Some(super::sessions::ChildTerminalState::Stopped) => {
                        super::ChildSessionState::Stopped
                    }
                    None => match record.phase {
                        ProtocolPhase::Streaming => super::ChildSessionState::Running,
                        ProtocolPhase::AwaitingTool => super::ChildSessionState::AwaitingApproval,
                        ProtocolPhase::Idle | _ => super::ChildSessionState::Idle,
                    },
                },
            })
            .collect();
        children.sort_by_key(|child| child.session.as_str().to_owned());
        children
    }

    /// Rebuilds the in-memory child registry from the persisted
    /// `parent_client` links (point 2 of the delegation cleanup).
    ///
    /// The registry is the tools' live view (`spawn_agent` registers there);
    /// the session records are the durable one. After a restart only the
    /// records remain, so the registry is rebuilt from them: a child that
    /// survived the restart is still inspectable, messageable and stoppable
    /// by its parent. The registry is rebuilt, not merged, so a child that
    /// was deleted while down does not reappear.
    pub fn restore_child_registry(&self) {
        let links = {
            let sessions = self.lock_sessions();
            sessions
                .iter()
                .filter_map(|(key, record)| {
                    record.parent_client.as_ref().map(|parent| {
                        (
                            parent.clone(),
                            key.clone(),
                            record.parent_tool_call_id.clone().unwrap_or_default(),
                            record.delegator_suspended,
                            record.delegator_queue.clone(),
                            matches!(record.phase, ProtocolPhase::Idle),
                            record.child_terminal,
                        )
                    })
                })
                .collect::<Vec<_>>()
        };
        self.children.clear();
        for (parent, child, tool_call_id, suspended, held, finished, terminal) in links {
            let mut entry = super::delegator::Child::new(child, tool_call_id);
            entry.suspended = suspended;
            entry.held = held;
            entry.state = match terminal {
                Some(ChildTerminalState::Failed) => super::delegator::ChildState::Failed,
                Some(ChildTerminalState::Stopped) => super::delegator::ChildState::Stopped,
                None if finished => super::delegator::ChildState::Idle,
                None => super::delegator::ChildState::Running,
            };
            self.children.register(parent, entry);
        }
    }

    /// Consumes the pending overrides this turn started with, without removing
    /// entries a concurrent `SetReasoning` or `Steer` appended after the turn
    /// read them.
    pub(super) fn clear_pending(
        &self,
        key: &SessionKey,
        pending_reasoning: Option<&Reasoning>,
        pending_steers: &[Steered],
    ) {
        let mut sessions = self.lock_sessions();
        let Some(record) = sessions.get_mut(key) else {
            return;
        };
        if let Some(pending) = pending_reasoning
            && record.pending_reasoning.as_ref() == Some(pending)
        {
            record.pending_reasoning = None;
        }
        if !pending_steers.is_empty() && record.pending_steers.starts_with(pending_steers) {
            record.pending_steers.drain(..pending_steers.len());
        }
    }

    /// Drops the durable copy of the steering messages the engine applied.
    ///
    /// The messages are named, so the copy that goes is the one that was
    /// applied and not a message that arrived after it.
    pub(super) async fn clear_applied_steers(&self, key: &SessionKey, applied: &[Steered]) {
        if applied.is_empty() {
            return;
        }
        {
            let mut sessions = self.lock_sessions();
            let Some(record) = sessions.get_mut(key) else {
                return;
            };
            record.pending_steers.retain(|steered| {
                !applied
                    .iter()
                    .any(|done| done.id == steered.id && done.text == steered.text)
            });
        }
        if let Err(error) = self.persist(key).await {
            tracing::warn!(%error, "could not store the applied steering messages");
        }
    }

    /// The usage counters of a session.
    pub(super) fn stats(&self, key: &SessionKey, turn: u64) -> Stats {
        self.lock_sessions().get(key).map_or(
            Stats {
                turn,
                billed: 0,
                requests: 0,
                cost: 0.0,
                unpriced: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                context_used: 0,
                pending: std::collections::VecDeque::new(),
            },
            |record| Stats {
                turn,
                billed: record.billed,
                requests: record.requests,
                cost: record.cost,
                unpriced: record.unpriced,
                input_tokens: record.input_tokens,
                output_tokens: record.output_tokens,
                cache_read_tokens: record.cache_read_tokens,
                cache_write_tokens: record.cache_write_tokens,
                context_used: record.context_used,
                pending: std::collections::VecDeque::new(),
            },
        )
    }

    /// Writes the usage counters back.
    pub(super) async fn store_stats(&self, key: &SessionKey, stats: &Stats) {
        if let Some(record) = self.lock_sessions().get_mut(key) {
            record.billed = stats.billed;
            record.requests = stats.requests;
            record.cost = stats.cost;
            record.unpriced = stats.unpriced;
            record.input_tokens = stats.input_tokens;
            record.output_tokens = stats.output_tokens;
            // The cache counters are the session's, like the token totals, and
            // live in the same record. They were the two the record never took
            // back, so a resumed session reported a cache share over only the
            // requests it had made since the restart: the footer showed 2.5%
            // for a session whose prompts had almost all been served cached.
            record.cache_read_tokens = stats.cache_read_tokens;
            record.cache_write_tokens = stats.cache_write_tokens;
            // The context is what the last request carried, not what the
            // session spent: a long conversation is sent whole every time.
            record.context_used = stats.context_used;
        }
        if let Err(error) = self.persist(key).await {
            tracing::warn!(session = %key, %error, "could not persist session counters");
        }
    }

    /// Persists the graph head reached by a terminal turn.
    pub(super) async fn record_turn_boundary(
        &self,
        key: &SessionKey,
        turn: u64,
    ) -> Result<(), ServiceError> {
        let session_id = self
            .lock_sessions()
            .get(key)
            .map(|record| record.id)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        let session = self.store.load(session_id).await?;
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record
                .turn_boundaries
                .retain(|boundary| boundary.turn != turn);
            record.turn_boundaries.push(TurnBoundary {
                turn,
                session: session_id,
                head: session.head,
            });
            record.turn_boundaries.sort_by_key(|boundary| boundary.turn);
            record.last_turn = record.last_turn.max(turn);
        }
        self.persist(key).await
    }

    /// Records the phase the session is in.
    pub(super) async fn set_phase(&self, key: &SessionKey, phase: ProtocolPhase) {
        if let Some(record) = self.lock_sessions().get_mut(key) {
            record.phase = phase;
        }
        if let Err(error) = self.persist(key).await {
            tracing::warn!(session = %key, %error, "could not persist session phase");
        }
    }

    /// Sets the reasoning override of the session or of its next turn.
    pub(super) async fn set_reasoning(
        &self,
        _request_id: u64,
        key: SessionKey,
        scope: ReasoningScope,
        reasoning: Reasoning,
    ) -> Result<EventStream, ServiceError> {
        let (_, spec, _) = self.require_record(&key)?;
        let spec = spec.ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        // Validate at the point the setting is chosen, not on the next turn:
        // an effort the selected model does not declare is never downgraded.
        super::resolve_reasoning(
            Some(&reasoning),
            &self.config().reasoning.default,
            &spec.capabilities.reasoning,
        )?;
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            match scope {
                ReasoningScope::Session => record.reasoning = Some(reasoning),
                ReasoningScope::Message => record.pending_reasoning = Some(reasoning),
            }
        }
        self.persist(&key).await?;
        Ok(Box::pin(futures_util::stream::empty()))
    }
}
