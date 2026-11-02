//! One bounded-row helper, shared by every widget.
//!
//! The hint row, the status row and the completion menu used to each carry
//! their own copy of "cut at the width, end with …". A row longer than the
//! terminal now loses its tail in exactly one place, so the cut and its glyph
//! cannot drift between widgets.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};

use crate::style::icon;
use crate::terminal::Draw;

/// Cuts plain text to `room` columns, ending it with `…` when it went over.
#[must_use]
pub(crate) fn clip_text(text: &str, room: usize) -> String {
    let text = crate::select::strip_style(text);
    let text = text.replace(['\n', '\r'], " ");
    let text = crate::text::terminal(&text);
    let text = text.into_owned();
    if text.chars().count() <= room {
        return text;
    }
    let mut cut: String = text.chars().take(room.saturating_sub(1)).collect();
    cut.push(icon::MORE.chars().next().unwrap_or('…'));
    cut
}

/// Cuts a styled line to `room` columns, keeping each span's ink.
#[must_use]
pub(crate) fn clip_line(line: Line<'static>, room: usize) -> Line<'static> {
    let total: usize = line
        .spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum();
    if total <= room {
        return line;
    }
    let mut left = room.saturating_sub(1);
    let mut spans = Vec::with_capacity(line.spans.len());
    for span in line.spans {
        if left == 0 {
            break;
        }
        let style = span.style;
        let text = span.content.into_owned();
        let count = text.chars().count();
        if count <= left {
            left -= count;
            spans.push(Span::styled(text, style));
        } else {
            let cut: String = text.chars().take(left).collect();
            spans.push(Span::styled(cut, style));
            left = 0;
        }
    }
    spans.push(Span::raw(icon::MORE));
    Line::from(spans)
}

/// Paints one bounded text row, with the two-column gutter the chrome uses.
pub(crate) fn paint_text(
    frame: &mut Draw<'_>,
    area: Rect,
    text: &str,
    style: ratatui::style::Style,
) {
    if area.height == 0 || text.is_empty() {
        return;
    }
    let room = usize::from(area.width).saturating_sub(2).max(1);
    let shown = clip_text(text, room);
    let line = Line::from(vec![Span::raw("  "), Span::styled(shown, style)]);
    Paragraph::new(line).render(area, frame.buffer_mut());
}
