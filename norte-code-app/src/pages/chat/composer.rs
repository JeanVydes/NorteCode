//! One part of the conversation page.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

#[component]
pub(super) fn ComposerReasoning(session_id: String) -> Element {
    let context = use_context::<AppContext>();
    // The setting lives on the session, so the control starts from what the
    // session reports instead of pretending everything is "auto".
    let initial = context
        .selected_session
        .read()
        .as_ref()
        .and_then(|session| session.reasoning.clone())
        .map_or_else(|| "automatic".to_owned(), |value| reasoning_choice(&value));
    let mut current = use_signal(|| initial);
    let mut busy = use_signal(|| false);
    rsx! {
        select {
            class: "composer-chip",
            title: "Reasoning effort for this session",
            value: "{current}",
            disabled: *busy.read(),
            onchange: move |event| {
                let value = event.value();
                let setting = match value.as_str() {
                    "disabled" => norte_code_wire::Reasoning::Disabled,
                    "low" => norte_code_wire::Reasoning::Effort { effort: "low".to_owned() },
                    "medium" => norte_code_wire::Reasoning::Effort { effort: "medium".to_owned() },
                    "high" => norte_code_wire::Reasoning::Effort { effort: "high".to_owned() },
                    _ => norte_code_wire::Reasoning::Automatic,
                };
                current.set(value);
                busy.set(true);
                let context = context;
                let session_id = session_id.clone();
                spawn(async move {
                    if let Some(client) = api(&context)
                        && let Err(error) = client
                            .set_reasoning(
                                &session_id,
                                norte_code_wire::ReasoningScope::Session,
                                setting,
                            )
                            .await
                    {
                        context.set_page_error(Some(format!(
                            "Could not set reasoning: {error}"
                        )));
                    }
                    busy.set(false);
                });
            },
            option { value: "automatic", "Reasoning: auto" }
            option { value: "disabled", "Reasoning: off" }
            option { value: "low", "Reasoning: low" }
            option { value: "medium", "Reasoning: medium" }
            option { value: "high", "Reasoning: high" }
        }
    }
}

/// The select value for a `SET_REASONING` setting as the host reports it.
pub(super) fn reasoning_choice(value: &norte_code_wire::Reasoning) -> String {
    match value {
        norte_code_wire::Reasoning::Disabled => "disabled".to_owned(),
        norte_code_wire::Reasoning::Effort { effort } => effort.clone(),
        // A token budget has no effort level to show, so it reads as auto.
        norte_code_wire::Reasoning::Budget { .. } | norte_code_wire::Reasoning::Automatic => {
            "automatic".to_owned()
        }
    }
}

