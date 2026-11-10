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
        info.capabilities
            .contains(&crate::domain::Capability::Uploads)
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
    // down and reload the history mid-answer — which is exactly what made the
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

/// One drawn line of the transcript.
#[derive(Clone)]
pub enum TranscriptRow {
    /// A note the page itself wrote (compaction, rewind).
    Note(ConversationItem),
    /// A message, a reasoning block, or a single tool call.
    Single(ConversationItem),
    /// A run of read-only calls, collapsed into one "Exploring" block.
    Exploring(Vec<ConversationItem>),
}

/// Tools whose calls only read: a run of them is one step of looking around,
/// not twenty separate events, so they fold into a single expandable block.
fn is_exploration(item: &ConversationItem) -> bool {
    let name = item
        .tool
        .as_ref()
        .map(|tool| tool.name.as_str())
        .or_else(|| item.content.split([' ', '{']).next())
        .unwrap_or_default();
    matches!(
        name,
        "read" | "grep" | "glob" | "list_dir" | "ls" | "search" | "find"
    )
}

fn group_tool_rows(items: Vec<ConversationItem>) -> Vec<TranscriptRow> {
    let mut rows = Vec::new();
    let mut run: Vec<ConversationItem> = Vec::new();
    let flush = |run: &mut Vec<ConversationItem>, rows: &mut Vec<TranscriptRow>| {
        if run.is_empty() {
            return;
        }
        let taken = std::mem::take(run);
        if taken.len() == 1 {
            rows.push(TranscriptRow::Single(taken[0].clone()));
        } else {
            rows.push(TranscriptRow::Exploring(taken));
        }
    };
    for item in items {
        if item.kind == HistoryKind::SystemEvent {
            flush(&mut run, &mut rows);
            rows.push(TranscriptRow::Note(item));
        } else if matches!(item.kind, HistoryKind::ToolCall | HistoryKind::ToolResult)
            && is_exploration(&item)
        {
            run.push(item);
        } else {
            flush(&mut run, &mut rows);
            rows.push(TranscriptRow::Single(item));
        }
    }
    flush(&mut run, &mut rows);
    rows
}

/// A run of read-only calls: one line until the reader opens it.
#[component]
fn ExploringGroup(rows: Vec<ConversationItem>) -> Element {
    let counts = rows
        .iter()
        .filter_map(|item| {
            item.tool
                .as_ref()
                .map(|tool| tool.name.clone())
                .or_else(|| item.content.split([' ', '{']).next().map(str::to_owned))
        })
        .fold(std::collections::BTreeMap::new(), |mut counts, name| {
            *counts.entry(name).or_insert(0usize) += 1;
            counts
        });
    let summary = counts
        .iter()
        .map(|(name, count)| format!("{count}× {name}"))
        .collect::<Vec<_>>()
        .join(", ");
    rsx! {
        details { class: "tool exploring",
            summary { class: "tool-line",
                span { class: "tool-name", "Exploring" }
                span { class: "tool-target", "{summary}" }
                span { class: "tool-state tool-state-done", "{rows.len()}" }
            }
            div { class: "exploring-rows",
                for item in rows {
                    MessageItem {
                        key: "{item.id}",
                        item: item,
                        show_thinking: true,
                        show_timestamp: false,
                        on_rewind: move |_| {},
                        on_replace: move |_| {},
                        on_fork: move |_| {},
                        on_pin: move |_| {},
                        pinned: false,
                    }
                }
            }
        }
    }
}

