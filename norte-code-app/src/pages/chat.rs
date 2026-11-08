use crate::api::{ApiClient, delay};
use crate::components::{EmptyState, PageHeader};
use crate::domain::{ChatState, ConversationItem, HistoryKind, HistoryRole};
use crate::state::AppContext;
use dioxus::prelude::*;
use std::time::Duration;

const REPLY_POLL_ATTEMPTS: usize = 120;
const REPLY_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[component]
pub fn ChatPage() -> Element {
    let context = use_context::<AppContext>();
    let mut draft = use_signal(String::new);
    let selected = context.selected_session.read().clone();
    let chat_state = *context.chat_state.read();
    let items = context.chat_items.read().clone();
    let show_thinking = *context.show_thinking.read();
    let show_tool_output = *context.show_tool_output.read();
    let show_timestamps = *context.show_timestamps.read();
    let visible_items: Vec<_> = items
        .into_iter()
        .filter(|item| match item.kind {
            HistoryKind::Reasoning => show_thinking,
            HistoryKind::ToolCall | HistoryKind::ToolResult => show_tool_output,
            _ => true,
        })
        .collect();

    rsx! {
        PageHeader {
            eyebrow: "Session",
            title: "Conversation",
            description: "Live turns and history use the same timeline model. This host exposes history and command APIs; this app refreshes the transcript while a turn is running."
        }
        if let Some(session) = selected {
            div { class: "section-heading",
                div {
                    h2 { "{session.title.as_deref().unwrap_or(\"Untitled session\")}" }
                    p { class: "session-id", "{session.session_id}" }
                }
                span { class: "status status-muted", "{chat_state.label()}" }
            }
            if visible_items.is_empty() {
                EmptyState {
                    title: "No conversation loaded",
                    message: "Load history or send a message to this session. The renderer is shared for both sources."
                }
            } else {
                div { class: "conversation-list",
                    for item in visible_items {
                        ConversationCard { item, show_timestamps }
                    }
                }
            }
            div { class: "composer",
                textarea {
                    value: "{draft}",
                    placeholder: "Write a message...",
                    disabled: chat_state != ChatState::Idle,
                    oninput: move |event| draft.set(event.value())
                }
                div { class: "composer-footer",
                    span { "{chat_state.label()}" }
                    div { class: "button-row",
                        button {
                            class: "button-secondary",
                            disabled: chat_state != ChatState::Idle,
                            onclick: move |_| load_history(&context),
                            "Load history"
                        }
                        if chat_state == ChatState::Attaching
                            || chat_state == ChatState::Sending
                            || chat_state == ChatState::WaitingForReply {
                            button {
                                class: "button-danger",
                                onclick: move |_| cancel_turn(&context),
                                "Stop turn"
                            }
                        } else {
                            button {
                                class: "button",
                                disabled: draft.read().trim().is_empty(),
                                onclick: move |_| {
                                    let prompt = draft.read().trim().to_owned();
                                    if !prompt.is_empty() {
                                        draft.set(String::new());
                                        submit_message(&context, prompt);
                                    }
                                },
                                "Send"
                            }
                        }
                    }
                }
            }
        } else {
            EmptyState {
                title: "Choose a session first",
                message: "Select a session from the Sessions page to open its conversation."
            }
        }
    }
}

#[component]
fn ConversationCard(item: ConversationItem, show_timestamps: bool) -> Element {
    let role_class = match item.role {
        HistoryRole::User => "conversation-item conversation-item-user",
        HistoryRole::Assistant => "conversation-item conversation-item-assistant",
        HistoryRole::Tool => "conversation-item conversation-item-tool",
        HistoryRole::SystemEvent => "conversation-item conversation-item-system_event",
    };
    let timestamp = show_timestamps.then(|| format_timestamp(item.created_at));
    let untrusted = matches!(item.kind, HistoryKind::Tool | HistoryKind::ToolResult);
    rsx! {
        article { class: role_class, key: "{item.id}",
            div { class: "conversation-head",
                span { "{item.role.label()}" }
                if let Some(timestamp) = timestamp {
                    time { "{timestamp}" }
                }
            }
            if untrusted {
                p { class: "helper-text", "Tool output is untrusted data." }
            }
            p { class: "conversation-body", "{item.content}" }
        }
    }
}

