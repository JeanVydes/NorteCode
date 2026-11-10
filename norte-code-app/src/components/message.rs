use crate::domain::{
    ConversationItem, HistoryKind, MessageDelivery, SessionTodo, TodoStatus, ToolActivity,
    ToolActivityStatus,
};
use dioxus::prelude::*;
use norte_code_wire::JobStatus;

/// One entry of the conversation.
///
/// The assistant answer carries its usage and per-turn actions; the user's
/// message carries its own actions (rewind, edit and re-send); tool calls
/// collapse to one line unless they failed or the reader asks for output.
#[component]
pub fn MessageItem(
    item: ConversationItem,
    show_thinking: bool,
    show_timestamp: bool,
    on_rewind: EventHandler<String>,
    on_replace: EventHandler<ConversationItem>,
    on_fork: EventHandler<Option<String>>,
    on_pin: EventHandler<String>,
    pinned: bool,
) -> Element {
    if matches!(item.kind, HistoryKind::ToolCall | HistoryKind::ToolResult) {
        return rsx! { ToolRow { item } };
    }

    let is_user = item.kind == HistoryKind::User;
    let is_reasoning = item.kind == HistoryKind::Reasoning;
    if is_reasoning && !show_thinking {
        return rsx! {};
    }
    let is_failed = item.delivery == MessageDelivery::Failed;
    let class = if is_user {
        "message message-user"
    } else if is_reasoning {
        "message message-reasoning"
    } else if is_failed {
        "message message-assistant message-failed"
    } else {
        "message message-assistant"
    };
    let turn = item.turn_id.clone();
    let turn_for_actions = turn.clone();
    let timestamp = show_timestamp.then(|| {
        let seconds = item.created_at.div_euclid(1_000);
        format!("{seconds}")
    });
    let item_for_replace = item.clone();

    rsx! {
        article { class,
            if is_reasoning {
                details { class: "reasoning",
                    summary { "Thought for a moment" }
                    div { class: "markdown-body", dangerous_inner_html: crate::components::render_markdown(&item.content) }
                }
            } else if is_user {
                div { class: "message-text", "{item.content}" }
            } else {
                div { class: "markdown-body", dangerous_inner_html: crate::components::render_markdown(&item.content) }
            }
            if is_failed {
                p { class: "message-error", "Not delivered" }
            }
            if let Some(usage) = &item.usage {
                div { class: "message-usage",
                    if let Some(model) = usage.model_label() {
                        span { class: "message-model", "{model}" }
                    }
                    if !usage.performance_label().is_empty() {
                        span { "{usage.performance_label()}" }
                    }
                    span { "{usage.token_label()}" }
                    if let Some(cost) = usage.cost_usd {
                        span { "${cost:.4}" }
                    }
                    if usage.cache_hit {
                        span { "cache hit" }
                    }
                }
            }
            // The actions are a menu behind one button, not a row of links
            // under every message: a transcript where every line offers
            // Rewind and Fork reads as a toolbar, and half of them do not
            // apply to what the person is looking at.
            if turn_for_actions.is_some() || is_user {
                details { class: "message-menu",
                    summary { class: "message-menu-button", title: "Message options", "···" }
                    div { class: "message-menu-items",
                        if let Some(turn) = turn_for_actions.clone() {
                            button {
                                class: "popover-item",
                                title: "Rewind the conversation to just before this message",
                                onclick: {
                                    let turn = turn.clone();
                                    move |_| on_rewind.call(turn.clone())
                                },
                                "Rewind"
                            }
                        }
                        if is_user {
                            button {
                                class: "popover-item",
                                title: "Rewind and send an edited version of this message",
                                onclick: move |_| on_replace.call(item_for_replace.clone()),
                                "Edit & resend"
                            }
                        }
                        if let Some(turn) = turn_for_actions.clone() {
                            button {
                                class: "popover-item",
                                title: "Pin this turn so compaction keeps it verbatim",
                                onclick: {
                                    let turn = turn.clone();
                                    move |_| on_pin.call(turn.clone())
                                },
                                if pinned { "Unpin" } else { "Pin" }
                            }
                            button {
                                class: "popover-item",
                                title: "Branch a new session from here",
                                onclick: {
                                    let turn = turn.clone();
                                    move |_| on_fork.call(Some(turn.clone()))
                                },
                                "Fork from here"
                            }
                        }
                        if let Some(stamp) = &timestamp {
                            span { class: "message-menu-time", "{stamp}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ToolRow(item: ConversationItem) -> Element {
    let activity = item.tool.clone().or_else(|| historical(&item));
    let Some(activity) = activity else {
        return rsx! {};
    };
    let failed = activity.status == ToolActivityStatus::Failed;
    let state = match activity.status {
        ToolActivityStatus::Running => "running",
        ToolActivityStatus::Completed => "done",
        ToolActivityStatus::Failed => "failed",
        ToolActivityStatus::Cancelled => "stopped",
    };
    let title = activity
        .path
        .clone()
        .or_else(|| activity.result_summary.clone())
        .unwrap_or_else(|| activity.name.clone());
    let has_output = !activity.output.trim().is_empty();
    rsx! {
        details { class: if failed { "tool tool-failed" } else { "tool" }, open: failed,
            summary { class: "tool-line",
                span { class: "tool-name", "{activity.name}" }
                span { class: "tool-target", "{title}" }
                if let Some(ms) = activity.duration_ms {
                    span { class: "tool-time", "{ms} ms" }
                }
                span { class: "tool-state tool-state-{state}", "{state}" }
            }
            if has_output {
                pre { class: "tool-output", "{activity.output}" }
            }
            // Arguments are already summarised on the line; the full document
            // is for the reader who opened the row.
            if let Some(arguments) = arguments_text(&activity)
                && activity.path.is_none()
            {
                pre { class: "tool-output tool-arguments", "{arguments}" }
            }
        }
    }
}

/// A tool call replayed from history.
///
/// The replay text is `name {pretty json}`, and the arguments are pretty
/// printed, so the JSON spans lines and starts with a brace rather than the
/// arguments name, the old split on the first space left the raw document as
/// the "path". The name is the first token; everything after it is the
/// argument document.
fn historical(item: &ConversationItem) -> Option<ToolActivity> {
    if item.kind != HistoryKind::ToolCall {
        return None;
    }
    let call = parse_replayed_call(&item.content)?;
    Some(ToolActivity {
        call_id: item.id.clone(),
        name: call.name,
        path: call.target,
        arguments: call.arguments,
        output: String::new(),
        status: ToolActivityStatus::Completed,
        duration_ms: None,
        result_summary: None,
    })
}

/// The tool name, the argument document and the line summary of a replayed
/// call, which the host writes as `name {json}` with the JSON pretty printed.
///
/// The name is everything before the first brace; when there is no brace the
/// call carries no arguments and the whole line is the name. The summary is
/// the most telling scalar in the document, the file, the command, the query -
/// so the row reads `shell · cargo test · 1.2s` instead of a wall of JSON.
#[derive(Debug, PartialEq, Eq)]
struct ReplayedCall {
    name: String,
    target: Option<String>,
    arguments: serde_json::Value,
}

const TARGET_KEYS: [&str; 8] = [
    "path",
    "file_path",
    "file",
    "pattern",
    "query",
    "command",
    "url",
    "target",
];

fn parse_replayed_call(content: &str) -> Option<ReplayedCall> {
    let content = content.trim_start();
    if content.is_empty() {
        return None;
    }
    let (name, rest) = match content.find('{') {
        Some(brace) => (content[..brace].trim(), content[brace..].trim()),
        None => (content, ""),
    };
    if name.is_empty() || name.contains('\n') {
        return None;
    }
    let arguments = if rest.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(rest).unwrap_or(serde_json::Value::Null)
    };
    let target = TARGET_KEYS
        .iter()
        .find_map(|key| arguments.get(*key).and_then(serde_json::Value::as_str))
        .or_else(|| {
            arguments
                .get("args")
                .and_then(serde_json::Value::as_array)
                .and_then(|args| args.last())
                .and_then(serde_json::Value::as_str)
        })
        .map(|value| {
            let line = value.lines().next().unwrap_or_default().trim();
            if line.chars().count() > 80 {
                format!("{}…", line.chars().take(80).collect::<String>())
            } else {
                line.to_owned()
            }
        })
        .filter(|value| !value.is_empty());
    Some(ReplayedCall {
        name: name.to_owned(),
        target,
        arguments,
    })
}

fn arguments_text(activity: &ToolActivity) -> Option<String> {
    if activity.arguments.is_null() {
        return None;
    }
    serde_json::to_string_pretty(&activity.arguments).ok()
}

#[component]
pub fn TodoPanel(todos: Vec<SessionTodo>) -> Element {
    if todos.is_empty() {
        return rsx! {};
    }
    let done = todos
        .iter()
        .filter(|todo| todo.status == TodoStatus::Completed)
        .count();
    let total = todos.len();
    rsx! {
        details { class: "plan", open: done < total,
            summary {
                span { "Plan" }
                span { class: "plan-count", "{done}/{total}" }
            }
            ul { class: "plan-list",
                for (index, todo) in todos.iter().enumerate() {
                    li { key: "{index}", class: match todo.status {
                        TodoStatus::Pending => "plan-item",
                        TodoStatus::InProgress => "plan-item plan-active",
                        TodoStatus::Completed => "plan-item plan-done",
                    },
                        span { class: "plan-mark", match todo.status {
                            TodoStatus::Pending => "○",
                            TodoStatus::InProgress => "◐",
                            TodoStatus::Completed => "●",
                        }}
                        span { "{todo.content}" }
                    }
                }
            }
        }
    }
}

/// One running job, only rendered where the conversation crosses a detached
/// call so the reader can tell why the turn is still open.
#[component]
pub fn JobRow(job: norte_code_wire::JobSummary) -> Element {
    let state = job.status.label();
    rsx! {
        details { class: "tool",
            summary { class: "tool-line",
                span { class: "tool-name", "job" }
                span { class: "tool-target", "{job.command}" }
                span { class: "tool-state", "{state}" }
            }
            if let Some(output) = &job.output_snapshot {
                pre { class: "tool-output", "{output}" }
            }
            if job.status == JobStatus::Running {
                p { class: "helper-text", "Running in the background. Output appears here as it arrives." }
            }
        }
    }
}

#[cfg(test)]
#[path = "message_tests.rs"]
mod tests;
