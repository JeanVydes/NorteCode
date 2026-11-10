//! One part of the conversation page.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

#[allow(clippy::large_types_passed_by_value)]
pub(super) async fn run_conversation(context: AppContext, request: ConversationRequest) {
    let Some(api) = api(&context) else {
        return;
    };
    let mut attempt = 0u32;
    loop {
        if !still_selected(&context, &request.session_id) {
            return;
        }
        let opened = api
            .open_session_events(&request.session_id, &request.agent_id)
            .await;
        match opened {
            Ok(stream) => {
                attempt = 0;
                read_conversation(&context, &api, &request, stream).await;
            }
            Err(error) => {
                // The host does not have this session any more (it was deleted
                // here or in another tab, or the database was restored). The
                // id must be dropped, not retried: every retry fails the same
                // way and the page keeps a session that cannot exist.
                let message = error.to_string();
                if message.contains("SESSION_NOT_FOUND") || message.contains("session not found") {
                    context.forget_session(&request.session_id);
                    return;
                }
                // "Another client is attached" is not a network problem and
                // retrying forever would steal the session back from the tab
                // that is using it. The page says so and waits for the person
                // to decide.
                if message.contains("SESSION_BUSY") {
                    context.set_chat_state(ChatState::Displaced);
                    context.set_page_error(Some(
                        "This session is open in another tab. Close it there, or take over \
                         here."
                            .to_owned(),
                    ));
                    return;
                }
                context.set_page_error(Some(format!("Could not open the conversation: {error}")));
            }
        }
        if !still_selected(&context, &request.session_id) {
            return;
        }
        // A reconnect is not an error the person must act on: the page keeps
        // showing what it has and tries again, backing off so a host that is
        // down is not hammered.
        attempt = attempt.saturating_add(1);
        let delay =
            std::time::Duration::from_millis(500_u64.saturating_mul(1_u64 << attempt.min(5)));
        context.set_chat_state(ChatState::Reconnecting);
        sleep(delay).await;
    }
}

pub(super) fn still_selected(context: &AppContext, session_id: &str) -> bool {
    context
        .selected_session
        .read()
        .as_ref()
        .is_some_and(|session| session.session_id == session_id)
}

/// Reloads history into the page, keeping what the stream measured.
pub(super) async fn load_history(context: &AppContext, api: &ApiClient, session_id: &str) {
    let request = context.next_history_request();
    match api.history(session_id).await {
        Ok(items) => {
            // A slower, older reply must not overwrite a newer one: the older
            // one predates the answer that is already on screen.
            if !context.is_latest_history_request(request) {
                return;
            }
            if let Some(todos) = crate::domain::todos_from_history(&items) {
                context.set_session_todos(todos);
            }
            let measured = usage_by_turn(context);
            context.set_chat_items(reconcile(items, context, &measured));
        }
        Err(error) => context.set_page_error(Some(format!("Could not load history: {error}"))),
    }
}

/// Reads one attachment until it ends.
pub(super) async fn read_conversation(
    context: &AppContext,
    api: &ApiClient,
    request: &ConversationRequest,
    mut stream: crate::sse::SessionEventStream,
) {
    load_history(context, api, &request.session_id).await;
    context.set_chat_state(ChatState::Idle);
    loop {
        let frame = match chat_client::next_frame(&mut stream).await {
            Ok(Some(frame)) => frame,
            Ok(None) => return,
            Err(error) => {
                context.set_page_error(Some(format!("The conversation stream stopped: {error}")));
                return;
            }
        };
        if !still_selected(context, &request.session_id) {
            return;
        }
        apply(*context, frame);
    }
}

