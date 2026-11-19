use norte_code_wire::{TodoInfo, TodoStatus};

use super::TodosPanel;

fn todo(content: &str, status: TodoStatus) -> TodoInfo {
    TodoInfo {
        content: content.to_owned(),
        status,
    }
}

#[test]
fn the_panel_counts_the_tasks_that_are_done() {
    let todos = vec![
        todo("read the file", TodoStatus::Completed),
        todo("write the fix", TodoStatus::InProgress),
        todo("run the tests", TodoStatus::Pending),
    ];

    assert_eq!(TodosPanel::title(&todos), "tasks 1/3");
}

#[test]
fn the_panel_marks_every_task_with_its_state() {
    let todos = vec![
        todo("read the file", TodoStatus::Completed),
        todo("write the fix", TodoStatus::InProgress),
        todo("run the tests", TodoStatus::Pending),
    ];

    let lines = TodosPanel::lines(&todos);

    assert_eq!(lines.len(), 3);
    assert!(lines[0].spans[0].content.as_ref().contains('●'));
    assert!(lines[1].spans[0].content.as_ref().contains('◐'));
    assert!(lines[2].spans[0].content.as_ref().contains('○'));
}

#[test]
fn the_panel_keeps_room_for_the_transcript() {
    assert_eq!(TodosPanel::height(&[]), 0);
    assert_eq!(TodosPanel::height(&[todo("one", TodoStatus::Pending)]), 2);
}
