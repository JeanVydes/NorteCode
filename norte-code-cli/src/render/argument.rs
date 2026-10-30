//! The argument readers of the small tools.

use crate::input::{TodoItem, TodoStatus};

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

/// The wire's copy of one task, as the footer's widget holds it.
///
/// The service sends the list it read back from the session's own `todo` call,
/// so this is a conversion and not a parse: the shape is already checked, and a
/// value the tool would reject never reaches the wire.
pub(crate) fn todo_item(info: &norte_code_protocol::TodoInfo) -> TodoItem {
    use norte_code_protocol::TodoStatus as Wire;
    let status = match info.status {
        Wire::Pending => TodoStatus::Pending,
        Wire::InProgress => TodoStatus::InProgress,
        Wire::Completed => TodoStatus::Completed,
    };
    TodoItem {
        content: info.content.clone(),
        status,
    }
}