/// The session's plans: what the model proposed and where each one stands.
///
/// A pending plan is approved or rejected by answering the question the
/// `plan` tool asks, so the card in the transcript is the decision point and
/// this pane is the record of what was decided.
#[component]
fn plans_panel() -> Element {
    let context = use_context::<AppContext>();
    let session_id = context
        .selected_session
        .read()
        .as_ref()
        .map(|session| session.session_id.clone())
        .unwrap_or_default();
    let mut plans = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    use_effect(use_reactive((&session_id,), move |(session_id,)| {
        if session_id.is_empty() {
            return;
        }
        spawn(async move {
            loading.set(true);
            if let Some(client) = api(&context)
                && let Ok(list) = client.plans(&session_id).await
            {
                plans.set(list);
            }
            loading.set(false);
        });
    }));

    let listed = plans.read().clone();
    if *loading.read() {
        return rsx! { p { class: "helper-text", "Loading plans…" } };
    }
    if listed.is_empty() {
        return rsx! {
            EmptyState {
                title: "No plans",
                message: "When the model proposes a plan, it appears here and in the transcript for approval."
            }
        };
    }
    rsx! {
        div { class: "session-list",
            for plan in listed {
                details { class: "tool", key: "{plan.id}",
                    summary { class: "tool-line",
                        span { class: "tool-name", "plan" }
                        span { class: "tool-target",
                            "{plan.title.clone().or_else(|| plan.content.lines().next().map(str::to_owned)).unwrap_or_else(|| plan.id.clone())}"
                        }
                        span { class: plan_status_class(plan.status), "{plan.status.label()}" }
                    }
                    if let Some(model) = &plan.model {
                        p { class: "helper-text", "{model}" }
                    }
                    if *context.show_thinking.read() || plan.status != norte_code_wire::PlanStatus::Pending {
                        div { class: "markdown-body",
                            dangerous_inner_html: crate::components::render_markdown(&plan.content)
                        }
                    }
                }
            }
        }
    }
}

fn plan_status_class(status: norte_code_wire::PlanStatus) -> &'static str {
    match status {
        norte_code_wire::PlanStatus::Active => "status status-active",
        norte_code_wire::PlanStatus::Pending => "status status-warning",
        norte_code_wire::PlanStatus::Canceled => "status status-muted",
    }
}

