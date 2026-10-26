//! Compaction trigger, layer and history helpers.
//!
//! A compaction rewrites a long transcript as a handoff summary and seeds a
//! fresh Norte session with it, so the context keeps fitting the window.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use norte::harness::EngineEvent;
use norte::{
    Continuation, Layer, LayerContext, Message, ModelRequest, ModelStream,
    Reasoning as ReasoningCapability, ReasoningEffort, ReasoningRequest, Role, ShortCircuitReason,
};
use norte_code_protocol::{ServerEvent, ServerEventKind, SessionKey};

use super::mapping::reported_cost;
use super::{CostRecord, ModelSpec, Service, ServiceError};

impl Service {
    /// Checks whether the session has crossed the automatic compaction
    /// threshold at a turn boundary. Eager mode uses [`CompactionGate`] to make
    /// the same decision before a model request inside a turn.
    #[allow(clippy::cast_precision_loss)]
    pub(super) async fn compaction_due(
        &self,
        key: &SessionKey,
        spec: &ModelSpec,
        nid: norte::SessionId,
    ) -> Result<bool, ServiceError> {
        if !self.config().compaction.enabled {
            return Ok(false);
        }
        if self
            .lock_sessions()
            .get(key)
            .is_none_or(|record| record.compacting)
        {
            return Ok(false);
        }
        let Some(max_context) = spec.capabilities.max_context else {
            return Ok(false);
        };
        let engine = self.engine(spec)?;
        let state = engine.session(nid).await?;
        if state.phase != norte::harness::Phase::Idle {
            return Ok(false);
        }
        let history = engine.history(nid).await?;
        let baseline = self
            .lock_sessions()
            .get(key)
            .map_or(0, |record| record.compaction_baseline);
        if history.len() <= baseline
            || history.len() <= self.config().compaction.keep_recent.saturating_add(1)
        {
            return Ok(false);
        }
        let used = used_context(&history);
        Ok(used as f64 >= max_context as f64 * self.config().compaction.threshold)
    }

    /// Runs one compaction and reports its events, including a failure.
    pub(super) async fn perform_compaction(
        &self,
        request_id: u64,
        key: &SessionKey,
        reason: &str,
    ) -> Result<Vec<ServerEvent>, ServiceError> {
        if !self.begin_compaction(key) {
            return Err(ServiceError::Compaction(
                "another compaction is already running".to_owned(),
            ));
        }
        let result = self.compact_core(key).await;
        self.end_compaction(key);
        let mut events = vec![ServerEvent::new(
            request_id,
            Some(key.clone()),
            ServerEventKind::CompactionStarted {
                reason: reason.to_owned(),
            },
        )];
        match result {
            Ok(compaction) => events.push(ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::CompactionFinished {
                    compacted: true,
                    north_session: Some(compaction.fresh.to_string()),
                    summary: Some(compaction.summary),
                    error: None,
                },
            )),
            Err(error) => {
                events.push(ServerEvent::new(
                    request_id,
                    Some(key.clone()),
                    ServerEventKind::CompactionFinished {
                        compacted: false,
                        north_session: None,
                        summary: None,
                        error: Some(error.to_string()),
                    },
                ));
            }
        }
        Ok(events)
    }

    fn begin_compaction(&self, key: &SessionKey) -> bool {
        let mut sessions = self.lock_sessions();
        let Some(record) = sessions.get_mut(key) else {
            return false;
        };
        if record.compacting {
            return false;
        }
        record.compacting = true;
        true
    }

    fn end_compaction(&self, key: &SessionKey) {
        if let Some(record) = self.lock_sessions().get_mut(key) {
            record.compacting = false;
        }
    }
}

impl Service {
    /// The client-facing manual compaction, as a stream of events.
    pub(super) async fn compact_event_stream(
        &self,
        request_id: u64,
        key: SessionKey,
    ) -> Result<super::EventStream, ServiceError> {
        self.require_record(&key)?;
        let events = self.perform_compaction(request_id, &key, "manual").await?;
        Ok(Box::pin(futures_util::stream::iter(events)))
    }

