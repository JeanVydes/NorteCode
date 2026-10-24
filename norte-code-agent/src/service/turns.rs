//! The turns: how a message becomes a stream of events.
//!
//! A turn starts at a boundary, runs the engine's loop and settles what it
//! cost. The steering messages the running turns have not reached are handed
//! to the engine here, at the boundary where the turn begins.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use futures_util::StreamExt;
use tokio::sync::{Notify, mpsc};

use norte::{
    Engine, EngineEvent, FinishReason, Message, Part, ReasoningRequest, Role, SessionId, Usage,
};
use norte_code_cost::{BudgetBook, BudgetNotice, CostRecord};
use norte_code_protocol::{
    Budget, ErrorCode, Reasoning as ProtocolReasoning, ServerEvent, ServerEventKind, SessionKey,
};

use norte_code_store_sqlite::SessionDirectory;

use super::compaction::{
    COMPACTION_SAFE_POINT, CompactionGate, is_compaction_abort, is_compaction_message, used_context,
};
use super::mapping::{
    call_arguments, configured_budget, emit, error_code, reported_cost, request_metrics,
    retry_plan, shorten_tool_output, to_delta, to_phase, to_usage, tool_error, tool_result_text,
};
use super::models::resolve_reasoning;
use super::sandbox::sandbox_name;
use super::tooling::output_text;
use super::{EventStream, Service, ServiceError, Stats};

