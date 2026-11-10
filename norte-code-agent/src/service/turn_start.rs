//! Starting a turn: its boundary, its replay and its budget.

use std::sync::Arc;

use futures_util::StreamExt;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use norte_code_wire::{
    Attachment, Budget, ErrorCode, Reasoning as ProtocolReasoning, ServerEvent, ServerEventKind,
    SessionKey,
};

use super::mapping::{configured_budget, to_phase};
use super::models::resolve_reasoning;
use super::{EventStream, Service, ServiceError};

use super::turn_history::{compaction_ancestors, replay_events, restored_todos, to_todo_item};
use super::turn_media::unsupported_attachment;

impl Service {
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
        attachments: Vec<Attachment>,
        message_budget: Option<Option<Budget>>,
        reasoning: Option<ProtocolReasoning>,
        replay: bool,
    ) -> Result<EventStream, ServiceError> {
        let (key, mut nid, mut spec) = if let Some(key) = session {
            let (nid, spec, _) = self.require_record(&key)?;
            let spec = spec
                .filter(|s| !s.model.trim().is_empty())
                .ok_or(ServiceError::Unsupported("no model is configured"))?;
            (key, nid, spec)
        } else {
            let key = self.create_session(None).await?;
            let (nid, spec, _) = self.require_record(&key)?;
            let spec = spec
                .filter(|s| !s.model.trim().is_empty())
                .ok_or(ServiceError::Unsupported("no model is configured"))?;
            (key, nid, spec)
        };
        // A session has one durable turn owner. Without this gate, two
        // simultaneous messages can both read version N, then one loses its
        // optimistic commit at N+1. Steering remains independent and can still
        // be queued while the owner is running.
        let turn_guard = self.turn_lock(&key).lock_owned().await;
        // Registered before the first await: a cancel that lands while the
        // turn boundary compacts must survive the compaction, not be lost.
        let cancellation = CancellationToken::new();
        let cancel_generation = self.register_cancel(key.clone(), cancellation.clone());
        let result: Result<EventStream, ServiceError> = async {
            let mut leading = Vec::new();
            if let Some(changed) = self.apply_pending_model(request_id, &key).await? {
                leading.push(changed);
                let (new_nid, new_spec, _) = self.require_record(&key)?;
                let new_spec =
                    new_spec.ok_or(ServiceError::Unsupported("no model is configured"))?;
                nid = new_nid;
                spec = new_spec;
            }
            if self.compaction_due(&key, &spec, nid).await? {
                // The compaction joins what the boundary already reported: a model
                // switch that landed first must not be dropped.
                let events = match self
                    .perform_compaction(request_id, &key, "threshold", None)
                    .await
                {
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
                let new_spec =
                    new_spec.ok_or(ServiceError::Unsupported("no model is configured"))?;
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
            let session_config = self.profile_for_session(&key);
            let reasoning = resolve_reasoning(
                requested_reasoning.as_ref(),
                &session_config.reasoning.default,
                &spec.capabilities.reasoning,
            )?;
            let configured_message = configured_budget(session_config.budgets.message.as_ref());
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
            let engine = self.engine_with_budget(
                &spec,
                Some(&key),
                Some(Arc::clone(&book)),
                gate.clone(),
                None,
            )?;
            let state = engine.session(nid).await?;
            let last_turn = self
                .lock_sessions()
                .get(&key)
                .map_or(0, |record| record.last_turn);
            let turn = state.turn.max(last_turn) + 1;
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
            // A client that starts blank cannot tell what the session holds, so it
            // asks for the transcript. It asks explicitly because the request's own
            // shape cannot answer it: a resume has no message and a message has one,
            // but `run --session KEY` sends a message to a session with a history
            // and wants both.
            let replay = if replay {
                let ancestors = compaction_ancestors(self.directory.as_ref(), nid).await;
                let mut events = Vec::new();
                if let Some(stats) = self
                    .resume_stats(request_id, &key, &engine, nid, &spec)
                    .await
                {
                    events.push(stats);
                }
                events.extend(replay_events(request_id, &key, &engine, nid, &ancestors).await);
                // The task list is the model's, written through `todo`, and it lives
                // in the call it made: which is committed with the conversation and
                // therefore survived the restart. Reading it back puts the widget on
                // screen and, just as much, gives the next compaction a list to
                // summarise instead of nothing.
                if let Ok(history) = engine.history(nid).await {
                    let todos = restored_todos(&history);
                    if !todos.is_empty() {
                        self.todo
                            .restore(nid, todos.iter().map(to_todo_item).collect());
                        events.push(ServerEvent::new(
                            request_id,
                            Some(key.clone()),
                            ServerEventKind::SessionTodos { todos },
                        ));
                    }
                }
                events
            } else {
                Vec::new()
            };
            // A turn starts by committing the user's message and only then checks
            // it against the model, so an attachment the model cannot take used to
            // be committed first and rejected second: every later turn rebuilt the
            // history, met the same part and failed the same way, which left the
            // session unusable. A new model did not help, because the part is in the
            // history, not in the request. The check therefore belongs here, before
            // anything is committed: a part that cannot be sent is refused while
            // there is still nothing to undo, and the session carries on.
            if let Some((code, message)) = unsupported_attachment(&spec.capabilities, &attachments)
            {
                // The session's own phase, not a made-up `Idle`: nothing was
                // committed, so whatever it was before the turn it still is, and a
                // session parked on an approval has to stay parked for the client
                // that is looking at the approval it owes.
                let phase = to_phase(state.phase);
                self.set_phase(&key, phase).await;
                self.remove_cancel(&key, cancel_generation);
                let stream: EventStream = Box::pin(futures_util::stream::iter(vec![
                    ServerEvent::new(
                        request_id,
                        Some(key.clone()),
                        ServerEventKind::TurnStarted { turn },
                    ),
                    ServerEvent::error(request_id, Some(key.clone()), code, message),
                    ServerEvent::new(
                        request_id,
                        Some(key.clone()),
                        ServerEventKind::TurnFinished {
                            turn,
                            phase,
                            committed: false,
                            output_schema_valid: None,
                        },
                    ),
                ]));
                return Ok(stream);
            }
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
            let task_cancellation = cancellation.clone();

            let this = Arc::clone(self);
            let task_key = key.clone();
            self.set_active_turn(key.clone(), turn);
            tokio::spawn(async move {
                let _turn_guard = turn_guard;
                let runner = Arc::clone(&this);
                let result = runner
                    .run_turn(
                        engine,
                        Some(book),
                        gate,
                        request_id,
                        &task_key,
                        nid,
                        turn,
                        input,
                        attachments,
                        reasoning,
                        spec.capabilities.max_output,
                        &tx,
                        &task_cancellation,
                    )
                    .await;
                this.clear_active_turn(&task_key, turn);
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
                this.remove_cancel(&task_key, cancel_generation);
            });

            Ok(Box::pin(futures_util::stream::iter(leading).chain(
                futures_util::stream::unfold(rx, |mut rx| async move {
                    rx.recv().await.map(|event| (event, rx))
                }),
            )))
        }
        .await;
        if result.is_err() {
            self.remove_cancel(&key, cancel_generation);
        }
        result
    }
}