fn load_history(context: &AppContext) {
    let context = *context;
    spawn(async move {
        context.set_page_error(None);
        let Some((api, session_id, agent_id)) = session_api(&context) else {
            return;
        };
        context.set_chat_state(ChatState::Attaching);
        let result = api.attach(&session_id, &agent_id).await;
        match result {
            Ok(()) => {
                let history_result = api.history(&session_id).await;
                let detach_result = api.detach(&session_id).await;
                match (history_result, detach_result) {
                    (Ok(items), Ok(())) => context.set_chat_items(items),
                    (Err(error), _) | (Ok(_), Err(error)) => {
                        context.set_page_error(Some(error.to_string()));
                    }
                }
            }
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
        context.set_chat_state(ChatState::Idle);
    });
}

fn submit_message(context: &AppContext, prompt: String) {
    let context = *context;
    spawn(async move {
        context.set_page_error(None);
        let Some((api, session_id, agent_id)) = session_api(&context) else {
            return;
        };
        context.set_chat_state(ChatState::Attaching);
        let mut attached = false;
        let result = async {
            api.attach(&session_id, &agent_id).await?;
            attached = true;
            context.set_chat_state(ChatState::Sending);
            api.send_text(&session_id, prompt.clone()).await?;
            context.set_chat_state(ChatState::WaitingForReply);
            for _ in 0..REPLY_POLL_ATTEMPTS {
                delay(REPLY_POLL_INTERVAL).await;
                let items = api.history(&session_id).await?;
                let reply_received = has_reply_after_prompt(&items, &prompt);
                context.set_chat_items(items);
                if reply_received {
                    return Ok(());
                }
            }
            Err(crate::api::ApiError::InvalidResponse(
                "The turn is still running. Reload history to see its latest output.".to_owned(),
            ))
        }
        .await;
        let detach_result = if attached {
            api.detach(&session_id).await
        } else {
            Ok(())
        };
        match (result, detach_result) {
            (Ok(()), Ok(())) => context.set_chat_state(ChatState::Idle),
            (Err(error), _) | (Ok(()), Err(error)) => {
                context.set_chat_state(ChatState::Failed);
                context.set_page_error(Some(error.to_string()));
            }
        }
    });
}

fn cancel_turn(context: &AppContext) {
    let context = *context;
    spawn(async move {
        let Some((api, session_id, _agent_id)) = session_api(&context) else {
            return;
        };
        let result = api.cancel_turn(&session_id).await;
        match result {
            Ok(()) => {
                context.set_chat_state(ChatState::Idle);
                context.set_page_error(None);
            }
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
    });
}

fn session_api(context: &AppContext) -> Option<(ApiClient, String, String)> {
    let Some(profile) = context.active_profile() else {
        context.set_page_error(Some("Select a server profile first.".to_owned()));
        return None;
    };
    let Some(session) = context.selected_session.read().clone() else {
        context.set_page_error(Some("Choose a session first.".to_owned()));
        return None;
    };
    let agent_id = session.agent.map_or_else(String::new, |agent| agent.id);
    match ApiClient::new(&profile.base_url, &profile.bearer_token) {
        Ok(api) => Some((api, session.session_id, agent_id)),
        Err(error) => {
            context.set_page_error(Some(error.to_string()));
            None
        }
    }
}

fn has_reply_after_prompt(items: &[ConversationItem], prompt: &str) -> bool {
    let Some(prompt_index) = items
        .iter()
        .rposition(|item| item.role == HistoryRole::User && item.content == prompt)
    else {
        return false;
    };
    items[prompt_index + 1..]
        .iter()
        .any(|item| item.role == HistoryRole::Assistant)
}

fn format_timestamp(timestamp: i64) -> String {
    let seconds = timestamp.div_euclid(1_000);
    format!("Unix {seconds}")
}
