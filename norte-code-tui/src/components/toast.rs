//! Notices drawn as floating toasts.
//!
//! They come out of the top left corner over everything else, so the footer
//! can stay a status line. A toast has no background of its own beyond its
//! band: the icon and the text carry the severity.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::components::{palette, spacing};
use crate::ui_model::{Notice, Severity, ThemeMode};

/// The widest a toast grows before it wraps.
const MAX_WIDTH: u16 = 64;

/// Draws the notice as a toast at the top left, under the tab bar.
pub(crate) fn draw(frame: &mut Frame<'_>, area: Rect, notice: &Notice, theme: ThemeMode) {
    if area.height < 3 || area.width < 12 {
        return;
    }
    let width = area
        .width
        .saturating_sub(spacing::SCREEN_GUTTER * 2)
        .min(MAX_WIDTH);
    let text_width = usize::from(width).saturating_sub(4);
    let lines = wrap(notice.text(), text_width);
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height);
    let rect = Rect {
        x: area.right().saturating_sub(width + spacing::SCREEN_GUTTER),
        y: area.y,
        width,
        height,
    };

    let (icon, color) = glyph(notice.severity(), theme);
    let background = palette::surface_alt(theme);
    let mut body: Vec<Line<'static>> = Vec::with_capacity(lines.len());
    for (index, text) in lines.into_iter().enumerate() {
        let mut spans = Vec::new();
        if index == 0 {
            spans.push(Span::styled(
                format!("{icon} "),
                Style::default()
                    .fg(color)
                    .bg(background)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            text,
            Style::default().fg(palette::text(theme)).bg(background),
        ));
        body.push(Line::from(spans));
    }

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(ratatui::text::Text::from(body))
            .style(Style::default().bg(background))
            .block(
                ratatui::widgets::Block::default()
                    .borders(ratatui::widgets::Borders::LEFT | ratatui::widgets::Borders::RIGHT)
                    .border_style(Style::default().fg(color))
                    .style(Style::default().bg(background))
                    .padding(spacing::panel()),
            ),
        rect,
    );
}

/// The icon and color a severity is drawn with.
fn glyph(severity: Severity, theme: ThemeMode) -> (&'static str, ratatui::style::Color) {
    match severity {
        Severity::Info => ("i", palette::info()),
        Severity::Warning => ("!", palette::warning()),
        Severity::Error => ("x", palette::danger()),
        Severity::Sticky => ("*", palette::accent(theme)),
    }
}

/// Breaks the text at the width, keeping whole words where it can.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let projected = if current.is_empty() {
            word.len()
        } else {
            current.len() + 1 + word.len()
        };
        if projected > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
