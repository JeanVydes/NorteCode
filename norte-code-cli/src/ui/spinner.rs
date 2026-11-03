//! The animated spinner as a widget.

use crate::render::Wait;
use crate::style::{Style, icon, metrics};
use crate::ui::Widget;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};
use std::time::{SystemTime, UNIX_EPOCH};

const THINKING_WORDS: &[&str] = &[
    "Pondering…", "Musing…", "Ruminating…", "Cogitating…",
    "Deliberating…", "Contemplating…", "Reflecting…", "Weighing…",
];
const WAITING_WORDS: &[&str] = &[
    "Connecting…", "Handshaking…", "Negotiating…", "Establishing…",
    "Reaching out…", "Dialing in…", "Waking up…", "Knocking…",
];

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
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let step = now / u128::from(metrics::SPINNER_MS);
        let label = match waiting {
            Wait::Streaming => "streaming",
            Wait::Thinking => {
                let idx = usize::try_from(step / 30).unwrap_or(0) % THINKING_WORDS.len();
                THINKING_WORDS[idx]
            }
            Wait::Waiting => {
                let idx = usize::try_from(step / 30).unwrap_or(0) % WAITING_WORDS.len();
                WAITING_WORDS[idx]
            }
        };
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
