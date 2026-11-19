//! Dims everything under a modal.
//!
//! The scrim is the whole point of a modal being modal: the screen behind it
//! keeps its layout but loses its contrast, so the eye goes to the box.

use ratatui::Frame;
use ratatui::style::Style;

use crate::{components::palette, ui_model::ThemeMode};

/// Flattens every cell of the screen to the scrim colors.
///
/// It must run before the modal is drawn, or it would erase it too.
pub(crate) fn paint_scrim(frame: &mut Frame<'_>, theme: ThemeMode) {
    let style = Style::default()
        .fg(palette::scrim_foreground(theme))
        .bg(palette::scrim_background(theme));
    let area = frame.area();
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_style(style);
            }
        }
    }
}