#[component]
fn jobs_panel() -> Element {
    let context = use_context::<AppContext>();
    let session_id = context
        .selected_session
        .read()
        .as_ref()
        .map(|session| session.session_id.clone())
        .unwrap_or_default();
    // The live stream keeps running jobs current; asking the host once on open
    // fills in the ones that finished before this page was watching.
    use_effect(use_reactive((&session_id,), move |(session_id,)| {
        if session_id.is_empty() {
            return;
        }
        spawn(async move {
            if let Some(client) = api(&context)
                && let Ok(jobs) = client.list_jobs(&session_id).await
            {
                context.set_jobs(jobs);
            }
        });
    }));
    let jobs = context.jobs.read().clone();
    if jobs.is_empty() {
        return rsx! {
            EmptyState { title: "No jobs", message: "Detached tool calls show up here while they run." }
        };
    }
    rsx! {
        div { class: "job-list",
            for job in jobs {
                details {
                    class: if job.status == norte_code_wire::JobStatus::Running { "tool job job-running" } else { "tool job" },
                    key: "{job.job_id}",
                    open: job.status == norte_code_wire::JobStatus::Running,
                    summary { class: "tool-line",
                        span { class: "tool-name", "shell" }
                        span { class: "tool-target", "{job.command}" }
                        if let Some(ms) = job.duration_ms {
                            span { class: "tool-time", "{ms} ms" }
                        }
                        span {
                            class: if job.status == norte_code_wire::JobStatus::Running {
                                "tool-state tool-state-running"
                            } else {
                                "tool-state tool-state-done"
                            },
                            "{job.status.label()}"
                        }
                    }
                    // The tail is the point: a running job streams its output,
                    // and the panel follows it to the end as it grows.
                    if let Some(output) = &job.output_snapshot {
                        pre { class: "tool-output job-tail", "{output}" }
                    } else {
                        p { class: "helper-text", "No output yet." }
                    }
                    if job.output_truncated {
                        p { class: "helper-text", "Earlier output was truncated." }
                    }
                }
            }
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::needless_pass_by_value
)]
fn transcript(
    context: &AppContext,
    session: &norte_code_wire::SessionSummary,
    items: &[ConversationItem],
    todos: Vec<crate::domain::SessionTodo>,
    questions: Vec<PendingQuestion>,
    pinned: Vec<String>,
    show_thinking: bool,
    show_timestamp: bool,
    streaming: Option<String>,
    mut draft: Signal<String>,
    mut replacing: Signal<Option<String>>,
    mut attachments: Signal<Vec<FileData>>,
    uploads_supported: bool,
    attachment_accept: String,
    queue_mode: QueueMode,
    chat_state: ChatState,
) -> Element {
    let context = *context;
    let busy = chat_state.is_busy();
    let stoppable = chat_state.has_running_turn() && context.active_turn_id.read().is_some();
    let awaiting_answer = !questions.is_empty();
    // The transcript is its own scroll container: the composer and the
    // header stay put, and the log follows the answer as it prints.
    let mut stick_to_bottom = use_signal(|| true);
    // What changes as the transcript grows: the number of rows, and the total
    // number of characters. Watching only the last row misses the common case
    // — a tool row at the bottom while the answer above it keeps printing.
    let rows = items.len();
    let total_chars = items.iter().map(|item| item.content.len()).sum::<usize>();
    let busy_rows = busy.then_some(chat_state.label());
    let follow = *stick_to_bottom.read();
    use_effect(use_reactive(
        (&rows, &total_chars, &streaming, &busy_rows, &follow),
        move |(rows, chars, streaming, activity, stick)| {
            let _ = (rows, chars, streaming, activity);
            if !stick {
                return;
            }
            spawn(async move {
                // One frame later: the DOM has the new text by then.
                gloo_timeout().await;
                scroll_transcript_to_end();
            });
        },
    ));

    rsx! {
        TodoPanel { todos: todos.clone() }
        div {
            class: "transcript",
            id: "transcript",
            onscroll: move |_| stick_to_bottom.set(at_transcript_bottom()),
            if items.is_empty() && !chat_state.is_busy() {
                EmptyState {
                    title: "Start the conversation",
                    message: "Type below. The answer streams in here as the model produces it."
                }
            } else {
                div { class: "timeline",
                    for row in group_tool_rows(
                        items
                            .iter()
                            .filter(|item| show_thinking || item.kind != HistoryKind::Reasoning)
                            .cloned()
                            .collect(),
                    ) {
                        match row {
                            TranscriptRow::Single(item) => rsx! {
                                MessageItem {
                                    key: "{item.id}",
                                    item: item.clone(),
                                    show_thinking,
                                    show_timestamp,
                                    on_rewind: move |turn| rewind_to(&context, turn),
                                    on_replace: move |item| start_replace(&context, item, draft, replacing),
                                    on_fork: move |turn| fork_from(&context, turn),
                                    on_pin: move |turn| toggle_pin(&context, turn),
                                    pinned: item.turn_id.as_ref().is_some_and(|turn| pinned.contains(turn)),
                                }
                            },
                            TranscriptRow::Note(item) => rsx! {
                                div { class: "transcript-note", key: "{item.id}", "{item.content}" }
                            },
                            TranscriptRow::Exploring(rows) => rsx! {
                                ExploringGroup { key: "explore-{rows[0].id}", rows: rows.clone() }
                            },
                        }
                    }
                    for job in context.jobs.read().iter().filter(|job| {
                        job.status == norte_code_wire::JobStatus::Running
                    }) {
                        JobRow { key: "{job.job_id}", job: job.clone() }
                    }
                }
                // The question is where the answer would go: the model is
                // parked on it, so it belongs at the end of the conversation,
                // not above it.
                for question in questions {
                    QuestionCard {
                        key: "{question.id}",
                        question: question.clone(),
                        on_answer: move |answer| answer_question(&context, question.clone(), answer),
                        on_cancel: move |()| stop_turn(&context),
                    }
                }
                if busy && !awaiting_answer {
                    div { class: "transcript-activity",
                        span { class: "thinking-dot" }
                        "{chat_state.label()}"
                    }
                }
            }
        }
        div { class: "composer",
            if replacing.read().is_some() {
                div { class: "replace-hint",
                    span { "Editing a message — sending will replace that turn." }
                    button {
                        class: "link-button",
                        onclick: move |_| replacing.set(None),
                        "Cancel"
                    }
                }
            }
            if !attachments.read().is_empty() {
                div { class: "attachment-list",
                    for (index, file) in attachments.read().iter().enumerate() {
                        button {
                            class: "attachment-chip",
                            key: "{index}",
                            title: "Remove",
                            onclick: move |_| attachments.with_mut(|files| { files.remove(index); }),
                            "{file.name()} ×"
                        }
                    }
                }
            }
            textarea {
                value: "{draft}",
                placeholder: if streaming.is_some() { "Send another message to steer the running turn…" } else { "Message the agent…" },
                rows: "1",
                oninput: move |event| draft.set(event.value()),
                onkeydown: move |event| {
                    if event.key() == Key::Enter && !event.modifiers().shift() {
                        event.prevent_default();
                        let prompt = draft.read().trim().to_owned();
                        let files = attachments.read().clone();
                        if !prompt.is_empty() || !files.is_empty() {
                            draft.set(String::new());
                            attachments.set(Vec::new());
                            let target = replacing.read().clone();
                            replacing.set(None);
                            send(&context, Some(prompt.as_str()), files, target);
                        }
                    }
                }
            }
            div { class: "composer-bar",
                div { class: "composer-left",
                    if uploads_supported {
                        label { class: "icon-button", title: "Attach a file",
                            "＋"
                            input {
                                class: "attachment-input",
                                r#type: "file",
                                multiple: true,
                                accept: "{attachment_accept}",
                                onchange: move |event| attachments.set(event.files())
                            }
                        }
                    }
                    // What a message typed during a turn does. It is a
                    // property of the session, so it sits with the other
                    // per-session controls and is set before it is needed,
                    // not only while a turn happens to be running.
                    select {
                        class: "composer-chip",
                        title: "What a message sent during a turn does",
                        value: "{queue_mode.as_wire()}",
                        onchange: move |event| {
                            let value = event.value();
                            let mode = match value.as_str() {
                                "interrupt" => QueueMode::Interrupt,
                                "queue" => QueueMode::Queue,
                                _ => QueueMode::Reject,
                            };
                            context.set_queue_mode(mode);
                        },
                        for mode in QueueMode::ALL {
                            option { value: "{mode.as_wire()}", "{mode.label()}" }
                        }
                    }
                }
                div { class: "composer-right",
                    ComposerReasoning { session_id: session.session_id.clone() },
                    ComposerModel { session_id: session.session_id.clone() },
                    if stoppable {
                        button {
                            class: "composer-stop",
                            title: "Stop the running turn",
                            onclick: move |_| stop_turn(&context),
                            "Stop"
                        }
                    }
                    button {
                        class: "send-button",
                        disabled: draft.read().trim().is_empty() && attachments.read().is_empty(),
                        onclick: move |_| {
                            let prompt = draft.read().trim().to_owned();
                            let files = attachments.read().clone();
                            if !prompt.is_empty() || !files.is_empty() {
                                draft.set(String::new());
                                attachments.set(Vec::new());
                                let target = replacing.read().clone();
                                replacing.set(None);
                                send(&context, Some(prompt.as_str()), files, target);
                            }
                        },
                        "Send"
                    }
                }
            }
        }
    }
}

/// How much the session reasons, next to the model.
///
/// The setting is the session's, so it is changed here and applies to every
/// later message; `Off` is a real choice for models that accept it.
#[component]
fn ComposerReasoning(session_id: String) -> Element {
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
fn reasoning_choice(value: &norte_code_wire::Reasoning) -> String {
    match value {
        norte_code_wire::Reasoning::Disabled => "disabled".to_owned(),
        norte_code_wire::Reasoning::Effort { effort } => effort.clone(),
        // A token budget has no effort level to show, so it reads as auto.
        norte_code_wire::Reasoning::Budget { .. } | norte_code_wire::Reasoning::Automatic => {
            "automatic".to_owned()
        }
    }
}

/// The model the next message will run on, next to Send.
#[component]
fn ComposerModel(session_id: String) -> Element {
    let context = use_context::<AppContext>();
    let models = use_signal(Vec::new);
    // A `<select>` shows the option whose `value` matches its own `value`; when
    // none does, the browser falls back to the first option. The key therefore
    // has to be the same one the options use — provider and model — or the
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
            // The session may run on a model the catalogue does not list — a
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

fn human_model(provider: &str, name: &str) -> String {
    if provider.is_empty() {
        name.to_owned()
    } else {
        format!("{name} · {provider}")
    }
}

/// Waits, on the wasm timer or a blocking thread natively.
#[cfg_attr(not(target_arch = "wasm32"), allow(clippy::unused_async))]
async fn sleep(duration: std::time::Duration) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let promise = js_sys::Promise::new(&mut |resolve, _reject| {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    &resolve,
                    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX),
                );
            });
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::sleep(duration);
    }
}