#[component]
pub(super) fn ComposerModel(session_id: String) -> Element {
    let context = use_context::<AppContext>();
    let models = use_signal(Vec::new);
    // A `<select>` shows the option whose `value` matches its own `value`; when
    // none does, the browser falls back to the first option. The key therefore
    // has to be the same one the options use, provider and model, or the
    // control claims the session runs on a model it does not.
    // The session summary names the *agent* and the model id, not the
    // provider: `agent.id` is `norte-code`, so building the key from it never
    // matched an option and the control fell back to the first one in the
    // list. The provider is the catalogue's answer for that model id.
    let selected = context.selected_session.read().clone();
    let model_id = selected
        .as_ref()
        .and_then(|session| session.model.clone())
        .map(|model| model.id)
        .unwrap_or_default();
    let session_provider = selected
        .as_ref()
        .and_then(|session| session.provider.clone())
        .unwrap_or_default();
    let current = if model_id.is_empty() {
        String::new()
    } else if !session_provider.is_empty() {
        format!("{session_provider}\u{1f}{model_id}")
    } else {
        // A summary from a host that does not name the provider still keys by
        // the catalogue entry for the model.
        models
            .read()
            .iter()
            .find(|entry: &&norte_code_wire::ModelInfo| entry.model == model_id)
            .map_or_else(
                || model_id.clone(),
                |entry| format!("{}\u{1f}{}", entry.provider, entry.model),
            )
    };
    use_effect({
        let mut models = models;
        move || {
            if models.read().is_empty() {
                spawn(async move {
                    if let Some(client) = api(&context)
                        && let Ok(list) = client.models().await
                    {
                        models.set(list);
                    }
                });
            }
        }
    });
    let catalogue = models.read().clone();
    rsx! {
        select {
            class: "composer-model",
            title: "The model this session runs on",
            value: "{current}",
            onchange: move |event| {
                let value = event.value();
                let context = context;
                let session_id = session_id.clone();
                spawn(async move {
                    let Some((provider, model)) = value.split_once('\u{1f}') else {
                        return;
                    };
                    match api(&context) {
                        Some(client) => {
                            if let Err(error) = client
                                .set_session_model(&session_id, provider, model)
                                .await
                            {
                                context.set_page_error(Some(format!(
                                    "Model change failed: {error}"
                                )));
                            } else {
                                // Show the switch at once: the select is driven
                                // by the session's model, and the host confirms
                                // it on the stream a moment later.
                                context.set_selected_model(model.to_owned());
                            }
                        }
                        None => context.set_page_error(Some(
                            "Choose a server profile first.".to_owned(),
                        )),
                    }
                });
            },
            if catalogue.is_empty() {
                option { value: "{current}", "{current}" }
            }
            // The session may run on a model the catalogue does not list, a
            // plugin's, or one removed from the files since. Without an option
            // for it the control falls back to the first entry and claims a
            // model the session is not running.
            if !current.is_empty()
                && !catalogue
                    .iter()
                    .any(|entry: &norte_code_wire::ModelInfo| format!("{}\u{1f}{}", entry.provider, entry.model) == current)
            {
                option { value: "{current}", "{current.replace('\u{1f}', \" / \")} (current)" }
            }
            for model in &catalogue {
                option {
                    value: "{model.provider}\u{1f}{model.model}",
                    "{human_model(&model.provider, &model.name)}",
                }
            }
        }
    }
}

pub(super) fn human_model(provider: &str, name: &str) -> String {
    if provider.is_empty() {
        name.to_owned()
    } else {
        format!("{name} · {provider}")
    }
}

pub(super) fn send(
    context: &AppContext,
    prompt: Option<&str>,
    files: Vec<FileData>,
    replaces_turn_id: Option<String>,
) {
    let Some(api) = api(context) else {
        return;
    };
    let Some(session) = context.selected_session.read().clone() else {
        return;
    };
    let session_id = session.session_id.clone();
    let queue_mode = *context.queue_mode.read();
    let media = selected_agent(context)
        .and_then(|agent| agent.capabilities)
        .map(|capabilities| (capabilities.input.clone(), capabilities.limits.clone()));
    let context = *context;
    context.set_page_error(None);

    if let Some(replaces) = replaces_turn_id.as_deref() {
        context.update_chat_items(|items| {
            items.retain(|item| item.turn_id.as_deref() != Some(replaces));
        });
    }
    let text = prompt.unwrap_or_default().to_owned();
    if !text.is_empty() || !files.is_empty() {
        let timestamp = crate::api::now_millis().unwrap_or(0);
        let mut label = text.clone();
        for file in &files {
            if !label.is_empty() {
                label.push_str("\n\n");
            }
            let _ = std::fmt::Write::write_fmt(&mut label, format_args!("[{}]", file.name()));
        }
        context.update_chat_items(|items| {
            items.push(ConversationItem {
                id: format!("local:{timestamp}"),
                turn_id: None,
                role: HistoryRole::User,
                kind: HistoryKind::User,
                content: label,
                created_at: timestamp,
                delivery: MessageDelivery::Sent,
                tool: None,
                usage: None,
            });
        });
    }
    context.set_chat_state(ChatState::Sending);

    spawn(async move {
        let uploaded = match upload_parts(&api, &session_id, files, media.as_ref()).await {
            Ok(parts) => parts,
            Err(error) => {
                context.set_page_error(Some(error.to_string()));
                context.set_chat_state(ChatState::Idle);
                return;
            }
        };
        let result = api
            .send_prompt(
                &session_id,
                (!text.is_empty()).then_some(text.as_str()),
                uploaded,
                replaces_turn_id.as_deref(),
                queue_mode.as_wire(),
            )
            .await;
        match result {
            Ok(turn_id) => {
                context.set_active_turn_id(Some(turn_id));
                if context.streaming_turn_id.read().is_none() {
                    context.set_chat_state(ChatState::Thinking);
                }
            }
            Err(error) => {
                // The optimistic bubble is still on screen. Without this it
                // looks delivered and the person waits for an answer that was
                // never asked for; marking it failed says what happened.
                context.update_chat_items(|items| {
                    if let Some(item) = items
                        .iter_mut()
                        .rev()
                        .find(|item| item.kind == HistoryKind::User && item.turn_id.is_none())
                    {
                        item.delivery = MessageDelivery::Failed;
                    }
                });
                context.clear_pending_turns();
                context.set_page_error(Some(error.to_string()));
                context.set_active_turn_id(None);
                context.set_chat_state(ChatState::Idle);
            }
        }
    });
}