#[allow(clippy::too_many_lines, clippy::large_types_passed_by_value)]
pub(super) fn apply(context: AppContext, frame: norte_code_client::DecodedEvent) {
    let turn_of_frame = frame.turn_id.clone();
    let timestamp = frame.timestamp;
    match chat_client::decode_event(frame) {
        DecodedFrame::Accepted { turn_id, queued } => {
            // The server names the turn. A queued one waits behind the turn
            // already running: it must not steal the streaming id, or the
            // answer in flight stops drawing, and it must not be remembered
            // as pending when it is the turn that is starting now, or the
            // page never returns to Idle and "Thinking…" stays forever.
            if queued {
                context.add_pending_turn(turn_id);
                return;
            }
            context.set_streaming_turn_id(Some(turn_id.clone()));
            context.set_active_turn_id(raep_types::TurnId::new(turn_id).ok());
            context.set_chat_state(ChatState::Thinking);
        }
        DecodedFrame::Ignored => {}
        DecodedFrame::Token {
            turn_id,
            channel,
            index,
            delta,
            timestamp,
        } => {
            // A turn this page is not driving, another client's, a replay, or
            // the turn a finished job started, is adopted by the id its frame
            // carries. A frame with no id at all keeps the turn already open:
            // clearing it here hid the rest of that turn's answer.
            if let Some(turn) = turn_id.clone()
                && context.streaming_turn_id.read().as_deref() != Some(turn.as_str())
            {
                context.set_streaming_turn_id(Some(turn));
            }
            let kind = match channel {
                chat_client::TokenChannel::Text => HistoryKind::Assistant,
                chat_client::TokenChannel::Reasoning => HistoryKind::Reasoning,
            };
            let _ = index;
            // One bubble per (turn, channel): the provider's index changes
            // mid-answer when it starts a new part, and keying by it split a
            // single answer into several.
            let key = format!(
                "stream:{}:{}",
                turn_id.as_deref().unwrap_or("pending"),
                if kind == HistoryKind::Assistant {
                    "text"
                } else {
                    "reasoning"
                }
            );
            context.update_chat_items(|items| {
                if let Some(item) = items.iter_mut().find(|item| item.id == key) {
                    item.content.push_str(&delta);
                } else {
                    items.push(ConversationItem {
                        id: key,
                        turn_id,
                        role: HistoryRole::Assistant,
                        kind,
                        content: delta,
                        created_at: timestamp,
                        delivery: MessageDelivery::Sent,
                        tool: None,
                        usage: None,
                    });
                }
            });
            if kind == HistoryKind::Assistant
                && *context.chat_state.read() != ChatState::RunningTool
            {
                context.set_chat_state(ChatState::Streaming);
            }
        }
        DecodedFrame::ToolStarted {
            turn_id,
            call_id,
            tool,
            arguments,
            timestamp,
        } => {
            let path = ["path", "file_path", "target"]
                .iter()
                .find_map(|key| arguments.get(*key).and_then(|value| value.as_str()))
                .map(str::to_owned);
            let activity = ToolActivity {
                call_id: call_id.clone(),
                name: tool,
                path,
                arguments,
                output: String::new(),
                status: ToolActivityStatus::Running,
                duration_ms: None,
                result_summary: None,
            };
            context.update_chat_items(|items| {
                items.push(ConversationItem {
                    id: format!("tool:{call_id}"),
                    turn_id,
                    role: HistoryRole::Tool,
                    kind: HistoryKind::ToolCall,
                    content: String::new(),
                    created_at: timestamp,
                    delivery: MessageDelivery::Sent,
                    tool: Some(activity),
                    usage: None,
                });
            });
            context.set_chat_state(ChatState::RunningTool);
        }
        DecodedFrame::ToolOutput { call_id, delta, .. } => {
            context.update_chat_items(|items| {
                if let Some(activity) = items
                    .iter_mut()
                    .filter_map(|item| item.tool.as_mut())
                    .find(|activity| activity.call_id == call_id)
                {
                    activity.output.push_str(&delta);
                }
            });
        }
        DecodedFrame::ToolFinished {
            call_id,
            failed,
            cancelled,
            duration_ms,
            result_summary,
            ..
        } => {
            context.update_chat_items(|items| {
                if let Some(activity) = items
                    .iter_mut()
                    .filter_map(|item| item.tool.as_mut())
                    .find(|activity| activity.call_id == call_id)
                {
                    activity.status = if cancelled {
                        ToolActivityStatus::Cancelled
                    } else if failed {
                        ToolActivityStatus::Failed
                    } else {
                        ToolActivityStatus::Completed
                    };
                    activity.duration_ms = duration_ms;
                    activity.result_summary = result_summary;
                }
            });
            // What happens next is the server's to say: it either calls the
            // provider again or ends the turn, and both announce it. Guessing
            // "Thinking" here is what left it on screen after the last tool.
        }
        DecodedFrame::TurnPhase(phase) => {
            // Only the four states that describe a turn in flight arrive here;
            // the terminal ones are decoded as `TurnEnded`, not as a phase.
            if let Some(state) = match phase {
                TurnPhase::Queued => Some(ChatState::Sending),
                TurnPhase::CallingProvider | TurnPhase::AwaitingApproval => {
                    Some(ChatState::Thinking)
                }
                TurnPhase::ExecutingTool => Some(ChatState::RunningTool),
            } {
                context.set_chat_state(state);
            }
        }
        DecodedFrame::TurnEnded { turn_id, outcome } => {
            if let TurnOutcome::Failed(message) = &outcome
                && let Some(message) = message
            {
                context.set_page_error(Some(message.clone()));
            }
            context.complete_turn(turn_id.as_deref());
            // The turn is over, so any question it parked on is gone: a stop
            // or a failure leaves no one to answer.
            context.set_pending_questions(Vec::new());
            context.set_active_turn_id(None);
            // A queued message becomes this session's next turn; the page
            // keeps showing "working" for it rather than falling back to Idle
            // until the queue is empty.
            if let Some(next) = context.take_next_pending_turn() {
                context.set_streaming_turn_id(Some(next.clone()));
                context.set_active_turn_id(raep_types::TurnId::new(next).ok());
                context.set_chat_state(ChatState::Thinking);
            } else {
                context.set_streaming_turn_id(None);
                context.set_active_turn_id(None);
                context.set_chat_state(ChatState::Idle);
            }
            // The stream already drew this turn: its answer, its tool calls and
            // their output arrived as events. Re-reading the whole history
            // after every turn was a full page of JSON per turn for a
            // transcript that was already on screen. It is worth reading when
            // the turn did not finish cleanly, a failure or a stop can leave
            // the live view short of what was committed, or when nothing was
            // drawn at all.
            let clean = matches!(outcome, TurnOutcome::Completed);
            if !clean || context.chat_items.read().is_empty() {
                refresh_history(&context);
            }
        }
        DecodedFrame::History(items) => {
            context.set_chat_items(items);
        }
        DecodedFrame::Todos(todos) => context.set_session_todos(todos),
        DecodedFrame::Usage { turn_id, usage } => {
            // The metrics belong to the answer this turn produced: fold them
            // into the newest assistant item of that turn, creating one if
            // the provider used tools and left no text yet.
            let turn_id = turn_id.or_else(|| context.streaming_turn_id.read().clone());
            context.update_chat_items(|items| {
                let target = items.iter_mut().rev().find(|item| {
                    item.kind == HistoryKind::Assistant
                        && item.turn_id.is_some()
                        && (turn_id.is_none() || item.turn_id == turn_id)
                });
                if let Some(item) = target {
                    match &mut item.usage {
                        Some(existing) => existing.merge(&usage),
                        slot => *slot = Some(usage.clone()),
                    }
                } else {
                    items.push(ConversationItem {
                        id: format!("usage:{}", turn_id.as_deref().unwrap_or("turn")),
                        turn_id,
                        role: HistoryRole::Assistant,
                        kind: HistoryKind::Assistant,
                        content: String::new(),
                        created_at: timestamp,
                        delivery: MessageDelivery::Sent,
                        tool: None,
                        usage: Some(usage),
                    });
                }
            });
            // Cost and speed arrive as the request closes, which is often
            // after the turn's `COMPLETED`. Setting a working state here put
            // "Thinking…" and Stop back on screen after the turn had ended,
            // and nothing ever cleared them again.
        }
        DecodedFrame::Job(mutation) => {
            let mut jobs = context.jobs.read().clone();
            chat_client::upsert_job(&mut jobs, &mutation);
            context.set_jobs(jobs);
        }
        DecodedFrame::Compacting => {
            context.set_compacting(true);
            context.set_chat_state(ChatState::Compacting);
        }
        DecodedFrame::Compacted {
            compacted,
            summary,
            error,
        } => {
            context.set_compacting(false);
            context.set_chat_state(ChatState::Idle);
            if let Some(error) = error {
                context.set_page_error(Some(format!("Compaction failed: {error}")));
            } else if compacted {
                let note = summary.map_or_else(
                    || "Context compacted.".to_owned(),
                    |summary| format!("Context compacted. Summary: {summary}"),
                );
                context.push_system_note(note);
            }
            refresh_history(&context);
        }
        DecodedFrame::Forked {
            from,
            from_turn_id,
            new_session_id,
        } => {
            // The fork's own stream is the one that carries it to `fork_from`,
            // which switches session from the HTTP reply. Seeing it here means
            // another client forked this session: read the list back from the
            // host instead of inserting a summary built from the frame, which
            // showed a session the host might not have stored yet.
            let _ = (from, from_turn_id);
            if new_session_id.as_deref().is_some_and(|id| !id.is_empty()) {
                refresh_sessions(&context);
            }
        }
        DecodedFrame::Rewound {
            turns,
            side_effects,
        } => {
            context.update_chat_items(|items| {
                items.retain(|item| {
                    item.turn_id
                        .as_ref()
                        .is_none_or(|turn| !turns.contains(turn))
                });
            });
            if !side_effects.is_empty() {
                let names = side_effects
                    .iter()
                    .map(|(tool, _)| tool.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                context.push_system_note(format!("Rewound. Tools that ran there: {names}."));
            }
            refresh_history(&context);
        }
        DecodedFrame::ChildStarted {
            session_id,
            agent_id,
            task_summary,
        } => {
            context.upsert_child(norte_code_wire::ChildSessionSummary {
                session_id,
                title: None,
                state: norte_code_wire::ChildState::Running,
                agent: (!agent_id.is_empty()).then_some(agent_id),
                task: (!task_summary.is_empty()).then_some(task_summary),
                started_at: crate::api::now_millis().ok(),
            });
        }
        DecodedFrame::ChildUpdated {
            session_id, state, ..
        } => {
            // The wire's closed set, mapped one to one: a state this build
            // does not know fails to decode instead of reading as "stopped".

            context.upsert_child_state(&session_id, state);
        }
        DecodedFrame::OutputPart {
            id,
            modality,
            bytes,
            blob,
            error,
        } => {
            // A part the model produced: it is an attachment on the answer, so
            // it is drawn as one instead of vanishing into the event stream.
            let label = match modality.as_str() {
                "image" => "image",
                "audio" => "audio",
                "video" => "video",
                "document" | "file" => "file",
                other => other,
            };
            let text = error.map_or_else(
                || {
                    let size = crate::domain::human_count(bytes);
                    format!("[produced {label} · {size} bytes]")
                },
                |error| format!("[could not produce {label}: {error}]"),
            );
            let item = ConversationItem {
                id: format!("part:{id}"),
                turn_id: context.streaming_turn_id.read().clone(),
                role: HistoryRole::Assistant,
                kind: HistoryKind::SystemEvent,
                content: match blob {
                    Some(blob) => format!("{text} · {blob}"),
                    None => text,
                },
                created_at: crate::api::now_millis().unwrap_or(0),
                delivery: MessageDelivery::Sent,
                tool: None,
                usage: None,
            };
            context.update_chat_items(|items| items.push(item));
        }
        DecodedFrame::QuestionAsked {
            turn,
            id,
            prompt,
            options,
            multiple,
        } => {
            // The turn is parked on this answer: nothing more will arrive
            // until someone replies, so the page stops claiming to work and
            // shows the question instead.
            context.set_chat_state(ChatState::AwaitingAnswer);
            context.update_pending_questions(|questions| {
                if let Some(existing) = questions.iter_mut().find(|pending| pending.id == id) {
                    existing.prompt.clone_from(&prompt);
                    existing.options.clone_from(&options);
                    existing.multiple = multiple;
                    existing.turn_id = turn;
                } else {
                    questions.push(crate::domain::PendingQuestion {
                        id,
                        turn_id: turn,
                        prompt,
                        options,
                        multiple,
                    });
                }
            });
        }
        DecodedFrame::QuestionAnswered { id } => {
            // Answered here or from another client, either way it is no
            // longer waiting, and the card must go.
            context.update_pending_questions(|questions| {
                questions.retain(|pending| pending.id != id);
            });
            if context.pending_questions.read().is_empty() {
                context.set_chat_state(ChatState::Thinking);
            }
        }
        DecodedFrame::ModelChanged { provider, model } => {
            // The session's model changed, by this client or another: keep the
            // header honest instead of showing the model the page opened with.
            context.update_sessions(|sessions| {
                if let Some(selected) = context.selected_session.read().clone()
                    && let Some(entry) = sessions
                        .iter_mut()
                        .find(|entry| entry.session_id == selected.session_id)
                {
                    entry.model = model.as_ref().map(|model| norte_code_wire::AgentRef {
                        id: model.clone(),
                        version: String::new(),
                    });
                    entry.agent = provider.as_ref().map(|provider| norte_code_wire::AgentRef {
                        id: provider.clone(),
                        version: String::new(),
                    });
                }
            });
            if let Some(model) = model {
                context.set_selected_model(model);
            }
        }
    }
    let _ = turn_of_frame;
}

/// Reloads the session's history and folds in what the stream measured.
pub(super) fn refresh_history(context: &AppContext) {
    let Some(api) = api(context) else {
        return;
    };
    let Some(session) = context.selected_session.read().clone() else {
        return;
    };
    let context = *context;
    spawn(async move {
        load_history(&context, &api, &session.session_id).await;
    });
}

/// Reloads the session list so a fork or a rename shows up in the sidebar.
pub(super) fn refresh_sessions(context: &AppContext) {
    let Some(api) = api(context) else {
        return;
    };
    let context = *context;
    spawn(async move {
        if let Ok(sessions) = api.sessions().await {
            context.set_sessions(sessions);
        }
    });
}
