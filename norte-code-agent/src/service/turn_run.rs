//! Running a turn: the stream, the tools and the compaction it may trigger.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::runtime::{EngineEvent, Harness};
use norte::domain::part::Part;
use norte::id::SessionId;
use norte::{Message, MessageStatus, ReasoningRequest};
use norte_code_protocol::costs::BudgetBook;
use norte_code_wire::{Attachment, ErrorCode, ServerEvent, ServerEventKind, SessionKey};
use std::sync::PoisonError;

use super::compaction::{
    COMPACTION_SAFE_POINT, CompactionGate, is_compaction_abort, is_compaction_message,
};
use super::mapping::{ToolResultText, emit, error_code, retry_plan, to_phase};
use super::turn_context::{AbortOnDrop, TurnContext};
use super::turn_history::{compaction_ancestors, replay_events};
use super::{Service, ServiceError, Stats};

impl Service {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) async fn run_turn(
        self: Arc<Self>,
        mut engine: Harness,
        budget: Option<Arc<Mutex<BudgetBook>>>,
        mut gate: Option<Arc<CompactionGate>>,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        turn: u64,
        input: Option<String>,
        attachments: Vec<Attachment>,
        reasoning: ReasoningRequest,
        max_output: Option<u64>,
        tx: &mpsc::Sender<ServerEvent>,
        cancellation: &CancellationToken,
    ) -> Result<(), ServiceError> {
        let mut current_nid = nid;
        let mut input = input;
        let mut stats = self.stats(key, turn);
        let current_nid_cell = Arc::new(Mutex::new(current_nid));
        let q_tx = tx.clone();
        let q_key = key.clone();
        let q_service = Arc::clone(&self);
        let q_cell = Arc::clone(&current_nid_cell);
        // Subscribe synchronously so the broker sees a receiver before any
        // tool calls `ask` (which would otherwise fail with "no listener").
        let mut qrx = self.questions.subscribe();
        let q_handle = tokio::spawn(async move {
            #[allow(clippy::while_let_loop)]
            loop {
                match qrx.recv().await {
                    Ok(question) => {
                        let nid = *q_cell.lock().unwrap_or_else(PoisonError::into_inner);
                        if question.session == Some(nid) {
                            if !emit(
                                &q_tx,
                                ServerEvent::new(
                                    request_id,
                                    Some(q_key.clone()),
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
                                break;
                            }
                            q_service
                                .set_phase(&q_key, norte_code_wire::Phase::AwaitingTool)
                                .await;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let _q_guard = AbortOnDrop(q_handle);

        let mut compacted_eagerly = false;
        let mut retries = 0u32;
        loop {
            let mut turn_builder = engine.turn(current_nid).reasoning(reasoning);
            if let Some(max_output) = max_output {
                turn_builder = turn_builder.max_output_tokens(max_output);
            }
            if let Some(text) = input.take() {
                // Text and media are one message, not two: the model reads the
                // sentence and sees the image it is about in the same turn, which
                // is what makes a line naming the image mean something.
                let mut message = Message::user(text);
                for attachment in &attachments {
                    match self.media_part(attachment).await {
                        Ok(Some(media)) => message.parts.push(Part::Media(media)),
                        Ok(None) => {}
                        Err(error) => {
                            let _ = tx
                                .send(ServerEvent::error(
                                    request_id,
                                    Some(key.clone()),
                                    ErrorCode::InvalidRequest,
                                    error,
                                ))
                                .await;
                            return Ok(());
                        }
                    }
                }
                turn_builder = turn_builder.input(message);
            }
            for (key, value) in &self.config().extensions {
                turn_builder = turn_builder.extension(key.clone(), value.clone());
            }
            let mut running_turn = turn_builder.cancellation(cancellation.clone()).start();

            let mut interrupted = false;
            let mut cancelled_without_close = false;
            let mut failure: Option<String> = None;
            let mut failure_code = None;
            let mut tool_results: Option<HashMap<String, ToolResultText>> = None;

            // The engine reports the turn's own events, and a cancel has to be
            // able to stop waiting on them. A tool that is parked: an
            // `ask_user` nobody answered: leaves the engine with no event to
            // give and no terminal event coming, so waiting on `next()` alone
            // kept the session busy for good. The cancel therefore races the
            // next event, and when the engine does not close the turn itself
            // within a grace period the turn is closed here.
            loop {
                let event = tokio::select! {
                    biased;
                    event = running_turn.next() => event,
                    () = cancellation.cancelled() => {
                        if let Ok(event) = tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            running_turn.next(),
                        )
                        .await
                        {
                            event
                        } else {
                            cancelled_without_close = true;
                            None
                        }
                    }
                };
                let Some(event) = event else { break };
                if is_compaction_abort(&event) {
                    failure = Some(COMPACTION_SAFE_POINT.to_owned());
                }
                // A call that finished after the cancel is the engine's last
                // word: report it, then end the turn, because the engine no
                // longer owns it and will send nothing more.
                if cancellation.is_cancelled()
                    && matches!(event, EngineEvent::ToolCallFinished { .. })
                {
                    let _ = self
                        .handle(
                            &mut TurnContext {
                                engine: &engine,
                                request_id,
                                key,
                                nid: current_nid,
                                stats: &mut stats,
                                tx,
                                gate: gate.as_deref(),
                                output_schema_valid: None,
                            },
                            &event,
                            &mut tool_results,
                        )
                        .await;
                    self.finish_turn(
                        &mut TurnContext {
                            engine: &engine,
                            request_id,
                            key,
                            nid: current_nid,
                            stats: &mut stats,
                            tx,
                            gate: gate.as_deref(),
                            output_schema_valid: None,
                        },
                        norte_code_wire::Phase::Idle,
                        false,
                    )
                    .await;
                    return Ok(());
                }
                if matches!(event, EngineEvent::GenerationCommitted { .. })
                    && !self
                        .settle(
                            &mut TurnContext {
                                engine: &engine,
                                request_id,
                                key,
                                nid: current_nid,
                                stats: &mut stats,
                                tx,
                                gate: gate.as_deref(),
                                output_schema_valid: None,
                            },
                            false,
                        )
                        .await
                {
                    cancellation.cancel();
                    return Ok(());
                }
                if !matches!(event, EngineEvent::Error { .. })
                    && !self
                        .handle(
                            &mut TurnContext {
                                engine: &engine,
                                request_id,
                                key,
                                nid: current_nid,
                                stats: &mut stats,
                                tx,
                                gate: gate.as_deref(),
                                output_schema_valid: None,
                            },
                            &event,
                            &mut tool_results,
                        )
                        .await
                {
                    cancellation.cancel();
                    return Ok(());
                }
                if cancelled_without_close {
                    break;
                }
            }
            // A turn the engine abandoned mid-call still has to end, or the
            // session can never be used again.
            if cancelled_without_close {
                self.finish_turn(
                    &mut TurnContext {
                        engine: &engine,
                        request_id,
                        key,
                        nid: current_nid,
                        stats: &mut stats,
                        tx,
                        gate: gate.as_deref(),
                        output_schema_valid: None,
                    },
                    norte_code_wire::Phase::Idle,
                    false,
                )
                .await;
                return Ok(());
            }
            let outcome = running_turn.finish().await;
            if let Ok(result) = &outcome {
                interrupted = result.message.as_ref().is_some_and(|message| {
                    message
                        .origin
                        .as_ref()
                        .is_some_and(|origin| origin.status == MessageStatus::Interrupted)
                });
            } else if let Err(error) = &outcome {
                failure_code = Some(error_code(error));
                failure.get_or_insert_with(|| error.to_string());
            }

            let cancelled = cancellation.is_cancelled();
            if interrupted || cancelled {
                self.finish_turn(
                    &mut TurnContext {
                        engine: &engine,
                        request_id,
                        key,
                        nid: current_nid,
                        stats: &mut stats,
                        tx,
                        gate: gate.as_deref(),
                        output_schema_valid: None,
                    },
                    norte_code_wire::Phase::Idle,
                    false,
                )
                .await;
                return Ok(());
            }

            let _ = self
                .settle(
                    &mut TurnContext {
                        engine: &engine,
                        request_id,
                        key,
                        nid: current_nid,
                        stats: &mut stats,
                        tx,
                        gate: gate.as_deref(),
                        output_schema_valid: None,
                    },
                    true,
                )
                .await;
            self.store_stats(key, &stats).await;

            let compaction_abort = failure.as_deref().is_some_and(is_compaction_message)
                || matches!(&outcome, Err(e) if is_compaction_message(&e.to_string()));

            if !cancelled && !interrupted && compaction_abort {
                match self
                    .compact_and_rebuild(
                        &mut engine,
                        budget.clone(),
                        request_id,
                        key,
                        current_nid,
                        &mut stats,
                        Some(tx),
                    )
                    .await
                {
                    Ok(Some(new_nid)) => {
                        current_nid = new_nid;
                        *current_nid_cell
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) = new_nid;
                    }
                    Ok(None) => {
                        self.finish_turn(
                            &mut TurnContext {
                                engine: &engine,
                                request_id,
                                key,
                                nid: current_nid,
                                stats: &mut stats,
                                tx,
                                gate: gate.as_deref(),
                                output_schema_valid: None,
                            },
                            norte_code_wire::Phase::Idle,
                            false,
                        )
                        .await;
                        return Ok(());
                    }
                    Err(error) => {
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
                        self.finish_turn(
                            &mut TurnContext {
                                engine: &engine,
                                request_id,
                                key,
                                nid: current_nid,
                                stats: &mut stats,
                                tx,
                                gate: gate.as_deref(),
                                output_schema_valid: None,
                            },
                            norte_code_wire::Phase::Idle,
                            false,
                        )
                        .await;
                        return Ok(());
                    }
                }
                input = None;
                gate = None;
                compacted_eagerly = true;
                continue;
            }

            // A transient failure is retried by resuming the same session with
            // no new input, so the user's message is never appended twice. The
            // engine left the session streaming from that message.
            if !cancelled
                && !interrupted
                && let Err(error) = &outcome
                && let Some((delay, reason)) = retry_plan(error, &self.config().retry, retries)
            {
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
                    () = cancellation.cancelled() => false,
                    () = tokio::time::sleep(delay) => true,
                };
                if !waited {
                    // Cancelled while waiting to retry: the failure that
                    // scheduled this retry already left the session
                    // `Streaming`, and this turn is not coming back to
                    // lower it.
                    self.settle_streaming_to_idle(current_nid).await;
                    let phase = engine
                        .session(current_nid)
                        .await
                        .map_or(norte_code_wire::Phase::Idle, |s| to_phase(s.phase));
                    self.finish_turn(
                        &mut TurnContext {
                            engine: &engine,
                            request_id,
                            key,
                            nid: current_nid,
                            stats: &mut stats,
                            tx,
                            gate: gate.as_deref(),
                            output_schema_valid: None,
                        },
                        phase,
                        false,
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

            match outcome {
                Ok(outcome) => {
                    // A turn that parked keeps its phase; one that ended is
                    // over, so nothing is streaming it any more. `Streaming`
                    // here is a leftover an interceptor left behind by stopping
                    // the turn after the input was appended. The failure arm
                    // settles the same leftover, and the invariant is one rule
                    // either way: the host never leaves a session it is not
                    // streaming in `Streaming`.
                    let settled = outcome.session.phase == crate::runtime::Phase::Streaming
                        && self.settle_streaming_to_idle(current_nid).await;
                    let phase = if settled {
                        norte_code_wire::Phase::Idle
                    } else {
                        to_phase(outcome.session.phase)
                    };
                    self.finish_turn(
                        &mut TurnContext {
                            engine: &engine,
                            request_id,
                            key,
                            nid: current_nid,
                            stats: &mut stats,
                            tx,
                            gate: gate.as_deref(),
                            output_schema_valid: None,
                        },
                        phase,
                        outcome.message.is_some(),
                    )
                    .await;
                    if !compacted_eagerly
                        && !interrupted
                        && outcome.session.phase == crate::runtime::Phase::Idle
                        && self.budget_stopped(key).is_none()
                        && let Ok((current, Some(spec), _)) = self.require_record(key)
                        && self
                            .compaction_due(key, &spec, current)
                            .await
                            .unwrap_or(false)
                    {
                        match self
                            .compact_and_rebuild(
                                &mut engine,
                                budget.clone(),
                                request_id,
                                key,
                                current,
                                &mut stats,
                                Some(tx),
                            )
                            .await
                        {
                            Ok(Some(new_nid)) => {
                                *current_nid_cell
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner) = new_nid;
                            }
                            Ok(None) => {}
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
                    // The turn is over and no retry follows: the engine left the
                    // session `Streaming` so `resume` could pick it up (FR-ERR-5),
                    // but nothing in this process will. Left there, every
                    // `/model` queues behind an idle boundary that never
                    // arrives. Settle it here, where the decision that there is
                    // no retry was just made: the retry path above `continue`s
                    // and never reaches this arm, so the resume it depends on
                    // still finds the session as it left it.
                    self.settle_streaming_to_idle(current_nid).await;
                    let phase = engine
                        .session(current_nid)
                        .await
                        .map_or(norte_code_wire::Phase::Idle, |s| to_phase(s.phase));
                    emit(
                        tx,
                        ServerEvent::error(request_id, Some(key.clone()), code, message),
                    )
                    .await;
                    if code == ErrorCode::Conflict {
                        // The failed turn may have raced with another writer.
                        // Send the committed history before closing the stream so
                        // a client can reconcile its view without resending the
                        // user's message or requiring a manual resume.
                        let ancestors =
                            compaction_ancestors(self.directory.as_ref(), current_nid).await;
                        for replay in
                            replay_events(request_id, key, &engine, current_nid, &ancestors).await
                        {
                            if !emit(tx, replay).await {
                                break;
                            }
                        }
                    }
                    self.finish_turn(
                        &mut TurnContext {
                            engine: &engine,
                            request_id,
                            key,
                            nid: current_nid,
                            stats: &mut stats,
                            tx,
                            gate: gate.as_deref(),
                            output_schema_valid: None,
                        },
                        phase,
                        false,
                    )
                    .await;
                }
            }
            break;
        }
        Ok(())
    }

    /// Runs a compaction and rebuilds the engine on the new session.
    ///
    /// The eight parameters are the turn's state; grouping them would be a
    /// bigger change than this extraction.
    ///
    /// The mid-turn abort and the post-turn threshold both compact; the
    /// differences (whether events are emitted to `tx`, whether the engine
    /// is rebuilt) used to be copied at each site. This is the one place
    /// that compacts, reloads the stats the compaction replaced, rebuilds
    /// the engine and reports the new session id.
    ///
    /// Returns `None` when the compaction failed or left the session
    /// unchanged: the caller finishes the turn in that case.
    #[allow(clippy::too_many_arguments)]
    async fn compact_and_rebuild(
        &self,
        engine: &mut Harness,
        budget: Option<Arc<Mutex<BudgetBook>>>,
        request_id: u64,
        key: &SessionKey,
        current_nid: SessionId,
        stats: &mut Stats,
        tx: Option<&mpsc::Sender<ServerEvent>>,
    ) -> Result<Option<SessionId>, ServiceError> {
        let events = self
            .perform_compaction(request_id, key, "threshold", tx)
            .await
            .map_err(|error| ServiceError::Compaction(format!("compaction failed: {error}")))?;
        if let Some(tx) = tx {
            for event in events {
                if !emit(tx, event).await {
                    return Ok(None);
                }
            }
        }
        let (new_nid, new_spec, _) = self.require_record(key)?;
        let new_spec = new_spec.ok_or(ServiceError::Unsupported("no model is configured"))?;
        if new_nid == current_nid {
            return Ok(None);
        }
        if let Some(record) = self.lock_sessions().get(key) {
            stats.billed = record.billed;
            stats.requests = record.requests;
            stats.cost = record.cost;
            stats.unpriced = record.unpriced;
            stats.input_tokens = record.input_tokens;
            stats.output_tokens = record.output_tokens;
            // The cache counters come across too. A compaction replaces
            // the session behind the record and the counters are reloaded
            // from it, so leaving these two out reset the session's cache
            // share to zero at every compaction: and a turn that
            // compacted kept reporting it as zero afterwards.
            stats.cache_read_tokens = record.cache_read_tokens;
            stats.cache_write_tokens = record.cache_write_tokens;
        }
        *engine = self.engine_with_budget(&new_spec, Some(key), budget, None, None)?;
        Ok(Some(new_nid))
    }

    /// The epilogue every turn exit shares: settle the stats, store them,
    /// set the session phase and emit `TurnFinished`. Returns false when
    /// the client stopped reading.
    ///
    /// The phase is the only thing exits disagree on (a parked turn keeps
    /// its phase, a settled one goes `Idle`), so it travels as a parameter
    /// instead of living in each call site.
    async fn finish_turn(
        &self,
        ctx: &mut TurnContext<'_>,
        phase: norte_code_wire::Phase,
        committed: bool,
    ) -> bool {
        self.clear_active_turn(ctx.key, ctx.stats.turn);
        self.set_phase(ctx.key, phase).await;
        let _ = self.settle(ctx, true).await;
        self.store_stats(ctx.key, ctx.stats).await;
        if let Err(error) = self.record_turn_boundary(ctx.key, ctx.stats.turn).await {
            tracing::warn!(session = %ctx.key, %error, "could not persist turn history boundary");
        }
        emit(
            ctx.tx,
            ServerEvent::new(
                ctx.request_id,
                Some(ctx.key.clone()),
                ServerEventKind::TurnFinished {
                    turn: ctx.stats.turn,
                    phase,
                    committed,
                    output_schema_valid: ctx.output_schema_valid,
                },
            ),
        )
        .await
    }
}