#[derive(Debug, thiserror::Error)]
pub(super) enum UploadError {
    /// The browser gave no media type.
    #[error("The browser did not report a type for {0}.")]
    MissingType(String),
    /// The media type is one this client cannot send.
    #[error("{name}: {mime} is not supported.")]
    Unsupported { name: String, mime: String },
    /// The agent does not accept that modality.
    #[error("This agent does not accept {0} input.")]
    Modality(String),
    /// The file could not be read.
    #[error("Could not read {name}: {reason}")]
    Read { name: String, reason: String },
    /// The file is larger than the agent's bound.
    #[error("{name} is larger than the {limit}-byte limit.")]
    TooLarge { name: String, limit: u64 },
    /// The upload was refused.
    #[error("Uploading {name} failed: {reason}")]
    Upload { name: String, reason: String },
}

pub(super) async fn upload_parts(
    api: &ApiClient,
    session_id: &str,
    files: Vec<FileData>,
    media: Option<&(
        Vec<norte_code_wire::OutputModality>,
        std::collections::BTreeMap<
            norte_code_wire::OutputModality,
            norte_code_wire::AgentModalityLimits,
        >,
    )>,
) -> Result<Option<Vec<UserInputPart>>, UploadError> {
    if files.is_empty() {
        return Ok(None);
    }
    let (input_modalities, limits) = media
        .cloned()
        .unwrap_or_else(|| (Vec::new(), std::collections::BTreeMap::new()));
    let mut parts = Vec::new();
    for file in files {
        let name = file.name();
        let mime = file
            .content_type()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| UploadError::MissingType(name.clone()))?;
        let kind = input_part_kind(&mime).ok_or_else(|| UploadError::Unsupported {
            name: name.clone(),
            mime: mime.clone(),
        })?;
        let modality = modality_of(kind);
        if !input_modalities.contains(&modality) {
            return Err(UploadError::Modality(modality_word(modality).to_owned()));
        }
        let limit = limits.get(&modality);
        let bytes = file.read_bytes().await.map_err(|error| UploadError::Read {
            name: name.clone(),
            reason: error.to_string(),
        })?;
        if let Some(max) = limit.and_then(|limit| limit.max_bytes)
            && bytes.len() as u64 > max
        {
            return Err(UploadError::TooLarge {
                name: name.clone(),
                limit: max,
            });
        }
        let receipt = api
            .upload(session_id, &mime, bytes.to_vec())
            .await
            .map_err(|error| UploadError::Upload {
                name: name.clone(),
                reason: error.to_string(),
            })?;
        parts.push(UserInputPart::upload(
            kind,
            receipt.upload_id,
            receipt.mime_type,
        ));
    }
    Ok(Some(parts))
}

pub(super) fn selected_agent(context: &AppContext) -> Option<AgentSummary> {
    let agent_id = context
        .selected_session
        .read()
        .as_ref()
        .and_then(|session| session.agent.as_ref())
        .map(|agent| agent.id.clone())?;
    context
        .agents
        .read()
        .iter()
        .find(|agent| agent.id == agent_id)
        .cloned()
}

