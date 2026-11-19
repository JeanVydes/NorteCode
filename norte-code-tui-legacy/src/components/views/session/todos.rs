//! The panel that shows what the model is working on.
//!
//! The host sends the session's task list with the resume prefix, so the
//! panel comes back with the session instead of starting empty.

use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph},
};

use norte_code_wire::{TodoInfo, TodoStatus};

use crate::components::palette;
use crate::ui_model::ThemeMode;

/// The most tasks the panel shows.
const MAX_TASKS: usize = 6;

#[cfg(test)]
#[path = "todos_tests.rs"]
mod tests;

/// The panel that shows the session's tasks.
pub(super) struct TodosPanel;

impl TodosPanel {
    /// The title of the panel: how many tasks are done.
    pub(super) fn title(todos: &[TodoInfo]) -> String {
        let done = todos
            .iter()
            .filter(|todo| todo.status == TodoStatus::Completed)
            .count();
        format!("tasks {done}/{}", todos.len())
    }

    /// The lines the panel draws.
    pub(super) fn lines(todos: &[TodoInfo]) -> Vec<Line<'static>> {
        todos
            .iter()
            .take(MAX_TASKS)
            .map(|todo| {
                let mark = match todo.status {
                    TodoStatus::Pending => "○",
                    TodoStatus::InProgress => "◐",
                    TodoStatus::Completed => "●",
                };
                Line::from(vec![
                    Span::raw(format!("{mark} ")),
                    Span::raw(todo.content.clone()),
                ])
            })
            .collect()
    }

    /// How many rows the panel needs, title included.
    pub(super) fn height(todos: &[TodoInfo]) -> u16 {
        if todos.is_empty() {
            0
        } else {
            u16::try_from(todos.len().min(MAX_TASKS) + 1).unwrap_or(u16::MAX)
        }
    }

    /// Draws the panel, when there is something to draw.
    pub(super) fn render(frame: &mut Frame<'_>, area: Rect, todos: &[TodoInfo], theme: ThemeMode) {
        let lines = Self::lines(todos);
        if lines.is_empty() {
            return;
        }
        frame.render_widget(
            Paragraph::new(ratatui::text::Text::from(lines)).block(
                Block::default()
                    .title(Self::title(todos))
                    .padding(Padding::horizontal(1))
                    .style(palette::panel_style(theme)),
            ),
            area,
        );
    }
}
