//! One part of the conversation page.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

/// Rewinds so the chosen message is the last thing said, then lets the same
/// stream continue: the turn that follows is new work, not a replay.
pub(super) fn rewind_to(context: &AppContext, turn: String) {
    let Some(client) = api(context) else {
        return;
    };
    let Some(session) = context.selected_session.read().clone() else {
        return;
    };
    let context = *context;
    spawn(async move {
        match client.rewind(&session.session_id, &turn, true).await {
            Ok(removed) => {
                context.update_chat_items(|items| {
                    items.retain(|item| {
                        item.turn_id.as_ref().is_none_or(|id| !removed.contains(id))
                    });
                });
                context.set_page_error(None);
            }
            Err(error) => context.set_page_error(Some(format!("Rewind failed: {error}"))),
        }
    });
}

/// Loads a message back into the composer and remembers the turn it came
/// from, so sending it again rewinds and replaces that turn in one step.
pub(super) fn start_replace(
    _context: &AppContext,
    item: ConversationItem,
    mut draft: Signal<String>,
    mut replacing: Signal<Option<String>>,
) {
    draft.set(item.content);
    replacing.set(item.turn_id.clone());
}

pub(super) fn fork_from(context: &AppContext, turn: Option<String>) {
    let Some(client) = api(context) else {
        return;
    };
    let Some(session) = context.selected_session.read().clone() else {
        return;
    };
    let context = *context;
    spawn(async move {
        match client
            .fork(&session.session_id, turn.as_deref(), None)
            .await
        {
            Ok(new_id) => {
                let title = session
                    .title
                    .as_deref()
                    .map_or_else(|| "Fork".to_owned(), |title| format!("{title} (fork)"));
                let fork = norte_code_wire::SessionSummary {
                    session_id: new_id,
                    title: Some(title),
                    ..Default::default()
                };
                context.update_sessions(|sessions| {
                    sessions.retain(|entry| entry.session_id != fork.session_id);
                    sessions.insert(0, fork.clone());
                });
                // The fork is a different session: opening it tears down this
                // stream and attaches the new one.
                context.select_session(fork, crate::domain::Page::Chat);
                refresh_sessions(&context);
            }
            Err(error) => context.set_page_error(Some(format!("Fork failed: {error}"))),
        }
    });
}

pub(super) fn toggle_pin(context: &AppContext, turn: String) {
    let Some(client) = api(context) else {
        return;
    };
    let Some(session) = context.selected_session.read().clone() else {
        return;
    };
    let pinned = context.pinned_turns.read().contains(&turn);
    let context = *context;
    spawn(async move {
        let turns = vec![turn];
        match client.pin_turns(&session.session_id, &turns, !pinned).await {
            Ok(pinned_turns) => context.set_pinned_turns(pinned_turns),
            Err(error) => context.set_page_error(Some(format!("Pin failed: {error}"))),
        }
    });
}

pub(super) fn compact(context: &AppContext, session_id: String) {
    let Some(client) = api(context) else {
        return;
    };
    let context = *context;
    // The finished event also clears this; starting it here means the transcript
    // says what is happening from the moment the button is pressed, even if no
    // event arrives.
    context.set_compacting(true);
    context.set_chat_state(ChatState::Compacting);
    spawn(async move {
        match client.compact(&session_id).await {
            Ok(()) => {
                load_history(&context, &client, &session_id).await;
                if context.is_compacting() {
                    context.set_compacting(false);
                    context.set_chat_state(ChatState::Idle);
                    context.push_system_note("Context compacted.".to_owned());
                }
            }
            Err(error) => {
                context.set_compacting(false);
                context.set_chat_state(ChatState::Idle);
                context.set_page_error(Some(format!("Compaction failed: {error}")));
            }
        }
    });
}

pub(super) fn stop_turn(context: &AppContext) {
    let Some(api) = api(context) else {
        return;
    };
    let Some(session) = context.selected_session.read().clone() else {
        return;
    };
    // A turn this page adopted from the stream has a streaming id and no
    // active id; the host cancels by the same id either way.
    let turn_id = context.active_turn_id.read().clone().or_else(|| {
        context
            .streaming_turn_id
            .read()
            .as_deref()
            .and_then(|id| raep_types::TurnId::new(id).ok())
    });
    let Some(turn_id) = turn_id else {
        return;
    };
    let context = *context;
    spawn(async move {
        if let Err(error) = api.cancel_turn(&session.session_id, &turn_id).await {
            context.set_page_error(Some(format!("Could not stop the turn: {error}")));
        }
        // A question the model was parked on belongs to the turn that just
        // ended: leaving it on screen offered an answer to a dead turn.
        context.set_pending_questions(Vec::new());
        context.set_streaming_turn_id(None);
        context.set_active_turn_id(None);
        context.set_chat_state(ChatState::Idle);
    });
}

pub(super) fn answer_question(context: &AppContext, question: PendingQuestion, answer: String) {
    let Some(api) = api(context) else {
        return;
    };
    let Some(session) = context.selected_session.read().clone() else {
        return;
    };
    let context = *context;
    spawn(async move {
        match api
            .answer_question(&session.session_id, &question.id, &answer)
            .await
        {
            Ok(()) => {
                context.update_pending_questions(|questions| {
                    questions.retain(|pending| pending.id != question.id);
                });
                // The answer unparks the turn; the host will now call the
                // provider again.
                if context.pending_questions.read().is_empty() {
                    context.set_chat_state(ChatState::Thinking);
                }
            }
            // A rejected answer leaves the question on screen: the model is
            // still waiting, and the card is the only way to reply.
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
    });
}

pub(super) fn api(context: &AppContext) -> Option<ApiClient> {
    let Some(profile) = context.active_profile() else {
        context.set_page_error(Some("Choose a server profile first.".to_owned()));
        return None;
    };
    match ApiClient::new(&profile.base_url, &profile.bearer_token) {
        Ok(api) => Some(api),
        Err(error) => {
            context.set_page_error(Some(error.to_string()));
            None
        }
    }
}