impl Service {
    /// Starts a turn (or a resume) and returns the stream of its events.
    ///
    /// The outer option distinguishes a new message (which replaces the
    /// message budget) from a resume (which keeps the one already in force).
    #[allow(
        clippy::option_option,
        clippy::too_many_arguments,
        clippy::too_many_lines
    )]
    pub(super) async fn start_turn(
        self: &Arc<Self>,
        request_id: u64,
        session: Option<SessionKey>,
        input: Option<String>,
        message_budget: Option<Option<Budget>>,
        reasoning: Option<ProtocolReasoning>,
    ) -> Result<EventStream, ServiceError> {
        let (key, mut nid, mut spec) = if let Some(key) = session {
            let (nid, spec, _) = self.require_record(&key)?;
            (key, nid, spec)
        } else {
            let key = self.create_session(None).await?;
            let (nid, spec, _) = self.require_record(&key)?;
            (key, nid, spec)
        };
        let mut leading = Vec::new();
        if let Some(changed) = self.apply_pending_model(request_id, &key).await? {
            leading.push(changed);
            let (new_nid, new_spec, _) = self.require_record(&key)?;
            nid = new_nid;
            spec = new_spec;
        }
        if self.compaction_due(&key, &spec, nid).await? {
            // The compaction joins what the boundary already reported: a model
            // switch that landed first must not be dropped.
            let events = match self.perform_compaction(request_id, &key, "threshold").await {
                Ok(events) => events,
                Err(error) => vec![ServerEvent::error(
                    request_id,
                    Some(key.clone()),
                    ErrorCode::Conflict,
                    error.to_string(),
                )],
            };
            leading.extend(events);
            let (new_nid, new_spec, _) = self.require_record(&key)?;
            nid = new_nid;
            spec = new_spec;
        }
        let (session_reasoning, pending_reasoning, pending_steers) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (
                record.reasoning.clone(),
                record.pending_reasoning.clone(),
                record.pending_steers.clone(),
            )
        };
        // Precedence: an explicit message setting, then a one-shot override,
        // then the session override, then the configured default.
        let requested_reasoning = reasoning
            .or(pending_reasoning.clone())
            .or(session_reasoning);
        let reasoning = resolve_reasoning(
            requested_reasoning.as_ref(),
            &self.config.reasoning.default,
            &spec.capabilities.reasoning,
        )?;
        let configured_message = configured_budget(self.config.budgets.message.as_ref());
        let message = message_budget.map(|budget| budget.or(configured_message.clone()));
        if let Some(budget) = message.as_ref().and_then(Option::as_ref) {
            self.validate_budget(&key, budget).await?;
        }
        let existing = self.lock_budget_states().get(&key).cloned();
        let book = if let Some(message) = message {
            let book = self.new_budget_book(&key, message).await?;
            self.lock_budget_states()
                .insert(key.clone(), Arc::clone(&book));
            book
        } else if let Some(book) = existing {
            book
        } else {
            let book = self.new_budget_book(&key, configured_message).await?;
            self.lock_budget_states()
                .insert(key.clone(), Arc::clone(&book));
            book
        };
        let gate = self.compaction_gate(&spec);
        let engine = self.engine_with_budget(&spec, Some(Arc::clone(&book)), gate.clone())?;
        let state = engine.session(nid).await?;
        let turn = state.turn + 1;
        // The steering messages belong to the turn boundary that is starting
        // now. The interceptor takes them before the turn's input, so they keep
        // their arrival order; a message the engine already holds is not
        // queued twice.
        for steered in &pending_steers {
            if !self.steering.is_pending(nid, &steered.id) {
                self.steering.push(nid, steered.clone());
            }
        }
        if pending_reasoning.is_some() {
            self.clear_pending(&key, pending_reasoning.as_ref(), &[]);
            self.persist(&key).await?;
        }
        // A resumable park: after a restart the engine will not re-emit the
        // pending approvals, so surface them from the store before the turn.
        let restored: Vec<ServerEvent> = if state.phase == norte::Phase::AwaitingApproval {
            let mut restored = Vec::new();
            for record in engine.pending_approvals(nid).await? {
                let arguments = call_arguments(&engine, nid, record.call_id).await;
                let sandbox = (record.tool.as_str() == "command"
                    && self.config.tools.sandbox.enabled)
                    .then(|| sandbox_name().to_owned());
                restored.push(ServerEvent::new(
                    request_id,
                    Some(key.clone()),
                    ServerEventKind::ApprovalRequested {
                        turn,
                        approval: record.id.to_string(),
                        call_id: record.call_id.to_string(),
                        tool: record.tool.to_string(),
                        arguments,
                        sandbox,
                    },
                ));
            }
            restored
        } else {
            Vec::new()
        };
        // A resume shows where the conversation was: the client starts blank, so
        // it gets the recent messages before the turn's own output. `input` is
        // `None` exactly for a resume, so a normal message never repeats history.
        let replay = if input.is_none() {
            let ancestors = compaction_ancestors(self.directory.as_ref(), nid).await;
            replay_events(request_id, &key, &engine, nid, &ancestors).await
        } else {
            Vec::new()
        };
        // The turn starts here, in the stream's prefix, so a client always sees
        // it begin before the transcript it replays. `run_turn` used to be the
        // only sender, and its event arrives after this prefix is read, which
        // put `TurnStarted` after the replayed messages.
        leading.push(ServerEvent::new(
            request_id,
            Some(key.clone()),
            ServerEventKind::TurnStarted { turn },
        ));
        leading.extend(replay);
        let (tx, rx) = mpsc::channel::<ServerEvent>(256);
        let notify = Arc::new(Notify::new());
        self.lock_cancels().insert(key.clone(), notify.clone());

        let this = Arc::clone(self);
        let task_key = key.clone();
        tokio::spawn(async move {
            let result = this
                .run_turn(
                    engine,
                    Some(book),
                    gate,
                    request_id,
                    &task_key,
                    nid,
                    turn,
                    input,
                    reasoning,
                    spec.capabilities.max_output,
                    &tx,
                    &notify,
                )
                .await;
            if let Err(error) = result {
                let _ = tx
                    .send(ServerEvent::error(
                        request_id,
                        Some(task_key.clone()),
                        ErrorCode::Internal,
                        error.to_string(),
                    ))
                    .await;
            }
            this.lock_cancels().remove(&task_key);
        });

        Ok(Box::pin(
            futures_util::stream::iter(leading)
                .chain(futures_util::stream::iter(restored))
                .chain(futures_util::stream::unfold(rx, |mut rx| async move {
                    rx.recv().await.map(|event| (event, rx))
                })),
        ))
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_turn(
        &self,
        mut engine: Engine,
        budget: Option<Arc<Mutex<BudgetBook>>>,
        mut gate: Option<Arc<CompactionGate>>,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        turn: u64,
        input: Option<String>,
        reasoning: ReasoningRequest,
        max_output: Option<u64>,
        tx: &mpsc::Sender<ServerEvent>,
        notify: &Notify,
    ) -> Result<(), ServiceError> {
        let mut current_nid = nid;
        let mut input = input;
        let mut stats = self.stats(key, turn);
        let mut questions = self.questions.subscribe();

        let mut compacted_eagerly = false;
        let mut retries = 0u32;
        loop {
            let mut turn_builder = engine.turn(current_nid).reasoning(reasoning);
            if let Some(max_output) = max_output {
                turn_builder = turn_builder.max_output_tokens(max_output);
            }
            if let Some(text) = input.take() {
                turn_builder = turn_builder.input(Message::user(text));
            }
            for (key, value) in &self.config.extensions {
                turn_builder = turn_builder.extension(key.clone(), value.clone());
            }
            let mut stream = turn_builder.start();

            let mut cancelled = false;
            let mut interrupted = false;
            let mut failure: Option<String> = None;
            let mut failure_code = None;
            loop {
                tokio::select! {
                    biased;
                    () = notify.notified() => {
                        cancelled = true;
                        interrupted = true;
                    }
                    question = questions.recv() => {
                        if let Ok(question) = question {
                            if question.session == Some(current_nid) {
                                if !emit(
                                    tx,
                                    ServerEvent::new(
                                        request_id,
                                        Some(key.clone()),
                                        ServerEventKind::QuestionAsked {
                                            turn,
                                            question: question.id,
                                            prompt: question.question,
                                            options: question.options,
                                            multiple: question.multiple,
                                        },
                                    ),
                                )
                                .await
                                {
                                    return Ok(());
                                }
                                self.set_phase(key, norte_code_protocol::Phase::AwaitingTool).await;
                            }
                        }
                    }
                    next = stream.next() => {
                        match next {
                            Some(Ok(event)) => {
                                if is_compaction_abort(&event) {
                                    failure = Some(COMPACTION_SAFE_POINT.to_owned());
                                    break;
                                }
                                // The engine reports a fatal error as an event and
                                // again as the turn's result. Wait for the result:
                                // the retry policy decides whether the client sees
                                // an error at all.
                                if !matches!(event, EngineEvent::Error { .. })
                                    && !self.handle(
                                        &engine,
                                        request_id,
                                        key,
                                        current_nid,
                                        &event,
                                        &mut stats,
                                        tx,
                                        gate.as_deref(),
                                    )
                                    .await
                                {
                                    return Ok(());
                                }
                            }
                            Some(Err(error)) => {
                                failure_code = Some(error_code(&error));
                                failure = Some(error.to_string());
                                break;
                            }
                            None => break,
                        }
                    }
                }
                if cancelled {
                    stream.cancel();
                    cancelled = false;
                }
            }

            let outcome = stream.finish().await;
            // The last request has no successor to settle it.
            let _ = self
                .settle(
                    &engine,
                    request_id,
                    key,
                    current_nid,
                    &mut stats,
                    tx,
                    true,
                    gate.as_deref(),
                )
                .await;
            self.store_stats(key, &stats).await;

            let compaction_abort = failure.as_deref().is_some_and(is_compaction_message)
                || matches!(
                    &outcome,
                    Err(error) if is_compaction_message(&error.to_string())
                );
            if !cancelled && !interrupted && compaction_abort {
                match self.perform_compaction(request_id, key, "threshold").await {
                    Ok(events) => {
                        for event in events {
                            if !emit(tx, event).await {
                                return Ok(());
                            }
                        }
                    }
                    Err(error) => {
                        let phase = norte_code_protocol::Phase::Idle;
                        self.set_phase(key, phase).await;
                        emit(
                            tx,
                            ServerEvent::error(
                                request_id,
                                Some(key.clone()),
                                ErrorCode::Internal,
                                error.to_string(),
                            ),
                        )
                        .await;
                        emit(
                            tx,
                            ServerEvent::new(
                                request_id,
                                Some(key.clone()),
                                ServerEventKind::TurnFinished {
                                    turn,
                                    phase,
                                    committed: false,
                                },
                            ),
                        )
                        .await;
                        return Ok(());
                    }
                }
                let (new_nid, new_spec, _) = self.require_record(key)?;
                if new_nid == current_nid {
                    // The summary failed; the old session is still the active
                    // one and the turn stopped at the safe point.
                    let phase = norte_code_protocol::Phase::Idle;
                    self.set_phase(key, phase).await;
                    emit(
                        tx,
                        ServerEvent::new(
                            request_id,
                            Some(key.clone()),
                            ServerEventKind::TurnFinished {
                                turn,
                                phase,
                                committed: false,
                            },
                        ),
                    )
                    .await;
                    return Ok(());
                }
                if let Some(record) = self.lock_sessions().get(key) {
                    stats.billed = record.billed;
                    stats.requests = record.requests;
                    stats.cost = record.cost;
                    stats.unpriced = record.unpriced;
                    stats.input_tokens = record.input_tokens;
                    stats.output_tokens = record.output_tokens;
                }
                engine = self.engine_with_budget(&new_spec, budget.clone(), None)?;
                current_nid = new_nid;
                input = None;
                gate = None;
                compacted_eagerly = true;
                continue;
            }

            // A transient failure is retried by resuming the same session with
            // no new input, so the user's message is never appended twice. The
            // engine left the session streaming from that message.
            if !cancelled && !interrupted {
                if let Err(error) = &outcome {
                    if let Some((delay, reason)) = retry_plan(error, &self.config.retry, retries) {
                        if let Ok(session) = engine.session(current_nid).await {
                            self.set_phase(key, to_phase(session.phase)).await;
                        }
                        let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
                        if !emit(
                            tx,
                            ServerEvent::new(
                                request_id,
                                Some(key.clone()),
                                ServerEventKind::RetryScheduled {
                                    turn,
                                    attempt: retries + 1,
                                    delay_ms,
                                    reason,
                                },
                            ),
                        )
                        .await
                        {
                            return Ok(());
                        }
                        let waited = tokio::select! {
                            biased;
                            () = notify.notified() => false,
                            () = tokio::time::sleep(delay) => true,
                        };
                        if !waited {
                            let phase = engine
                                .session(current_nid)
                                .await
                                .map_or(norte_code_protocol::Phase::Idle, |s| to_phase(s.phase));
                            self.set_phase(key, phase).await;
                            emit(
                                tx,
                                ServerEvent::new(
                                    request_id,
                                    Some(key.clone()),
                                    ServerEventKind::TurnFinished {
                                        turn,
                                        phase,
                                        committed: false,
                                    },
                                ),
                            )
                            .await;
                            return Ok(());
                        }
                        retries += 1;
                        if !emit(
                            tx,
                            ServerEvent::new(
                                request_id,
                                Some(key.clone()),
                                ServerEventKind::RetryStarted {
                                    turn,
                                    attempt: retries,
                                },
                            ),
                        )
                        .await
                        {
                            return Ok(());
                        }
                        input = None;
                        continue;
                    }
                }
            }

            match outcome {
                Ok(outcome) => {
                    let phase = to_phase(outcome.session.phase);
                    self.set_phase(key, phase).await;
                    emit(
                        tx,
                        ServerEvent::new(
                            request_id,
                            Some(key.clone()),
                            ServerEventKind::TurnFinished {
                                turn,
                                phase,
                                committed: outcome.message.is_some(),
                            },
                        ),
                    )
                    .await;
                    if !compacted_eagerly
                        && !interrupted
                        && outcome.session.phase == norte::Phase::Idle
                        && self.budget_stopped(key).is_none()
                    {
                        if let Ok((current, spec, _)) = self.require_record(key) {
                            if self
                                .compaction_due(key, &spec, current)
                                .await
                                .unwrap_or(false)
                            {
                                match self.perform_compaction(request_id, key, "threshold").await {
                                    Ok(events) => {
                                        for event in events {
                                            if !emit(tx, event).await {
                                                break;
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        let _ = emit(
                                            tx,
                                            ServerEvent::error(
                                                request_id,
                                                Some(key.clone()),
                                                ErrorCode::Internal,
                                                error.to_string(),
                                            ),
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    let code = failure_code.unwrap_or_else(|| error_code(&error));
                    let message = failure.unwrap_or_else(|| error.to_string());
                    let message = if code == ErrorCode::Conflict {
                        format!(
                            "{message}; the session had already advanced, so this turn did not commit. \
                             Resume the session to continue from the latest commit."
                        )
                    } else {
                        message
                    };
                    let phase = engine
                        .session(current_nid)
                        .await
                        .map_or(norte_code_protocol::Phase::Idle, |s| to_phase(s.phase));
                    self.set_phase(key, phase).await;
                    emit(
                        tx,
                        ServerEvent::error(request_id, Some(key.clone()), code, message),
                    )
                    .await;
                    emit(
                        tx,
                        ServerEvent::new(
                            request_id,
                            Some(key.clone()),
                            ServerEventKind::TurnFinished {
                                turn,
                                phase,
                                committed: false,
                            },
                        ),
                    )
                    .await;
                }
            }
            break;
        }
        Ok(())
    }

    /// Maps one engine event and answers whether to keep going (the client is
    /// still reading).
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn handle(
        &self,
        engine: &Engine,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        event: &EngineEvent,
        stats: &mut Stats,
        tx: &mpsc::Sender<ServerEvent>,
        gate: Option<&CompactionGate>,
    ) -> bool {
        let kind = match event {
            EngineEvent::StreamDelta { delta, .. } => {
                to_delta(delta).map(|delta| ServerEventKind::Delta {
                    turn: stats.turn,
                    delta,
                })
            }
            EngineEvent::ToolCallStarted { call_id, tool, .. } => {
                let arguments = if let Some(arguments) = self.activity.started(call_id) {
                    Some(arguments)
                } else {
                    call_arguments(engine, nid, *call_id).await
                };
                let source = self.activity.source(call_id);
                Some(ServerEventKind::ToolStarted {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    arguments,
                    source,
                })
            }
            EngineEvent::ToolCallFinished {
                call_id,
                tool,
                is_error,
                duration_ms,
                ..
            } => {
                self.effects
                    .finished(nid, *call_id, tool.as_str(), *is_error);
                let recorded = self.activity.finished(call_id);
                let error = if *is_error {
                    if let Some(text) = recorded.as_ref().and_then(output_text) {
                        Some(text)
                    } else {
                        tool_error(engine, nid, call_id).await
                    }
                } else {
                    None
                };
                let (output, truncated) = if *is_error {
                    (None, false)
                } else {
                    let text = recorded
                        .as_ref()
                        .and_then(output_text)
                        .or(tool_result_text(engine, nid, call_id).await);
                    text.map_or((None, false), |text| {
                        let (text, truncated) = shorten_tool_output(text);
                        (Some(text), truncated)
                    })
                };
                Some(ServerEventKind::ToolFinished {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    is_error: *is_error,
                    error,
                    output,
                    truncated,
                    duration_ms: *duration_ms,
                })
            }
            EngineEvent::ApprovalRequested {
                approval,
                call_id,
                tool,
                ..
            } => {
                let arguments = call_arguments(engine, nid, *call_id).await;
                let sandbox = (tool.as_str() == "command" && self.config.tools.sandbox.enabled)
                    .then(|| sandbox_name().to_owned());
                Some(ServerEventKind::ApprovalRequested {
                    turn: stats.turn,
                    approval: approval.to_string(),
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    arguments,
                    sandbox,
                })
            }
            EngineEvent::ApprovalGranted { approval, .. } => {
                Some(ServerEventKind::ApprovalResolved {
                    approval: approval.to_string(),
                    approved: true,
                })
            }
            EngineEvent::CommitSucceeded { version, .. } => {
                // The answer is committed, so the budget already knows what it
                // cost: a crossing that happened there is reported now, not at
                // the next request.
                for notice in self.budget_notices(key, false) {
                    let kind = budget_notice_kind(notice);
                    if !emit(tx, ServerEvent::new(request_id, Some(key.clone()), kind)).await {
                        return false;
                    }
                }
                Some(ServerEventKind::Committed {
                    turn: stats.turn,
                    version: *version,
                })
            }
            EngineEvent::LayerShortCircuited { layer, .. } if layer == "budget" => self
                .budget_stopped(key)
                .map(|status| ServerEventKind::BudgetExceeded {
                    scope: status.scope,
                    unit: status.unit,
                    spent: status.spent,
                    limit: status.limit,
                    action: status.action,
                }),
            EngineEvent::RequestReconstructed { .. } => {
                return self
                    .settle(engine, request_id, key, nid, stats, tx, false, gate)
                    .await;
            }
            EngineEvent::TurnStopped { reason, .. } => Some(ServerEventKind::TurnStopped {
                reason: reason.clone(),
            }),
            EngineEvent::InterceptionApplied { messages, .. } => {
                // The engine applied the steering messages, so the durable copy
                // in the session is no longer pending.
                let applied = self.steering.applied(nid, *messages);
                let ids: Vec<String> = applied.iter().map(|steered| steered.id.clone()).collect();
                self.clear_applied_steers(key, &applied).await;
                Some(ServerEventKind::SteerApplied { ids })
            }
            _ => None,
        };
        if let Some(kind) = kind {
            return emit(tx, ServerEvent::new(request_id, Some(key.clone()), kind)).await;
        }
        true
    }

    /// Reports every model request committed since the last look.
    #[allow(clippy::too_many_arguments)]
    async fn settle(
        &self,
        engine: &Engine,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        stats: &mut Stats,
        tx: &mpsc::Sender<ServerEvent>,
        final_check: bool,
        gate: Option<&CompactionGate>,
    ) -> bool {
        let Ok(history) = engine.history(nid).await else {
            return true;
        };
        if let Some(gate) = gate {
            gate.update(used_context(&history));
        }
        for record in self.telemetry_for(key, nid) {
            if record.status == "ok" {
                // The telemetry waits for the message it belongs to: the
                // provider answered before the message was committed.
                stats.pending.push_back(record);
            } else {
                self.record_request(key, &record).await;
            }
        }
        for message in history.iter().skip(stats.billed) {
            let Some(origin) = &message.origin else {
                continue;
            };
            stats.requests += 1;
            let cost = reported_cost(&origin.usage);
            if let Some(cost) = cost {
                stats.cost += cost;
            } else {
                stats.unpriced += 1;
            }
            stats.input_tokens += origin.usage.input_tokens.unwrap_or(0);
            stats.output_tokens += origin.usage.output_tokens.unwrap_or(0);
            let record = CostRecord {
                session: key.to_string(),
                turn: stats.turn,
                index: stats.requests,
                model: self.model_label(key),
                input_tokens: origin.usage.input_tokens,
                output_tokens: origin.usage.output_tokens,
                reasoning_tokens: origin.usage.reasoning_tokens,
                cache_read_tokens: origin.usage.cache_read_tokens,
                cache_write_tokens: origin.usage.cache_write_tokens,
                cost_usd: cost,
                recorded_at: Utc::now(),
            };
            if let Err(error) = self.directory.record_cost(&record).await {
                tracing::warn!(session = %key, %error, "could not record request cost");
            }
            let telemetry = stats
                .pending
                .pop_front()
                .unwrap_or_else(|| self.request_from_usage(key, nid, stats.turn, &origin.usage));
            let metrics = request_metrics(&telemetry);
            self.record_request(key, &telemetry).await;
            let notices = self.budget_notices(key, final_check);
            let event = ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::RequestReconstructed {
                    turn: stats.turn,
                    index: stats.requests,
                    usage: to_usage(&origin.usage),
                    cost_usd: cost,
                    truncated: matches!(origin.finish_reason, FinishReason::Length),
                    duration_ms: metrics.duration_ms,
                    first_token_ms: metrics.first_token_ms,
                    tokens_per_second: metrics.tokens_per_second,
                    cache_hit: metrics.cache_hit,
                    tools: metrics.tools,
                },
            );
            if !emit(tx, event).await {
                stats.billed = history.len();
                return false;
            }
            for notice in notices {
                let kind = budget_notice_kind(notice);
                if !emit(tx, ServerEvent::new(request_id, Some(key.clone()), kind)).await {
                    stats.billed = history.len();
                    return false;
                }
            }
        }
        stats.billed = history.len();
        true
    }

    /// Takes the requests the telemetry layer finished since the last look.
    pub(super) fn telemetry_for(
        &self,
        key: &SessionKey,
        nid: SessionId,
    ) -> Vec<norte_code_store_sqlite::RequestRecord> {
        let mut records = self.telemetry.take(nid);
        records.sort_by_key(|record| record.started_at);
        for record in &mut records {
            record.session = key.to_string();
        }
        records
    }

    pub(super) async fn record_request(
        &self,
        key: &SessionKey,
        record: &norte_code_store_sqlite::RequestRecord,
    ) {
        if let Err(error) = self.directory.record_request(record).await {
            tracing::warn!(session = %key, %error, "could not record request telemetry");
        }
    }

    fn request_from_usage(
        &self,
        key: &SessionKey,
        nid: SessionId,
        turn: u64,
        usage: &Usage,
    ) -> norte_code_store_sqlite::RequestRecord {
        let model = self.model_label(key);
        let (provider, model) = model.split_once('/').map_or_else(
            || (String::new(), model.clone()),
            |(p, m)| (p.to_owned(), m.to_owned()),
        );
        norte_code_store_sqlite::RequestRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session: key.to_string(),
            north_session: nid.to_string(),
            turn,
            provider,
            model,
            started_at: Utc::now(),
            duration_ms: 0,
            first_token_ms: None,
            last_token_ms: None,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            estimated_input_tokens: None,
            cache_hit: usage.cache_read_tokens.unwrap_or(0) > 0,
            tool_calls: Vec::new(),
            reasoning: None,
            sandbox: self.config.tools.sandbox.enabled,
            sandbox_network: self.config.tools.sandbox.network,
            params: serde_json::Value::Null,
            extensions: serde_json::Value::Null,
            metadata: serde_json::json!({"source": "history"}),
            cost_usd: reported_cost(usage),
            finish_reason: None,
            status: "ok".to_owned(),
            error: None,
        }
    }

    /// The `provider/model` a session currently uses.
    pub(super) fn model_label(&self, key: &SessionKey) -> String {
        self.lock_sessions()
            .get(key)
            .map_or_else(String::new, |record| record.spec.label())
    }
}

/// The Norte sessions this one descends from through compaction, newest first.
///
/// A compaction replaces the live session and records only its immediate
/// parent, so the older links are read back from the stored sessions: each one
/// knows the parent it came from, and the directory keeps them all.
async fn compaction_ancestors(
    directory: &dyn SessionDirectory,
    current: SessionId,
) -> Vec<SessionId> {
    let Ok(stored) = directory.sessions().await else {
        return Vec::new();
    };
    // Map each Norte session to the one it was compacted from.
    let mut parents: std::collections::HashMap<SessionId, SessionId> =
        std::collections::HashMap::new();
    for entry in stored {
        if let Some(parent) = entry.compacted_from {
            parents.insert(entry.north, parent);
        }
    }
    let mut chain = Vec::new();
    let mut cursor = current;
    // A guard against a corrupt store that points a session at itself.
    let mut seen = std::collections::HashSet::new();
    seen.insert(cursor);
    while let Some(parent) = parents.get(&cursor).copied() {
        if !seen.insert(parent) {
            break;
        }
        chain.push(parent);
        cursor = parent;
    }
    chain
}

/// The most recent messages a resume replays, so a client that starts blank can
/// show where the conversation was.
const REPLAY_MESSAGES: usize = 100;

/// The recent messages of a session, as replay events for a client that is
/// starting blank.
///
/// A compaction seeds a fresh Norte session with the summary and the last few
/// messages, so the live history alone is short: a session that has compacted
/// would replay a handful of messages where the user expects the transcript.
/// The history is therefore reassembled from the chain of sessions this one was
/// compacted from, newest first, until [`REPLAY_MESSAGES`] are collected.
///
/// A compaction boundary is never crossed, which is what the user asked for: a
/// session compacted at message 50 replays from there, not from before the
/// summary that replaced the earlier turns.
async fn replay_events(
    request_id: u64,
    key: &SessionKey,
    engine: &Engine,
    nid: SessionId,
    ancestors: &[SessionId],
) -> Vec<ServerEvent> {
    let mut history = match engine.history(nid).await {
        Ok(history) => history,
        Err(_) => return Vec::new(),
    };
    // Walk back through the compaction chain, prepending each earlier session's
    // tail until there is enough to fill the window.
    for ancestor in ancestors {
        if history.len() >= REPLAY_MESSAGES {
            break;
        }
        if let Ok(earlier) = engine.history(*ancestor).await {
            let mut combined = earlier;
            combined.extend(history);
            history = combined;
        }
    }
    let start = history.len().saturating_sub(REPLAY_MESSAGES);
    history[start..]
        .iter()
        .enumerate()
        .filter_map(|(offset, message)| {
            let text = replay_text(message);
            if text.is_empty() {
                return None;
            }
            Some(ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::MessageReplayed {
                    index: u64::try_from(offset + 1).unwrap_or(u64::MAX),
                    role: role_name(message.role).to_owned(),
                    text,
                },
            ))
        })
        .collect()
}

/// The wire name of a message's role.
fn role_name(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::Developer => "developer",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        _ => "unknown",
    }
}

/// The text of a message for the replay: each text, reasoning, tool call and
/// tool result part on its own line, so a tool turn still reads.
fn replay_text(message: &Message) -> String {
    let mut lines: Vec<String> = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text(text) => lines.push(text.text.clone()),
            Part::Reasoning(reasoning) => lines.push(reasoning.text.clone()),
            Part::Refusal(refusal) => lines.push(refusal.text.clone()),
            Part::ToolCall(call) => lines.push(format!("$ {}", call.name)),
            Part::ToolResult(result) => {
                let text = result
                    .parts
                    .iter()
                    .filter_map(Part::as_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                lines.push(if text.is_empty() {
                    "(tool result)".to_owned()
                } else {
                    text
                });
            }
            _ => {}
        }
    }
    lines.join("\n")
}

/// The event a budget notice becomes.
fn budget_notice_kind(notice: BudgetNotice) -> ServerEventKind {
    match notice {
        BudgetNotice::Warning(warning) => ServerEventKind::BudgetWarning {
            scope: warning.scope,
            unit: warning.unit,
            spent: warning.spent,
            limit: warning.limit,
        },
        BudgetNotice::Exceeded(exceeded) => ServerEventKind::BudgetExceeded {
            scope: exceeded.scope,
            unit: exceeded.unit,
            spent: exceeded.spent,
            limit: exceeded.limit,
            action: exceeded.action,
        },
    }
}
