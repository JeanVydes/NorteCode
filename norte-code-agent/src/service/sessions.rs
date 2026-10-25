//! The session record and the lifecycle around it.
//!
//! A session is the durable half of the service: what it is, what it has
//! cost, what it is allowed to do and what it still owes the model. The turn
//! machinery lives in the parent module; this one owns the state the turns
//! read and write.

use std::collections::BTreeMap;
use std::sync::{MutexGuard, PoisonError};

use chrono::{DateTime, Utc};

use norte_code_protocol::{
    Budget, Phase as ProtocolPhase, Reasoning, ReasoningScope, SessionInfo, SessionKey,
};

use super::EventStream;

use super::Steered;
use super::mapping::to_phase;
use super::{ModelSpec, Service, ServiceError, Stats, StoredSession};
use crate::provider::ModelSpec as ProviderModelSpec;

/// What the service remembers about a session.
#[derive(Debug)]
pub(super) struct SessionRecord {
    pub(super) id: norte::SessionId,
    pub(super) title: Option<String>,
    pub(super) spec: ProviderModelSpec,
    /// A model switch accepted while the session was busy. It replaces the
    /// current spec at the next idle turn boundary.
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
    /// Output tokens, summed over reported requests.
    pub(super) output_tokens: u64,
    /// Prompt tokens the provider served from its cache, summed.
    pub(super) cache_read_tokens: u64,
    /// Prompt tokens the provider wrote to its cache, summed.
    pub(super) cache_write_tokens: u64,
    /// The session budget, when one was set.
    pub(super) budget: Option<Budget>,
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
    /// Messages queued for the next turn boundary, in arrival order.
    pub(super) pending_steers: Vec<Steered>,
}

/// Builds the record of a stored session.
fn record_from(
    entry: StoredSession,
    spec: ProviderModelSpec,
    phase: ProtocolPhase,
) -> SessionRecord {
    let stats = &entry.stats;
    let number = |key: &str| stats.get(key).and_then(serde_json::Value::as_u64);
    SessionRecord {
        id: entry.north,
        title: entry.title,
        spec,
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
        output_tokens: number("output_tokens").unwrap_or(0),
        cache_read_tokens: number("cache_read_tokens").unwrap_or(0),
        cache_write_tokens: number("cache_write_tokens").unwrap_or(0),
        budget: stat(stats, "budget"),
        denied_tools: entry.denied_tools.clone(),
        allowed_tools: entry.allowed_tools.clone(),
        compacted_from: entry.compacted_from,
        compacting: false,
        compaction_baseline: number("compaction_baseline")
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0),
        reasoning: stat(stats, "reasoning"),
        pending_reasoning: stat(stats, "pending_reasoning"),
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
    }
}

