//! The status row as a widget.

use crate::style::Style;
use crate::ui::clip;
use crate::ui::Widget;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Widget as _;

#[allow(dead_code)]
pub(crate) struct StatusRow {
    pub(crate) line: Line<'static>,
    pub(crate) style: Style,
}

impl Widget for StatusRow {
    fn measure(&self, _width: u16, _theme: &crate::style::Theme) -> u16 {
        1
    }

    fn draw(
        &self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        _theme: &crate::style::Theme,
    ) {
        if area.height == 0 {
            return;
        }
        let room = usize::from(area.width).saturating_sub(2).max(1);
        let line = clip::clip_line(self.line.clone(), room);
        let mut line = line;
        line.spans.insert(0, ratatui::text::Span::raw("  "));
        ratatui::widgets::Paragraph::new(line).render(area, frame.buffer_mut());
    }
}