    /// The algorithm: summarise with the old transcript as the prefix, then
    /// seed a fresh Norte session with the summary, the todo list and the last
    /// `keep_recent` messages. The old session is never modified.
    ///
    /// The summary comes back with the new session id: it is what the fresh
    /// session was seeded with, and it is the only record of what the history
    /// became, so a client that shows it is showing the replacement rather than
    /// guessing at it.
    pub(super) async fn compact_core(&self, key: &SessionKey) -> Result<Compaction, ServiceError> {
        let (old, spec, _) = self.require_record(key)?;
        let engine = self.engine(&spec)?;
        let state = engine.session(old).await?;
        if state.phase != norte::harness::Phase::Idle {
            return Err(ServiceError::Compaction(
                "the Norte session is not idle; resolve the turn first".to_owned(),
            ));
        }
        let history = engine.history(old).await?;
        let conversation: Vec<Message> = history
            .iter()
            .filter(|message| message.role != Role::System)
            .cloned()
            .collect();
        if conversation.is_empty() {
            return Err(ServiceError::Compaction(
                "there is no conversation to compact yet".to_owned(),
            ));
        }
        let summary_spec = self.summary_spec(&spec)?;
        let book = self.budget_book(key).await?;
        let summary_engine = self.engine_with_budget(&summary_spec, Some(book), None)?;
        let scratch = summary_engine.create().await?;
        let mut request_messages = history.clone();
        request_messages.push(Message::user(SUMMARY_PROMPT));
        summary_engine
            .append_history(scratch.id, request_messages)
            .await?;
        let reasoning = compaction_reasoning(
            &summary_spec.capabilities.reasoning,
            &self.config().compaction.reasoning,
        );
        let outcome = summary_engine
            .turn(scratch.id)
            .reasoning(reasoning)
            .tools(std::iter::empty::<norte::ToolName>())
            .start()
            .finish()
            .await?;
        let message = outcome.message.ok_or_else(|| {
            ServiceError::Compaction("the summary request committed no message".to_owned())
        })?;
        let summary = message.text();
        if summary.trim().is_empty() {
            return Err(ServiceError::Compaction(
                "the model returned an empty summary".to_owned(),
            ));
        }
        self.record_summary_usage(key, scratch.id, state.turn, &message)
            .await?;
        let start = recent_start(&conversation, self.config().compaction.keep_recent);
        let recent = conversation[start..].to_vec();
        let fresh = engine.create().await?;
        let mut seed = vec![Message::system(self.system_prompt.clone())];
        seed.push(Message::user(format!(
            "Summary of the earlier conversation, produced by Norte Code. Treat it as \
             context, not as a new request:\n\n{summary}"
        )));
        let todos = self.todo.text(Some(old));
        if !todos.starts_with("(the task list is empty)") {
            seed.push(Message::user(format!("Current task list:\n{todos}")));
        }
        seed.extend(recent);
        engine.append_history(fresh.id, seed).await?;
        let seeded = engine.history(fresh.id).await?.len();
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.id = fresh.id;
            record.compacted_from = Some(old);
            record.billed = seeded;
            record.compaction_baseline = seeded;
            record.phase = norte_code_protocol::Phase::Idle;
            self.approval_rules
                .load_session(fresh.id, &record.denied_tools, &record.allowed_tools);
        }
        self.todo.carry(old, fresh.id);
        self.persist(key).await?;
        Ok(Compaction {
            fresh: fresh.id,
            summary,
        })
    }

    /// The summariser's spec, for a test: it is the one path where a configured
    /// provider had to be resolved and was not.
    #[cfg(test)]
    pub(super) fn summary_spec_for_a_test(
        &self,
        session: &ModelSpec,
    ) -> Result<ModelSpec, ServiceError> {
        self.summary_spec(session)
    }

    fn summary_spec(&self, session: &ModelSpec) -> Result<ModelSpec, ServiceError> {
        match &self.config().compaction.model {
            // The same resolution a switch uses: a summariser can be a provider
            // the catalogue does not list, and a compaction that failed on one
            // would stop the session it was meant to make room for.
            Some(reference) => {
                Service::default_spec_for(&self.lock_config(), &self.catalog(), reference)
            }
            None => Ok(session.clone()),
        }
    }

    /// The cost the summary of a compaction reported.
    pub(super) async fn record_summary_usage(
        &self,
        key: &SessionKey,
        scratch: norte::SessionId,
        turn: u64,
        message: &Message,
    ) -> Result<(), ServiceError> {
        let Some(origin) = &message.origin else {
            return Ok(());
        };
        for record in self.telemetry_for(key, scratch) {
            self.record_request(key, &record).await;
        }
        let cost = reported_cost(&origin.usage);
        let index = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.requests += 1;
            record.cost += cost.unwrap_or(0.0);
            if cost.is_none() {
                record.unpriced += 1;
            }
            record.input_tokens += origin.usage.input_tokens.unwrap_or(0);
            record.output_tokens += origin.usage.output_tokens.unwrap_or(0);
            // The summary request is a request like any other, so its prompt
            // cache counts with the session's. It was the one counter this path
            // did not carry, and the two paths that write the record have to
            // agree or the session's cache share depends on which ran last.
            record.cache_read_tokens += origin.usage.cache_read_tokens.unwrap_or(0);
            record.cache_write_tokens += origin.usage.cache_write_tokens.unwrap_or(0);
            record.requests
        };
        let ledger = CostRecord {
            session: key.to_string(),
            turn,
            index,
            model: origin.model.to_string(),
            input_tokens: origin.usage.input_tokens,
            output_tokens: origin.usage.output_tokens,
            reasoning_tokens: origin.usage.reasoning_tokens,
            cache_read_tokens: origin.usage.cache_read_tokens,
            cache_write_tokens: origin.usage.cache_write_tokens,
            cost_usd: cost,
            recorded_at: chrono::Utc::now(),
        };
        self.directory.record_cost(&ledger).await?;
        self.budget_notices(key, true);
        self.persist(key).await?;
        Ok(())
    }
}

