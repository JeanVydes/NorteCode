//! The conversation as a widget.

use crate::input::Pinned;
use crate::app::State;
use crate::style::Style;
use crate::ui::Widget;
use ratatui::layout::Rect;
use std::sync::Arc;

pub(crate) struct TranscriptView<'a> {
    pub(crate) state: &'a State,
    pub(crate) pinned: &'a Arc<Pinned>,
    pub(crate) style: Style,
}

impl Widget for TranscriptView<'_> {
    fn measure(&self, width: u16, _theme: &crate::style::Theme) -> u16 {
        // The transcript takes whatever the chrome does not; its own height is
        // the conversation budget, computed by the layout. Report zero here so
        // the band is a fill handled by `split`, not a measured row.
        let _ = width;
        0
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
        // Delegate to the existing painter so behaviour is unchanged; this
        // widget is the seam later phases replace.
        crate::app::draw_transcript(frame, area, self.state, self.pinned, self.style);
    }
}
