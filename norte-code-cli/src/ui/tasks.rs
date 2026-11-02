//! The compact task row as a widget.

use crate::app::TaskRow;
use crate::style::{Style, icon};
use crate::ui::clip;
use crate::ui::Widget;
use ratatui::layout::Rect;

/// The task list, drawn as one compact row.
pub(crate) struct Tasks<'a> {
    pub(crate) todos: &'a [TaskRow],
    pub(crate) waiting: bool,
    pub(crate) style: Style,
}

impl Widget for Tasks<'_> {
    fn measure(&self, _width: u16, _theme: &crate::style::Theme) -> u16 {
        u16::from(!self.todos.is_empty())
    }

    fn draw(
        &self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        _theme: &crate::style::Theme,
    ) {
        if area.height == 0 || self.todos.is_empty() {
            return;
        }
        let done = self.todos.iter().filter(|task| task.done).count();
        let mark = if self.waiting {
            icon::RUNNING
        } else if done == self.todos.len() {
            icon::DONE
        } else {
            icon::PENDING
        };
        let text = self
            .todos
            .iter()
            .find(|task| !task.done)
            .map_or_else(
                || format!("{mark} tasks {done}/{}", self.todos.len()),
                |task| format!("{mark} tasks {done}/{} · {}", self.todos.len(), task.text),
            );
        clip::paint_text(frame, area, &text, self.style.dim_span());
    }
}
