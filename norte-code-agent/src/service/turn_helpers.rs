//! Small conversions the turn loop shares.

use norte::Role;
use norte::domain::message::Message;
use norte::domain::part::Part;

use norte_code_protocol::costs::BudgetNotice;
use norte_code_wire::{MessageRole, ServerEventKind};

/// The text of a message for the replay: each text, reasoning, tool call and
/// tool result part on its own line, so a tool turn still reads.
pub(super) fn replay_text(message: &Message) -> Vec<(norte_code_wire::ReplayedKind, String)> {
    use norte_code_wire::ReplayedKind;
    let mut out: Vec<(ReplayedKind, String)> = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text(text) if !text.text.trim().is_empty() => {
                out.push((ReplayedKind::Text, text.text.clone()));
            }
            Part::Reasoning(reasoning) if !reasoning.text.trim().is_empty() => {
                out.push((ReplayedKind::Reasoning, reasoning.text.clone()));
            }
            Part::Refusal(refusal) if !refusal.text.trim().is_empty() => {
                out.push((ReplayedKind::Refusal, refusal.text.clone()));
            }
            Part::ToolCall(call) => {
                let arguments = serde_json::to_string_pretty(&call.arguments)
                    .unwrap_or_else(|_| call.arguments.to_string());
                out.push((
                    ReplayedKind::ToolCall,
                    format!("{} {}", call.name, arguments),
                ));
            }
            Part::ToolResult(result) => {
                let text = result
                    .parts
                    .iter()
                    .filter_map(Part::as_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = if text.is_empty() {
                    result
                        .structured
                        .as_ref()
                        .and_then(|value| serde_json::to_string_pretty(value).ok())
                        .unwrap_or_else(|| "(tool result)".to_owned())
                } else {
                    text
                };
                out.push((ReplayedKind::ToolResult, text));
            }
            _ => {}
        }
    }
    out
}

/// The event a budget notice becomes.
pub(super) fn budget_notice_kind(notice: BudgetNotice) -> ServerEventKind {
    match notice {
        BudgetNotice::Warning(warning) => ServerEventKind::BudgetWarning {
            scope: warning.scope,
            unit: warning.unit,
            spent: warning.spent,
            limit: warning.limit,
        },
        BudgetNotice::Exceeded(exceeded) => ServerEventKind::BudgetExceeded {
            scope: exceeded.scope,
            unit: exceeded.unit,
            spent: exceeded.spent,
            limit: exceeded.limit,
            action: exceeded.action,
        },
    }
}

/// Whether a tool's own result says it was let go of.
///
/// The shell renders `code: detached` into the structured value it returns, so
/// a host holding both the structured result and the event can tell a call that
/// stopped waiting from one that failed.
pub(super) fn is_detached(output: &norte::ToolOutput) -> bool {
    output
        .structured
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|code| code == "detached")
}

/// The wire name of a message's role.
pub(super) fn role_name(role: Role) -> MessageRole {
    match role {
        Role::System => MessageRole::System,
        Role::Developer => MessageRole::Developer,
        Role::User => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
        Role::Tool => MessageRole::Tool,
        _ => MessageRole::Unknown,
    }
}