/// The instruction that turns the old transcript into a handoff summary.
pub(super) const SUMMARY_PROMPT: &str = "\
Rewrite this conversation as a concise handoff summary for another assistant that \
will continue the work. Keep the user's goal, the decisions and constraints, the \
files touched and why, the commands run and their failures, the open todos, and \
the user's latest request. Do not invent facts and do not call tools. Output only \
the summary.";

/// The marker the compaction layer puts on the error that stops a request.
/// What a compaction produced.
///
/// The summary is carried, not dropped: it is what the fresh session was seeded
/// with, so a client that shows it is showing what the history became.
#[derive(Clone, Debug)]
pub(super) struct Compaction {
    /// The fresh session, seeded with the summary.
    pub(super) fresh: norte::SessionId,
    /// The summary the model produced.
    pub(super) summary: String,
}

pub(super) const COMPACTION_SAFE_POINT: &str = "norte-code: compaction safe point";

/// The context a session currently occupies, in tokens.
///
/// The provider's last prompt count plus its answer is authoritative. Only when
/// no request reported usage do we fall back to a bytes/4 estimate.
pub(super) fn used_context(history: &[Message]) -> u64 {
    if let Some(used) = history
        .iter()
        .rev()
        .find_map(|message| message.origin.as_ref())
        .map(|origin| {
            origin.usage.input_tokens.unwrap_or(0) + origin.usage.output_tokens.unwrap_or(0)
        })
        .filter(|used| *used > 0)
    {
        return used;
    }
    serde_json::to_vec(history).map_or(0, |bytes| bytes.len() as u64 / 4)
}

