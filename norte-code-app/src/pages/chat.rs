use crate::api::ApiClient;
use crate::chat_client::{self, DecodedFrame, TurnOutcome, TurnPhase};
use crate::components::{EmptyState, JobRow, MessageItem, TodoPanel};
use crate::domain::{
    ChatState, ConversationItem, HistoryKind, HistoryRole, MessageDelivery, PendingQuestion,
    QueueMode, ToolActivity, ToolActivityStatus,
};
use crate::pages::{SessionHeader, SessionSubagents, SessionTab, SessionTelemetry};
use crate::state::AppContext;
use crate::wire::{InputPartKind, UserInputPart};
use dioxus::html::FileData;
use dioxus::prelude::*;
use norte_code_wire::AgentSummary;

/// One open conversation: the session it belongs to and the prompt the
/// composer is about to send.
struct ConversationRequest {
    session_id: String,
    agent_id: String,
}

mod actions;
mod composer;
mod conversation;
mod panels;
mod reconcile;
mod scroll;
mod transcript;

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use actions::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use composer::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use conversation::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use panels::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use reconcile::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use scroll::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use transcript::*;

#[component]

pub fn ChatPage() -> Element {
    let context = use_context::<AppContext>();
    let draft = use_signal(String::new);
    let attachments = use_signal(Vec::<FileData>::new);
    let replacing = use_signal(|| None::<String>);
    let selected = context.selected_session.read().clone();
    let epoch = *context.conversation_epoch.read();
    let chat_state = *context.chat_state.read();
    let busy = chat_state.is_busy();
    let items = context.chat_items.read().clone();
    let show_thinking = *context.show_thinking.read();
    let show_timestamp = *context.show_timestamps.read();
    let todos = context.session_todos.read().clone();
    let questions = context.pending_questions.read().clone();
    let pinned = context.pinned_turns.read().clone();
    let queue_mode = *context.queue_mode.read();
    let streaming = context.streaming_turn_id.read().clone();
    let uploads_supported = context.server_info.read().as_ref().is_some_and(|info| {
        info.capabilities.iter().any(|capability| {
            crate::domain::Capability::from_wire(capability) == crate::domain::Capability::Uploads
        })
    });
    let attachment_accept = selected_agent(&context)
        .as_ref()
        .map_or_else(String::new, attachment_accept);

    // Opening a session opens its event stream. The stream *is* the
    // attachment on this server, so it stays open while the page is open
    // and closes when the page unmounts or another session is chosen.
    // One stream per (session, epoch). Everything the effect touches is a
    // dependency, so it must not read `chat_state` or the item list: those
    // change on every delta, and re-running the effect would tear the stream
    // down and reload the history mid-answer, which is exactly what made the
    // conversation appear all at once instead of token by token.
    let session_key = selected
        .as_ref()
        .map(|session| session.session_id.clone())
        .unwrap_or_default();
    let session_agent = selected
        .as_ref()
        .and_then(|session| session.agent.as_ref())
        .map(|agent| agent.id.clone())
        .unwrap_or_default();
    let mut tab = use_signal(SessionTab::default);
    use_effect(use_reactive((&session_key,), move |(session_id,)| {
        // A new session starts on its transcript.
        if !session_id.is_empty() {
            tab.set(SessionTab::Transcript);
        }
    }));
    use_effect(use_reactive(
        (&session_key, &session_agent, &epoch),
        move |(session_id, agent_id, _epoch)| {
            if session_id.is_empty() {
                return;
            }
            let request = ConversationRequest {
                session_id: session_id.clone(),
                agent_id: agent_id.clone(),
            };
            context.set_chat_state(ChatState::LoadingHistory);
            spawn(async move { run_conversation(context, request).await });
        },
    ));

    rsx! {
        if let Some(session) = selected {
            div { class: "conversation",
                SessionHeader {
                    session_title: session.title.clone().unwrap_or_else(|| "Untitled session".to_owned()),
                    tab: *tab.read(),
                    on_tab: move |next| tab.set(next),
                    thinking: busy.then(|| chat_state.label().to_owned()).filter(|label| !label.is_empty()),
                    on_compact: {
                        let session_id = session.session_id.clone();
                        move |()| compact(&context, session_id.clone())
                    },
                }
                match *tab.read() {
                    SessionTab::Transcript => transcript(&context, &session, &items, todos.clone(), questions, pinned, show_thinking, show_timestamp, streaming.clone(), draft, replacing, attachments, uploads_supported, attachment_accept, queue_mode, chat_state),
                    SessionTab::Subagents => rsx! { SessionSubagents {} },
                    SessionTab::Jobs => rsx! { jobs_panel {} },
                    SessionTab::Plans => rsx! { plans_panel {} },
                    SessionTab::Telemetry => rsx! { SessionTelemetry {} },
                }
            }
        } else {
            EmptyState {
                title: "No session open",
                message: "Pick a session from the sidebar, or create one from Sessions."
            }
        }
    }
}