/// Waits one frame so the DOM has the appended text before scrolling.
#[cfg_attr(not(target_arch = "wasm32"), allow(clippy::unused_async))]
async fn gloo_timeout() {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(window) = web_sys::window() {
            let promise = js_sys::Promise::new(&mut |resolve, _reject| {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
            });
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Native runs render synchronously enough; nothing to wait for.
    }
}

/// Whether the transcript is scrolled to (or near) its bottom.
fn at_transcript_bottom() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast as _;
        if let Some(document) = web_sys::window().and_then(|window| window.document())
            && let Some(element) = document.get_element_by_id("transcript")
            && let Ok(element) = element.dyn_into::<web_sys::HtmlElement>()
        {
            let distance = element.scroll_height() - element.scroll_top() - element.client_height();
            return distance < 48;
        }
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        true
    }
}

fn scroll_transcript_to_end() {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast as _;
        if let Some(document) = web_sys::window().and_then(|window| window.document())
            && let Some(element) = document.get_element_by_id("transcript")
            && let Ok(element) = element.dyn_into::<web_sys::HtmlElement>()
        {
            element.set_scroll_top(element.scroll_height());
        }
    }
}

/// One question the model is waiting on.
///
/// It is the turn's only way forward: the answer unparks it, and cancelling is
/// the other exit. Both leave the card — one by being answered, one by the
/// turn ending.
#[component]
fn QuestionCard(
    question: PendingQuestion,
    on_answer: EventHandler<String>,
    on_cancel: EventHandler<()>,
) -> Element {
    // With `multiple`, the choices are a checklist and the answer is what is
    // ticked when the person confirms; with a single choice, clicking is the
    // answer.
    let mut chosen = use_signal(Vec::<String>::new);
    let mut custom_answer = use_signal(String::new);
    let multiple = question.multiple;
    let options = question.options.clone();

    #[allow(clippy::redundant_closure_for_method_calls)]
    let submit = {
        let options = options.clone();
        move || {
            let free = custom_answer.read().trim().to_owned();
            let answer = if free.is_empty() {
                let picked = chosen.read().join(", ");
                if picked.is_empty() && !options.is_empty() {
                    return;
                }
                picked
            } else {
                free
            };
            if !answer.is_empty() {
                on_answer.call(answer);
            }
        }
    };

    rsx! {
        article { class: "question",
            p { class: "question-label", "The agent needs your input" }
            p { class: "question-prompt", "{question.prompt}" }
            if !options.is_empty() {
                div { class: "button-row",
                    for option in options {
                        button {
                            class: if chosen.read().contains(&option) { "chip chip-selected" } else { "chip" },
                            onclick: {
                                let value = option.clone();
                                move |_| {
                                    if multiple {
                                        chosen.with_mut(|picked| {
                                            if let Some(index) = picked.iter().position(|entry| entry == &value) {
                                                picked.remove(index);
                                            } else {
                                                picked.push(value.clone());
                                            }
                                        });
                                    } else {
                                        chosen.set(vec![value.clone()]);
                                        on_answer.call(value.clone());
                                    }
                                }
                            },
                            "{option}"
                        }
                    }
                }
            }
            div { class: "question-freeform",
                input {
                    value: "{custom_answer}",
                    placeholder: "Or answer in your own words",
                    oninput: move |event| custom_answer.set(event.value()),
                    onkeydown: {
                        let submit = submit.clone();
                        move |event| {
                            if event.key() == Key::Enter {
                                submit();
                            }
                        }
                    },
                }
                button {
                    class: "button-secondary",
                    onclick: move |_| submit(),
                    if multiple { "Send answer" } else { "Reply" }
                }
                button {
                    class: "link-button",
                    title: "Stop the turn instead of answering",
                    onclick: move |_| on_cancel.call(()),
                    "Cancel turn"
                }
            }
        }
    }
}