/// Where a verbatim suffix may start without orphaning tool results.
pub(super) fn recent_start(messages: &[Message], keep: usize) -> usize {
    let mut start = messages.len().saturating_sub(keep);
    while start > 0 && messages[start].role == Role::Tool {
        start -= 1;
    }
    start
}

/// The lowest reasoning the model allows, or `Automatic` when it has none.
pub(super) fn compaction_reasoning(
    reasoning: &ReasoningCapability,
    setting: &str,
) -> ReasoningRequest {
    if !reasoning.supported {
        return ReasoningRequest::Automatic;
    }
    let setting = setting.trim().to_ascii_lowercase();
    match setting.as_str() {
        "off" | "lowest" if reasoning.can_disable => ReasoningRequest::Disabled,
        "lowest" => reasoning
            .efforts
            .iter()
            .next()
            .copied()
            .map_or(ReasoningRequest::Automatic, ReasoningRequest::Effort),
        "minimal" => ReasoningRequest::Effort(ReasoningEffort::Minimal),
        "low" => ReasoningRequest::Effort(ReasoningEffort::Low),
        "medium" => ReasoningRequest::Effort(ReasoningEffort::Medium),
        "high" => ReasoningRequest::Effort(ReasoningEffort::High),
        "xhigh" => ReasoningRequest::Effort(ReasoningEffort::XHigh),
        "max" => ReasoningRequest::Effort(ReasoningEffort::Max),
        _ => ReasoningRequest::Automatic,
    }
}

/// Whether the next request is over the compaction threshold.
///
/// The service updates it from the provider-reported usage as each request is
/// settled; the layer reads it just before the next request. It is shared by
/// every engine built for the turn and dropped after an eager compaction.
pub(super) struct CompactionGate {
    max_context: u64,
    threshold: f64,
    keep_recent: usize,
    due: AtomicBool,
}

impl CompactionGate {
    pub(super) fn new(max_context: u64, threshold: f64, keep_recent: usize) -> Self {
        Self {
            max_context,
            threshold,
            keep_recent,
            due: AtomicBool::new(false),
        }
    }

    #[allow(clippy::cast_precision_loss)]
    pub(super) fn update(&self, used: u64) {
        let due = used as f64 >= self.max_context as f64 * self.threshold;
        self.due.store(due, Ordering::Relaxed);
    }

    pub(super) fn due(&self) -> bool {
        self.due.load(Ordering::Relaxed)
    }
}

/// Aborts a model request at a safe point instead of sending it, so the
/// service can compact the idle session and restart the turn on the new one.
pub(super) struct CompactionLayer {
    pub(super) gate: Arc<CompactionGate>,
}

#[async_trait]
impl Layer for CompactionLayer {
    fn name(&self) -> &'static str {
        "compaction"
    }

    async fn wrap(
        &self,
        context: &LayerContext,
        request: ModelRequest,
        next: &dyn Continuation,
    ) -> norte::Result<ModelStream> {
        if self.gate.due() && request.messages.len() > self.gate.keep_recent.saturating_add(1) {
            let detail = format!(
                "used context reached the {} threshold of the model window; \
                 compacting before the next request",
                self.gate.threshold
            );
            // The layer names the refusal; the harness puts it in the audit
            // trail as `LayerShortCircuited` before propagating this.
            return Err(norte::Error::ShortCircuited {
                layer: self.name().to_owned(),
                reason: ShortCircuitReason::Custom {
                    name: "compaction_due".to_owned(),
                    detail: format!("{COMPACTION_SAFE_POINT}: {detail}"),
                },
            });
        }
        next.call(context, request).await
    }
}

/// Whether an engine error is the compaction layer stopping before a request.
pub(super) fn is_compaction_abort(event: &EngineEvent) -> bool {
    matches!(
        event,
        EngineEvent::Error { message, .. } if is_compaction_message(message)
    )
}

pub(super) fn is_compaction_message(message: &str) -> bool {
    message.contains(COMPACTION_SAFE_POINT)
}