pub(super) fn attachment_accept(agent: &AgentSummary) -> String {
    use norte_code_wire::OutputModality;

    let Some(capabilities) = agent.capabilities.as_ref() else {
        return String::new();
    };
    let mut accepted = std::collections::BTreeSet::new();
    for (modality, fallback) in [
        (OutputModality::Image, "image/*"),
        (
            OutputModality::Document,
            "application/pdf,text/plain,text/markdown",
        ),
        (OutputModality::Audio, "audio/*"),
        (OutputModality::Video, "video/*"),
    ] {
        if !capabilities.input.contains(&modality) {
            continue;
        }
        match capabilities.limits.get(&modality) {
            Some(limits) if !limits.mime_types.is_empty() => {
                accepted.extend(limits.mime_types.iter().cloned());
            }
            _ => accepted.extend(fallback.split(',').map(str::to_owned)),
        }
    }
    accepted.into_iter().collect::<Vec<_>>().join(",")
}

pub(super) fn input_part_kind(mime: &str) -> Option<InputPartKind> {
    if mime.starts_with("image/") {
        Some(InputPartKind::Image)
    } else if mime.starts_with("audio/") {
        Some(InputPartKind::Audio)
    } else if mime.starts_with("video/") {
        Some(InputPartKind::Video)
    } else if matches!(
        mime,
        "application/pdf" | "text/plain" | "text/markdown" | "application/json"
    ) {
        Some(InputPartKind::Document)
    } else {
        None
    }
}

pub(super) fn modality_of(kind: InputPartKind) -> norte_code_wire::OutputModality {
    match kind {
        InputPartKind::Text => norte_code_wire::OutputModality::Text,
        InputPartKind::Image => norte_code_wire::OutputModality::Image,
        InputPartKind::Document => norte_code_wire::OutputModality::Document,
        InputPartKind::Audio => norte_code_wire::OutputModality::Audio,
        InputPartKind::Video => norte_code_wire::OutputModality::Video,
    }
}

/// The word a person reads for a modality.
pub(super) fn modality_word(modality: norte_code_wire::OutputModality) -> &'static str {
    match modality {
        norte_code_wire::OutputModality::Text => "text",
        norte_code_wire::OutputModality::Image => "image",
        norte_code_wire::OutputModality::Document => "document",
        norte_code_wire::OutputModality::Audio => "audio",
        norte_code_wire::OutputModality::Video => "video",
    }
}

#[cfg(test)]
mod reconcile_tests {
    use super::*;

    fn tool_row(call_id: &str, name: &str, output: &str) -> ConversationItem {
        ConversationItem {
            id: format!("tool:{call_id}"),
            turn_id: Some("turn_1".to_owned()),
            role: HistoryRole::Tool,
            kind: HistoryKind::ToolCall,
            content: String::new(),
            created_at: 1,
            delivery: MessageDelivery::Sent,
            tool: Some(ToolActivity {
                call_id: call_id.to_owned(),
                name: name.to_owned(),
                path: None,
                arguments: serde_json::Value::Null,
                output: output.to_owned(),
                status: ToolActivityStatus::Completed,
                duration_ms: None,
                result_summary: None,
            }),
            usage: None,
        }
    }

    /// A row exactly as `HistoryPage` decodes it: the call is the text
    /// `name {json}`, and there is no live activity attached.
    fn historical_tool_row(_call_id: &str, name: &str) -> ConversationItem {
        ConversationItem {
            id: format!("history:{name}"),
            turn_id: Some("turn_1".to_owned()),
            role: HistoryRole::Tool,
            kind: HistoryKind::ToolCall,
            content: format!("{name} {{\"path\":\"a\"}}"),
            created_at: 1,
            delivery: MessageDelivery::Sent,
            tool: None,
            usage: None,
        }
    }

    fn answer(id: &str, content: &str, created_at: i64) -> ConversationItem {
        ConversationItem {
            id: id.to_owned(),
            turn_id: Some("turn_1".to_owned()),
            role: HistoryRole::Assistant,
            kind: HistoryKind::Assistant,
            content: content.to_owned(),
            created_at,
            delivery: MessageDelivery::Sent,
            tool: None,
            usage: None,
        }
    }

