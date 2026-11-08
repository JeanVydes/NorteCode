use crate::api::{ApiClient, delay};
use crate::components::{EmptyState, PageHeader};
use crate::domain::{Capability, ChatState, ConversationItem, HistoryKind, HistoryRole};
use crate::state::AppContext;
use crate::wire::{InputPartKind, UserInputPart};
use dioxus::html::FileData;
use dioxus::prelude::*;
use std::collections::HashSet;
use std::time::Duration;

const REPLY_POLL_ATTEMPTS: usize = 120;
const REPLY_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[component]
pub fn ChatPage() -> Element {
    let context = use_context::<AppContext>();
    let mut draft = use_signal(String::new);
    let mut attachments = use_signal(Vec::<FileData>::new);
    let selected = context.selected_session.read().clone();
    let chat_state = *context.chat_state.read();
    let items = context.chat_items.read().clone();
    let show_thinking = *context.show_thinking.read();
    let show_tool_output = *context.show_tool_output.read();
    let show_timestamps = *context.show_timestamps.read();
    let uploads_supported = context
        .server_info
        .read()
        .as_ref()
        .is_some_and(|info| info.capabilities.contains(&Capability::Uploads));
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
                if uploads_supported {
                    label { class: "field file-picker",
                        span { "Attach image, audio, video, or document" }
                        input {
                            r#type: "file",
                            multiple: true,
                            accept: "image/*,audio/*,video/*,application/pdf,text/plain,text/markdown",
                            disabled: chat_state != ChatState::Idle,
                            onchange: move |event| attachments.set(event.files())
                        }
                    }
                    if !attachments.read().is_empty() {
                        div { class: "attachment-list",
                            for file in attachments.read().iter() {
                                span { class: "attachment-chip", "{file.name()}" }
                            }
                        }
                    }
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
                                disabled: draft.read().trim().is_empty() && attachments.read().is_empty(),
                                onclick: move |_| {
                                    let prompt = draft.read().trim().to_owned();
                                    let files = attachments.read().clone();
                                    if !prompt.is_empty() || !files.is_empty() {
                                        draft.set(String::new());
                                        attachments.set(Vec::new());
                                        submit_message(&context, prompt, files);
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

fn submit_message(context: &AppContext, prompt: String, attachments: Vec<FileData>) {
    let context = *context;
    spawn(async move {
        context.set_page_error(None);
        context.set_active_turn_id(None);
        let Some((api, session_id, agent_id)) = session_api(&context) else {
            return;
        };
        context.set_chat_state(ChatState::Attaching);
        let mut attached = false;
        let result = async {
            api.attach(&session_id, &agent_id).await?;
            attached = true;
            let previous_items = api.history(&session_id).await?;
            let previous_ids = previous_items
                .iter()
                .map(|item| item.id.clone())
                .collect::<HashSet<_>>();
            context.set_chat_items(previous_items);
            context.set_chat_state(ChatState::Sending);
            let turn_id = if attachments.is_empty() {
                api.send_text(&session_id, &prompt).await?
            } else {
                if attachments.len() + usize::from(!prompt.is_empty()) > 16 {
                    return Err(crate::api::ApiError::InvalidResponse(
                        "A turn can contain at most 16 content parts.".to_owned(),
                    ));
                }
                let mut input_parts = Vec::with_capacity(attachments.len() + 1);
                if !prompt.is_empty() {
                    input_parts.push(UserInputPart::text(prompt.clone()));
                }
                for file in attachments {
                    let mime_type = file
                        .content_type()
                        .filter(|mime| !mime.trim().is_empty())
                        .ok_or_else(|| {
                            crate::api::ApiError::InvalidResponse(format!(
                                "The MIME type for {} is unavailable.",
                                file.name()
                            ))
                        })?;
                    let kind = input_part_kind(&mime_type).ok_or_else(|| {
                        crate::api::ApiError::InvalidResponse(format!(
                            "The server does not accept {mime_type} uploads."
                        ))
                    })?;
                    let bytes = file.read_bytes().await.map_err(|error| {
                        crate::api::ApiError::InvalidResponse(error.to_string())
                    })?;
                    let receipt = api.upload(&session_id, &mime_type, bytes.to_vec()).await?;
                    input_parts.push(UserInputPart::upload(
                        kind,
                        receipt.upload_id,
                        receipt.mime_type,
                    ));
                }
                api.send_content(&session_id, input_parts).await?
            };
            context.set_active_turn_id(Some(turn_id.clone()));
            context.set_chat_state(ChatState::WaitingForReply);
            for _ in 0..REPLY_POLL_ATTEMPTS {
                if context.active_turn_id.read().as_ref() != Some(&turn_id) {
                    return Ok(());
                }
                delay(REPLY_POLL_INTERVAL).await;
                let items = api.history(&session_id).await?;
                let reply_received = has_new_assistant(&items, &previous_ids);
                context.set_chat_items(items);
                if reply_received {
                    context.set_active_turn_id(None);
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
            (Ok(()), Ok(())) => {
                context.set_active_turn_id(None);
                context.set_chat_state(ChatState::Idle);
            }
            (Err(error), _) | (Ok(()), Err(error)) => {
                let pending = context.active_turn_id.read().is_some();
                if !pending {
                    context.set_chat_state(ChatState::Failed);
                }
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
        let Some(turn_id) = context.active_turn_id.read().clone() else {
            context.set_page_error(Some("There is no active turn to cancel.".to_owned()));
            return;
        };
        let result = api.cancel_turn(&session_id, &turn_id).await;
        match result {
            Ok(()) => {
                context.set_active_turn_id(None);
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

fn has_new_assistant(items: &[ConversationItem], previous_ids: &HashSet<String>) -> bool {
    items
        .iter()
        .any(|item| item.role == HistoryRole::Assistant && !previous_ids.contains(&item.id))
}

fn input_part_kind(mime_type: &str) -> Option<InputPartKind> {
    if mime_type.starts_with("image/") {
        Some(InputPartKind::Image)
    } else if mime_type.starts_with("audio/") {
        Some(InputPartKind::Audio)
    } else if mime_type.starts_with("video/") {
        Some(InputPartKind::Video)
    } else if matches!(
        mime_type,
        "application/pdf" | "text/plain" | "text/markdown"
    ) {
        Some(InputPartKind::Document)
    } else {
        None
    }
}

fn format_timestamp(timestamp: i64) -> String {
    let seconds = timestamp.div_euclid(1_000);
    format!("Unix {seconds}")
}