/// Opens the session's stream, loads its history, then keeps reading frames
/// until the page is torn down.
///
/// A stream that ends or breaks is reopened: the server closes an idle stream,
/// restarts, and closes one whose attachment was taken over. Leaving it closed
/// is what made the page "stop receiving anything" — the turn ran, but nothing
/// was listening. Reconnecting reloads the history, so whatever happened while
/// the stream was down is still shown.
#[allow(clippy::large_types_passed_by_value)]
async fn run_conversation(context: AppContext, request: ConversationRequest) {
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

fn still_selected(context: &AppContext, session_id: &str) -> bool {
    context
        .selected_session
        .read()
        .as_ref()
        .is_some_and(|session| session.session_id == session_id)
}

/// Reloads history into the page, keeping what the stream measured.
async fn load_history(context: &AppContext, api: &ApiClient, session_id: &str) {
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
async fn read_conversation(
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
        apply(*context, &frame);
    }
}

#[allow(clippy::too_many_lines, clippy::large_types_passed_by_value)]
fn apply(context: AppContext, frame: &raep_types::Envelope<serde_json::Value>) {
    let turn_of_frame = frame.turn_id.as_ref().map(ToString::to_string);
    match chat_client::decode_frame(frame) {
        DecodedFrame::Accepted { turn_id, queued } => {
            // The server names the turn. A queued one waits behind the turn
            // already running: it must not steal the streaming id, or the
            // answer in flight stops drawing — and it must not be remembered
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
            // A turn this page is not driving — another client's, a replay, or
            // the turn a finished job started — is adopted by the id its frame
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
                TurnPhase::Completed
                | TurnPhase::Cancelled
                | TurnPhase::Failed
                | TurnPhase::Stopped => None,
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
            // the turn did not finish cleanly — a failure or a stop can leave
            // the live view short of what was committed — or when nothing was
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
                        created_at: frame.timestamp,
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
            // The fork's frames land on the fork's own stream, which this page
            // is not reading; `fork_from` switches session from the HTTP reply.
            // Seeing it here means another client forked this session — add it
            // to the list so it is there when the person looks.
            if let Some(new_id) = new_session_id.filter(|id| !id.is_empty()) {
                let title = from
                    .and_then(|from| {
                        context
                            .sessions
                            .read()
                            .iter()
                            .find(|entry| entry.session_id == from)
                            .and_then(|entry| entry.title.clone())
                    })
                    .map(|title| format!("{title} (fork)"));
                let turn = from_turn_id.map(|turn| format!("Forked at {turn}"));
                context.update_sessions(|sessions| {
                    if !sessions.iter().any(|entry| entry.session_id == new_id) {
                        sessions.insert(
                            0,
                            norte_code_wire::SessionSummary {
                                session_id: new_id,
                                title: title.clone().or(turn),
                                ..Default::default()
                            },
                        );
                    }
                });
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
            // Answered here or from another client — either way it is no
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
fn refresh_history(context: &AppContext) {
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
fn refresh_sessions(context: &AppContext) {
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

/// The usage each turn reported, keyed by turn id, as measured live.
fn usage_by_turn(
    context: &AppContext,
) -> std::collections::HashMap<String, crate::domain::TurnUsage> {
    let mut measured = std::collections::HashMap::new();
    for item in context.chat_items.read().iter() {
        if let (Some(turn), Some(usage)) = (&item.turn_id, &item.usage) {
            measured
                .entry(turn.clone())
                .and_modify(|existing: &mut crate::domain::TurnUsage| existing.merge(usage))
                .or_insert_with(|| usage.clone());
        }
    }
    measured
}

/// Joins the server's canonical history with what the live stream built.
///
/// History is the source of record for what was said and which tools ran; the
/// stream is the source for what history cannot carry — the measured cost and
/// speed of each turn. Empty items are dropped: a tool call the stream opened
/// and history never got (it was cancelled) would otherwise render as a blank
/// row.
/// The live answer rows history has not committed.
///
/// Committed rows are matched by content, not by turn id: the stream names a
/// turn by the id this page minted or by the service's number, and history
/// names it `turn_N`, so the two never compare equal. A live bubble is kept
/// only when it is newer than everything committed and says something history
/// does not — that is the answer still being written.
fn live_answers_missing_from(
    merged: &[ConversationItem],
    live: Vec<ConversationItem>,
) -> Vec<ConversationItem> {
    let newest_committed = merged.iter().map(|item| item.created_at).max().unwrap_or(0);
    let committed_text: std::collections::HashSet<&str> = merged
        .iter()
        .filter(|item| matches!(item.kind, HistoryKind::Assistant | HistoryKind::Reasoning))
        .map(|item| item.content.as_str())
        .collect();
    live.into_iter()
        .filter(|item| {
            matches!(item.kind, HistoryKind::Assistant | HistoryKind::Reasoning)
                && item.id.starts_with("stream:")
                && item.created_at > newest_committed
                && !committed_text.contains(item.content.as_str())
        })
        .collect()
}

/// The live tool rows whose call history does not carry.
///
/// History names a call only by the text it replayed — `name {json}`, with no
/// id and no live activity — so the two sides are matched by tool name within
/// the same turn. A call in flight today is rendered as a live row; the same
/// call after the answer lands is a history row, and carrying both would show
/// it twice: once as it was and once as it is.
fn live_tools_missing_from(
    merged: &[ConversationItem],
    live: Vec<ConversationItem>,
) -> Vec<ConversationItem> {
    live.into_iter()
        .filter(|item| {
            if !matches!(item.kind, HistoryKind::ToolCall | HistoryKind::ToolResult) {
                return false;
            }
            let Some(activity) = &item.tool else {
                return false;
            };
            !merged.iter().any(|historical| {
                historical.kind == HistoryKind::ToolCall
                    && historical.turn_id == item.turn_id
                    && historical
                        .content
                        .trim_start()
                        .split([' ', '{'])
                        .next()
                        .is_some_and(|name| name == activity.name)
            })
        })
        .collect()
}

fn reconcile(
    fresh: Vec<ConversationItem>,
    context: &AppContext,
    measured: &std::collections::HashMap<String, crate::domain::TurnUsage>,
) -> Vec<ConversationItem> {
    let live = context.chat_items.read().clone();
    // History is the record of what happened, tools included. It used to be
    // stripped of every tool row on the theory that the live stream had them —
    // so the calls vanished from the transcript the moment the answer landed.
    // What history cannot carry is the *output* a live stream showed while the
    // call ran, and empty text: those two things are taken from the live side.
    let mut merged: Vec<ConversationItem> = fresh
        .into_iter()
        .filter(|item| item.usage.is_some() || !item.content.trim().is_empty())
        .collect();
    // A tool row the stream opened but history has not committed yet (the
    // turn is still running) stays where it is.
    merged.extend(live_tools_missing_from(&merged, live.clone()));
    // The same for the answer itself. A history read can land while the next
    // turn is already printing — the end of one turn and the start of the
    // next are milliseconds apart — and replacing the transcript with a
    // snapshot that predates those deltas erased the answer until a reload.
    // A live bubble whose turn history does not carry is kept, in its place.
    merged.extend(live_answers_missing_from(&merged, live));
    // The measurement belongs to the answer the turn produced, not to every
    // row the turn touched: it was being attached to the user's message and to
    // the reasoning block as well, so one request's cost printed three times.
    for item in &mut merged {
        if item.kind != HistoryKind::Assistant {
            item.usage = None;
            continue;
        }
        if let Some(turn) = &item.turn_id {
            item.usage = measured.get(turn).cloned();
        }
    }
    merged.sort_by_key(|item| item.created_at);
    merged
}

fn send(
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
                context.set_page_error(Some(error));
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

async fn upload_parts(
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
) -> Result<Option<Vec<UserInputPart>>, String> {
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
            .ok_or_else(|| format!("The browser did not report a type for {name}."))?;
        let kind =
            input_part_kind(&mime).ok_or_else(|| format!("{name}: {mime} is not supported."))?;
        let modality = modality_of(kind);
        if !input_modalities.contains(&modality) {
            return Err(format!(
                "This agent does not accept {} input.",
                modality_word(modality)
            ));
        }
        let limit = limits.get(&modality);
        let bytes = file
            .read_bytes()
            .await
            .map_err(|error| format!("Could not read {name}: {error}"))?;
        if let Some(max) = limit.and_then(|limit| limit.max_bytes)
            && bytes.len() as u64 > max
        {
            return Err(format!("{name} is larger than the {max}-byte limit."));
        }
        let receipt = api
            .upload(session_id, &mime, bytes.to_vec())
            .await
            .map_err(|error| format!("Uploading {name} failed: {error}"))?;
        parts.push(UserInputPart::upload(
            kind,
            receipt.upload_id,
            receipt.mime_type,
        ));
    }
    Ok(Some(parts))
}

/// Rewinds so the chosen message is the last thing said, then lets the same
/// stream continue: the turn that follows is new work, not a replay.
fn rewind_to(context: &AppContext, turn: String) {
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
fn start_replace(
    _context: &AppContext,
    item: ConversationItem,
    mut draft: Signal<String>,
    mut replacing: Signal<Option<String>>,
) {
    draft.set(item.content);
    replacing.set(item.turn_id.clone());
}

fn fork_from(context: &AppContext, turn: Option<String>) {
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

fn toggle_pin(context: &AppContext, turn: String) {
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

fn compact(context: &AppContext, session_id: String) {
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

fn stop_turn(context: &AppContext) {
    let Some(api) = api(context) else {
        return;
    };
    let Some(session) = context.selected_session.read().clone() else {
        return;
    };
    let Some(turn_id) = context.active_turn_id.read().clone() else {
        return;
    };
    let context = *context;
    spawn(async move {
        if let Err(error) = api.cancel_turn(&session.session_id, &turn_id).await {
            context.set_page_error(Some(format!("Could not stop the turn: {error}")));
        }
        context.set_streaming_turn_id(None);
        context.set_chat_state(ChatState::Idle);
    });
}

fn answer_question(context: &AppContext, question: PendingQuestion, answer: String) {
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

fn api(context: &AppContext) -> Option<ApiClient> {
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

fn selected_agent(context: &AppContext) -> Option<AgentSummary> {
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

fn attachment_accept(agent: &AgentSummary) -> String {
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

fn input_part_kind(mime: &str) -> Option<InputPartKind> {
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

fn modality_of(kind: InputPartKind) -> norte_code_wire::OutputModality {
    match kind {
        InputPartKind::Text => norte_code_wire::OutputModality::Text,
        InputPartKind::Image => norte_code_wire::OutputModality::Image,
        InputPartKind::Document => norte_code_wire::OutputModality::Document,
        InputPartKind::Audio => norte_code_wire::OutputModality::Audio,
        InputPartKind::Video => norte_code_wire::OutputModality::Video,
    }
}

/// The word a person reads for a modality.
fn modality_word(modality: norte_code_wire::OutputModality) -> &'static str {
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
