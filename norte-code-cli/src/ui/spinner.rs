//! The animated spinner as a widget.

use crate::render::Wait;
use crate::style::{Style, icon, metrics};
use crate::ui::Widget;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) struct Spinner {
    pub(crate) waiting: Option<Wait>,
    pub(crate) waiting_since: Option<u128>,
    pub(crate) style: Style,
}

impl Widget for Spinner {
    fn measure(&self, _width: u16, _theme: &crate::style::Theme) -> u16 {
        u16::from(self.waiting.is_some())
    }

    fn draw(&self, frame: &mut ratatui::Frame<'_>, area: Rect, _theme: &crate::style::Theme) {
        if area.height == 0 {
            return;
        }
        let Some(waiting) = self.waiting else { return };
        let label = match waiting {
            Wait::Streaming => "streaming",
            Wait::Thinking => "thinking",
            Wait::Waiting => "waiting for the first token",
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let step = now / u128::from(metrics::SPINNER_MS);
        let glyph = icon::SPINNER[(step % icon::SPINNER.len() as u128) as usize];
        let since = self
            .waiting_since
            .map_or(0, |started| now.saturating_sub(started));
        let since_secs = u64::try_from(since / 1_000).unwrap_or(u64::MAX);
        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled(glyph.to_owned(), self.style.accent_span()),
            Span::raw(" "),
            Span::styled(label.to_owned(), self.style.dim_span()),
            Span::styled(format!(" · {since_secs}s"), self.style.faint_span()),
        ]);
        Paragraph::new(line).render(area, frame.buffer_mut());
    }
}