/// One value of the stored statistics.
fn stat<T: serde::de::DeserializeOwned>(stats: &serde_json::Value, key: &str) -> Option<T> {
    stats
        .get(key)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
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
            let spec =
                match ModelSpec::from_catalog(&self.lock_config(), &catalog, &entry.model, None) {
                    Ok(spec) => spec,
                    Err(error) => {
                        tracing::warn!(
                            session = %entry.key,
                            model = %entry.model,
                            %error,
                            "stored session model is unavailable; using the default"
                        );
                        self.default_spec
                            .read()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                    }
                };
            let (phase, needs_repair) = self.read_phase(&entry).await;
            self.approval_rules.load_session(
                entry.north,
                &entry.denied_tools,
                &entry.allowed_tools,
            );
            let record = record_from(entry, spec, phase);
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
        Ok(())
    }

    /// The phase the session is in, and whether it needs a repair.
    ///
    /// Norte's store is the authority on phase: a parked tool or approval
    /// committed it, while the directory snapshot may predate the park.
    /// `streaming` is the exception: no process is streaming a session it just
    /// loaded, so a hard kill left it behind. Put the store back to idle and
    /// rewrite the directory row.
    async fn read_phase(&self, entry: &StoredSession) -> (ProtocolPhase, bool) {
        let stored_phase = entry.phase.clone();
        let mut needs_repair = stored_phase == "streaming";
        let phase = match self.store.load(entry.north).await {
            Ok(session) if session.phase == norte::Phase::Streaming => {
                needs_repair = true;
                let commit = norte::Commit {
                    session_id: entry.north,
                    expected_version: Some(session.version),
                    nodes: Vec::new(),
                    head: session.head,
                    phase: norte::Phase::Idle,
                    turn: session.turn,
                    grants: session.grants.clone(),
                    metadata: Some(session.metadata.clone()),
                };
                if let Err(error) = self.store.commit(commit).await {
                    tracing::warn!(
                        session = %entry.key,
                        %error,
                        "could not mark an interrupted session idle"
                    );
                }
                ProtocolPhase::Idle
            }
            Ok(session) => to_phase(session.phase),
            Err(error) => {
                tracing::warn!(
                    session = %entry.key,
                    %error,
                    "could not read the session phase; using the directory copy"
                );
                match stored_phase.as_str() {
                    "awaiting_tool" => ProtocolPhase::AwaitingTool,
                    "awaiting_approval" => ProtocolPhase::AwaitingApproval,
                    // `streaming` cannot survive a process; everything else
                    // starts idle.
                    _ => ProtocolPhase::Idle,
                }
            }
        };
        (phase, needs_repair)
    }

    /// Writes one session back to the directory.
    pub(super) async fn persist(&self, key: &SessionKey) -> Result<(), ServiceError> {
        let stored = self.lock_sessions().get(key).map(|record| StoredSession {
            key: key.to_string(),
            north: record.id,
            title: record.title.clone(),
            model: format!("{}/{}", record.spec.provider, record.spec.model),
            created_at: record.created_at,
            phase: match record.phase {
                ProtocolPhase::Idle => "idle",
                ProtocolPhase::Streaming => "streaming",
                ProtocolPhase::AwaitingTool => "awaiting_tool",
                ProtocolPhase::AwaitingApproval => "awaiting_approval",
            }
            .to_owned(),
            stats: serde_json::json!({
                "billed": record.billed,
                "requests": record.requests,
                "cost": record.cost,
                "unpriced": record.unpriced,
                "input_tokens": record.input_tokens,
                "output_tokens": record.output_tokens,
                "budget": record.budget,
                "compaction_baseline": record.compaction_baseline,
                "reasoning": record.reasoning,
                "pending_reasoning": record.pending_reasoning,
                "pending_steers": record.pending_steers,
                "pending_model": record.pending_model,
            }),
            denied_tools: record.denied_tools.clone(),
            allowed_tools: record.allowed_tools.clone(),
            compacted_from: record.compacted_from,
        });
        if let Some(stored) = stored {
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
        let engine = self.engine(&spec)?;
        let session = engine.create().await?;
        engine
            .append_history(
                session.id,
                vec![norte::Message::system(self.system_prompt.clone())],
            )
            .await?;
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let record = SessionRecord {
            id: session.id,
            title,
            spec,
            pending_model: None,
            created_at: Utc::now(),
            phase: ProtocolPhase::Idle,
            billed: 0,
            requests: 0,
            cost: 0.0,
            unpriced: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            budget: self.configured_session_budget(),
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            compacted_from: None,
            compacting: false,
            compaction_baseline: 0,
            reasoning: None,
            pending_reasoning: None,
            pending_steers: Vec::new(),
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
        Ok(key)
    }

    /// Copies a session into a new one, under a new key.
    pub(super) async fn fork_session(
        &self,
        from: &SessionKey,
        title: Option<String>,
    ) -> Result<SessionKey, ServiceError> {
        let (source, spec, _) = self.require_record(from)?;
        let engine = self.engine(&spec)?;
        let mut conversation = engine.export(source).await?;
        // Norte's import preserves the conversation's session id, which is the
        // right default for a backup/restore and exactly wrong for a fork: the
        // store would reject it as a conflict with the source. A fork is an
        // explicit new id, and the engine documents that it misses the provider
        // cache for the copied prefix.
        conversation.session = norte::SessionId::new();
        let imported = engine.import(conversation).await?;
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let record = SessionRecord {
            id: imported.id,
            title,
            spec,
            pending_model: None,
            created_at: Utc::now(),
            phase: ProtocolPhase::Idle,
            billed: 0,
            requests: 0,
            cost: 0.0,
            unpriced: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            budget: self.configured_session_budget(),
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            // A fork is a new branch, not a compaction of the source.
            compacted_from: None,
            compacting: false,
            compaction_baseline: 0,
            reasoning: None,
            pending_reasoning: None,
            pending_steers: Vec::new(),
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
        Ok(key)
    }

    /// The sessions the client can see.
    pub(super) fn list_sessions(&self) -> Vec<SessionInfo> {
        self.lock_sessions()
            .iter()
            .map(|(key, record)| SessionInfo {
                session: key.clone(),
                title: record.title.clone(),
                model: Some(record.spec.label()),
                created_at: Some(record.created_at.to_rfc3339()),
                phase: record.phase,
            })
            .collect()
    }

    /// Gives the session a new title.
    pub(super) async fn rename(&self, key: &SessionKey, title: &str) -> Result<(), ServiceError> {
        {
            let mut lock = self.lock_sessions();
            let record = lock
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.title = Some(title.to_owned());
        }
        self.persist(key).await
    }

    /// Forgets the session and its row.
    pub(super) async fn delete(&self, key: &SessionKey) -> Result<(), ServiceError> {
        self.lock_sessions()
            .remove(key)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        self.directory.delete_session(key.as_str()).await?;
        Ok(())
    }

    /// The record of a session, with the model and the phase it is in.
    pub(super) fn require_record(
        &self,
        key: &SessionKey,
    ) -> Result<(norte::SessionId, ProviderModelSpec, ProtocolPhase), ServiceError> {
        self.lock_sessions()
            .get(key)
            .map(|record| (record.id, record.spec.clone(), record.phase))
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))
    }

    /// The sessions the service holds.
    pub(super) fn lock_sessions(&self) -> MutexGuard<'_, BTreeMap<SessionKey, SessionRecord>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
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
        }
        if let Err(error) = self.persist(key).await {
            tracing::warn!(session = %key, %error, "could not persist session counters");
        }
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
