//! Approval labels and the argument readers of the small tools.

use crate::input::{TodoItem, TodoStatus};

use super::ApprovalMode;

/// Reads the `todo` tool's arguments into the footer's list.
///
/// The model sends the complete list on every call, so this is a snapshot,
/// not a patch. An argument shape the tool would reject is ignored rather
/// than guessed at.
pub(crate) fn todo_items(arguments: Option<&serde_json::Value>) -> Option<Vec<TodoItem>> {
    let todos = arguments?.get("todos")?.as_array()?;
    let mut items = Vec::with_capacity(todos.len());
    for todo in todos {
        let content = todo.get("content")?.as_str()?;
        let status = match todo.get("status")?.as_str()? {
            "pending" => TodoStatus::Pending,
            "in_progress" => TodoStatus::InProgress,
            "completed" => TodoStatus::Completed,
            _ => return None,
        };
        items.push(TodoItem {
            content: content.to_owned(),
            status,
        });
    }
    Some(items)
}

pub(crate) fn approval_label(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "ask",
        ApprovalMode::AutoEdit => "auto-edit",
        ApprovalMode::Yolo => "yolo",
    }
}
