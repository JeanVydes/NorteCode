//! The numbered, scrollable Markdown document inside the plan review.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use terminalui::Theme;
use unicode_width::UnicodeWidthStr;

use crate::ui::markdown::Markdown;

pub(super) struct DocumentLayout {
    pub(super) width: u16,
    pub(super) top: u16,
    pub(super) height: u16,
    pub(super) footer: u16,
}

pub(super) struct DocumentState<'a> {
    pub(super) doc: &'a mut Markdown,
    pub(super) width: &'a mut u16,
    pub(super) theme: &'a mut Option<Theme>,
    pub(super) scroll: &'a mut u16,
    pub(super) body: &'a mut Rect,
}

pub(super) fn layout(
    inner: Rect,
    content: &str,
    theme: &Theme,
    state: &mut DocumentState<'_>,
) -> DocumentLayout {
    let width = inner.width.saturating_sub(9);
    if *state.width != width || *state.theme != Some(*theme) {
        *state.width = width;
        *state.theme = Some(*theme);
        *state.doc = Markdown::render(content, width, theme);
    }
    let footer = inner.y + inner.height.saturating_sub(3);
    let top = inner.y + 2;
    let height = footer.saturating_sub(top);
    *state.body = Rect {
        y: top,
        height,
        ..inner
    };
    let max_scroll = state.doc.lines.len().saturating_sub(usize::from(height));
    *state.scroll = (*state.scroll).min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
    DocumentLayout {
        width,
        top,
        height,
        footer,
    }
}

pub(super) fn paint(
    buf: &mut Buffer,
    inner: Rect,
    theme: &Theme,
    doc: &Markdown,
    layout: &DocumentLayout,
    scroll: u16,
) {
    let lines = doc.lines.len();
    let max_scroll = lines.saturating_sub(usize::from(layout.height));
    for row in 0..layout.height {
        let index = usize::from(row.saturating_add(scroll));
        let y = layout.top + row;
        let marker = if max_scroll > 0 && index * max_scroll / lines.max(1) == usize::from(scroll) {
            "█"
        } else {
            "│"
        };
        buf.set_stringn(
            inner.x + inner.width.saturating_sub(1),
            y,
            marker,
            1,
            Style::default().fg(theme.border),
        );
        let Some(line) = doc.lines.get(index) else {
            continue;
        };
        paint_gutter(buf, inner, theme, y, index);
        paint_line(buf, inner.x + 7, y, layout.width, line);
    }
}

fn paint_gutter(buf: &mut Buffer, inner: Rect, theme: &Theme, y: u16, index: usize) {
    buf.set_stringn(
        inner.x + 1,
        y,
        format!("{:>4}  ", index + 1),
        6,
        Style::default().fg(theme.muted).bg(theme.secondary),
    );
}

fn paint_line(buf: &mut Buffer, x: u16, y: u16, width: u16, line: &Line<'_>) {
    let mut column = 0u16;
    for span in &line.spans {
        buf.set_stringn(
            x + column,
            y,
            span.content.as_ref(),
            usize::from(width.saturating_sub(column)),
            span.style,
        );
        column = column.saturating_add(u16::try_from(span.content.width()).unwrap_or(u16::MAX));
    }
}
