//! The animated spinner as a widget.

use crate::render::Wait;
use crate::style::{Style, metrics};
use crate::ui::Widget;
use ratatui::layout::Rect;
use ratatui::style::Color;
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
        let since = self
            .waiting_since
            .map_or(0, |started| now.saturating_sub(started));
        let since_secs = u64::try_from(since / 1_000).unwrap_or(u64::MAX);

        // Shimmer: a bright spot sweeps left-to-right across the label, leaving
        // a fading trail behind it — like a light passing over the text.
        let label_chars: Vec<char> = label.chars().collect();
        let len = label_chars.len().max(1);
        let cycle = (step as usize) % (len + 6);
        let spot = cycle.cast_signed() - 3; // allow off-screen for fade-in/out

        let base_bg = Color::Rgb(0x1A, 0x1F, 0x24);
        let light_bg = Color::Rgb(0x56, 0xB6, 0xC2);

        let mut spans: Vec<Span> = Vec::with_capacity(len + 2);
        spans.push(Span::raw("  "));
        for (i, ch) in label_chars.iter().enumerate() {
            let dist = isize::try_from(i).unwrap_or(0) - spot; let dist = dist.unsigned_abs();
            let bg = if dist == 0 {
                light_bg
            } else if dist == 1 {
                Color::Rgb(0x3A, 0x7A, 0x82)
            } else if dist == 2 {
                Color::Rgb(0x2A, 0x4A, 0x52)
            } else {
                base_bg
            };
            spans.push(Span::styled(
                ch.to_string(),
                self.style.dim_span().bg(bg),
            ));
        }
        spans.push(Span::styled(
            format!(" · {since_secs}s"),
            self.style.faint_span(),
        ));
        let line = Line::from(spans);
        Paragraph::new(line).render(area, frame.buffer_mut());
    }
}
