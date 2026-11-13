//! The panel that lets the reader pick one of a list.
//!
//! The list is drawn as it is, with a number in front of every entry, and
//! the reader answers with that number.

use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Block, Padding, Paragraph},
};

use crate::{components::palette, ui_model::ThemeMode};

/// The panel that lists the choices.
pub(super) struct PickerPanel;

impl PickerPanel {
    /// Draws the panel, when there is something to pick.
    pub(super) fn render(
        frame: &mut Frame<'_>,
        area: Rect,
        title: &str,
        lines: Vec<Line<'static>>,
        theme: ThemeMode,
    ) {
        if lines.is_empty() {
            return;
        }
        frame.render_widget(
            Paragraph::new(ratatui::text::Text::from(lines)).block(
                Block::default()
                    .title(title.to_owned())
                    .padding(Padding::horizontal(1))
                    .style(palette::panel_style(theme)),
            ),
            area,
        );
    }

    /// The entry the reader picked, counted from zero.
    pub(super) fn pick(count: usize, choice: usize) -> Option<usize> {
        let index = choice.checked_sub(1)?;
        (index < count).then_some(index)
    }
}
