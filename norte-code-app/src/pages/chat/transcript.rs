//! One part of the conversation page.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

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
pub(super) fn is_exploration(item: &ConversationItem) -> bool {
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

pub(super) fn group_tool_rows(items: Vec<ConversationItem>) -> Vec<TranscriptRow> {
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

#[component]
pub(super) fn ExploringGroup(rows: Vec<ConversationItem>) -> Element {
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

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::needless_pass_by_value
)]
pub(super) fn transcript(
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
    // A turn parked on a question is open and cancellable even though the
    // page is not claiming to work.
    let stoppable = (chat_state.has_running_turn() || chat_state == ChatState::AwaitingAnswer)
        && (context.active_turn_id.read().is_some() || context.streaming_turn_id.read().is_some());
    let awaiting_answer = !questions.is_empty();
    // The transcript is its own scroll container: the composer and the
    // header stay put, and the log follows the answer as it prints.
    let mut stick_to_bottom = use_signal(|| true);
    // What changes as the transcript grows: the number of rows, and the total
    // number of characters. Watching only the last row misses the common case
    //, a tool row at the bottom while the answer above it keeps printing.
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
                    span { "Editing a message, sending will replace that turn." }
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

#[component]
pub(super) fn QuestionCard(
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