    fn stream_bubble(turn: &str, content: &str, created_at: i64) -> ConversationItem {
        ConversationItem {
            id: format!("stream:{turn}:text"),
            turn_id: Some(turn.to_owned()),
            role: HistoryRole::Assistant,
            kind: HistoryKind::Assistant,
            content: content.to_owned(),
            created_at,
            delivery: MessageDelivery::Sent,
            tool: None,
            usage: None,
        }
    }

    #[test]
    fn an_answer_still_printing_survives_a_stale_history_snapshot() {
        // The first turn is committed; the next one is already printing when
        // the older history read lands. Its text must not be replaced by a
        // snapshot that predates it.
        let committed = vec![answer("h1", "first answer", 1_000)];
        let live = vec![stream_bubble("2", "second answer", 5_000)];
        let kept = live_answers_missing_from(&committed, live);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].content, "second answer");
    }

    #[test]
    fn an_answer_history_already_has_is_not_shown_twice() {
        let committed = vec![answer("h1", "done", 5_000)];
        let live = vec![stream_bubble("turn_1", "done", 4_000)];
        assert_eq!(live_answers_missing_from(&committed, live).len(), 0);
    }

    #[test]
    fn a_tool_call_in_history_stays_in_the_transcript() {
        let fresh = vec![historical_tool_row("call-1", "read")];
        let mut merged = fresh.clone();
        merged.extend(live_tools_missing_from(&fresh, Vec::new()));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].kind, HistoryKind::ToolCall);
        assert_eq!(
            merged[0].content, "read {\"path\":\"a\"}",
            "the call is still there, with its arguments"
        );
    }

    #[test]
    fn a_live_tool_the_history_lacks_is_kept() {
        let fresh = vec![historical_tool_row("call-1", "read")];
        let mut merged = fresh.clone();
        merged.extend(live_tools_missing_from(
            &fresh,
            vec![tool_row("call-2", "shell", "output")],
        ));
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn a_live_tool_history_already_has_is_not_duplicated() {
        let fresh = vec![historical_tool_row("call-1", "read")];
        let mut merged = fresh.clone();
        merged.extend(live_tools_missing_from(
            &fresh,
            vec![tool_row("call-1", "read", "output")],
        ));
        assert_eq!(merged.len(), 1);
    }
}

#[cfg(test)]
mod grouping_tests {
    use super::*;

    fn item(id: &str, kind: HistoryKind, content: &str) -> ConversationItem {
        ConversationItem {
            id: id.to_owned(),
            turn_id: Some("turn_1".to_owned()),
            role: HistoryRole::Assistant,
            kind,
            content: content.to_owned(),
            created_at: 0,
            delivery: MessageDelivery::Sent,
            tool: None,
            usage: None,
        }
    }

    #[test]
    fn a_run_of_reads_folds_into_one_exploring_row() {
        let rows = group_tool_rows(vec![
            item("1", HistoryKind::ToolCall, "read {\"path\":\"a\"}"),
            item("2", HistoryKind::ToolCall, "grep {\"pattern\":\"b\"}"),
            item("3", HistoryKind::Assistant, "done"),
        ]);
        assert_eq!(rows.len(), 2);
        assert!(matches!(&rows[0], TranscriptRow::Exploring(rows) if rows.len() == 2));
    }

    #[test]
    fn a_single_read_is_not_a_group() {
        let rows = group_tool_rows(vec![
            item("1", HistoryKind::ToolCall, "read {\"path\":\"a\"}"),
            item("2", HistoryKind::Assistant, "done"),
        ]);
        assert!(matches!(&rows[0], TranscriptRow::Single(_)));
    }

    #[test]
    fn a_write_between_reads_ends_the_group() {
        let rows = group_tool_rows(vec![
            item("1", HistoryKind::ToolCall, "read {}"),
            item("2", HistoryKind::ToolCall, "shell {\"command\":\"ls\"}"),
            item("3", HistoryKind::ToolCall, "read {}"),
        ]);
        assert_eq!(rows.len(), 3);
        assert!(matches!(&rows[1], TranscriptRow::Single(item) if item.id == "2"));
    }
}
