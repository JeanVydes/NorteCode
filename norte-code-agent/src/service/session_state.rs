//! The reads and writes of one session's state.

use std::sync::PoisonError;

use norte_code_wire::{
    ContextUsage, Phase as ProtocolPhase, Reasoning, ReasoningScope, SessionKey,
};

use super::EventStream;
use super::session_record::TurnBoundary;
use crate::provider::ModelSpec;

#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    /// The `provider/model` reference a session is on, if it is known.
    ///
    /// A client needs it to change one half of the reference while keeping the
    /// other: `/provider` swaps the provider and keeps the model, and it cannot
    /// read that from anywhere of its own while a turn is running.
    #[must_use]
    pub fn session_model(&self, session: &SessionKey) -> Option<String> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.spec.as_ref().map(ModelSpec::label))
    }
    /// The model a session is on right now, with its capabilities.
    ///
    /// The whole spec and not just its label: `/debug` reports what the model in
    /// play can do, and after a `/model` those are the new model's capabilities,
    /// not the ones the process started with.
    #[must_use]
    pub fn session_spec(&self, session: &SessionKey) -> Option<ModelSpec> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.spec.clone())
    }
    /// The turn currently running for a session, if one has started.
    #[must_use]
    pub fn session_active_turn_id(&self, session: &SessionKey) -> Option<u64> {
        self.active_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
            .copied()
    }
    /// How much of its model's context a session is using.
    ///
    /// The used part is what the last request carried: the whole
    /// conversation goes again every time: and the window is what the
    /// catalogue says the model holds. A session that has made no request yet
    /// has used nothing.
    #[must_use]
    pub fn session_context(&self, session: &SessionKey) -> ContextUsage {
        self.lock_sessions()
            .get(session)
            .map_or_else(ContextUsage::default, |record| ContextUsage {
                used: record.context_used,
                window: match &record.spec {
                    Some(s) => s.capabilities.max_context,
                    None => None,
                },
            })
    }
    /// The reasoning override a session carries, if it has one.
    ///
    /// `None` is not "no reasoning": it means the session set none, so the
    /// answer is the config's default as the model resolves it: a different
    /// thing, because a model that cannot reason turns `auto` into `off`.
    #[must_use]
    pub fn session_reasoning(&self, session: &SessionKey) -> Option<Reasoning> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.reasoning.clone())
    }
    /// The title a session has, if one was set.
    ///
    /// `None` means the session is still known by its key alone. The renderer
    /// reads this so the footer can show the title instead of the key.
    #[must_use]
    pub fn session_title(&self, session: &SessionKey) -> Option<String> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.title.clone())
    }
    /// Whether the session has a background job still running.
    ///
    /// The CLI asks before deciding what `Ctrl-C` means: it ends a job when
    /// there is one and does nothing otherwise. A plain `bool` rather than a
    /// borrow, so the answer does not hold the sessions lock across a redraw.
    #[must_use]
    pub fn has_running_jobs(&self, session: &SessionKey) -> bool {
        self.lock_sessions()
            .get(session)
            .is_some_and(|record| self.jobs.any_running(record.id))
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
