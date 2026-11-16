//! The blocks a turn holds: messages, reasoning and chips.

use norte_code_wire::MessageRole;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

use super::super::markdown;
use super::TranscriptOptions;
use crate::{
    components::palette,
    text_builder::TextBuilder,
    ui_model::{ThemeMode, ThinkingVisibility},
};

/// The lines a message is drawn with.
pub(crate) fn message_lines(
    role: MessageRole,
    text: &str,
    width: u16,
    timestamp: Option<i64>,
    theme: ThemeMode,
) -> Vec<Line<'static>> {
    let mut lines = markdown::render(text, width.saturating_sub(4));
    if lines.is_empty() {
        lines.push(Line::default());
    }
    for (index, line) in lines.iter_mut().enumerate() {
        if index == 0 {
            if let Some(timestamp) = timestamp {
                line.spans.insert(
                    0,
                    Span::styled(
                        format!("[{}s] ", timestamp.div_euclid(1_000)),
                        Style::default().fg(palette::muted()),
                    ),
                );
            }
        } else if role != MessageRole::User {
            line.spans.insert(0, Span::raw("  "));
        }
        if role == MessageRole::User {
            fill_user_line(line, width, theme);
        }
    }
    lines
}

/// The lines the reasoning is drawn with. `phase` animates the block while
/// the reasoning streams; `None` leaves it frozen on the neutral background.
pub(crate) fn reasoning_lines(
    text: &str,
    options: TranscriptOptions<'_>,
    phase: Option<u16>,
) -> Vec<Line<'static>> {
    let TranscriptOptions {
        width,
        theme,
        thinking_visibility,
        ..
    } = options;
    let mut lines = Vec::new();
    let mut summary = TextBuilder::thinking_summary();
    palette::style_thinking_line(&mut summary, phase, width, theme);
    lines.push(summary);
    if thinking_visibility == ThinkingVisibility::Expanded {
        for mut line in markdown::render(text, width.saturating_sub(4)) {
            line.spans.insert(0, Span::raw("  "));
            palette::style_thinking_line(&mut line, phase, width, theme);
            lines.push(line);
        }
    }
    lines
}

/// Pins a user message row to both edges: the block's gray carries from the
/// first column to the last, like the shell block does, and the brand text
/// hugs the right edge.
fn fill_user_line(line: &mut Line<'static>, width: u16, theme: ThemeMode) {
    let background = palette::panel_background(theme);
    let used: usize = line
        .spans
        .iter()
        .map(|span| {
            span.content
                .chars()
                .map(|character| UnicodeWidthChar::width(character).unwrap_or(0))
                .sum::<usize>()
        })
        .sum();
    // The timestamp span keeps its own color; the rest goes brand on the
    // block's gray.
    for span in &mut line.spans {
        if span.style.fg == Some(palette::muted()) {
            span.style = span.style.bg(background);
        } else {
            span.style = span.style.fg(palette::brand()).bg(background);
        }
    }
    let fill = usize::from(width).saturating_sub(used);
    line.spans.insert(
        0,
        Span::styled(" ".repeat(fill), Style::default().bg(background)),
    );
    line.style = Style::default().bg(background);
}

/// The lines the chips of a message are drawn with.
pub(crate) fn chip_lines(chips: &[crate::chips::Chip]) -> Vec<Line<'static>> {
    chips
        .iter()
        .map(|chip| {
            Line::from(vec![
                Span::styled(
                    format!("{} ", crate::chips::chip_icon(chip.kind())),
                    Style::default().fg(palette::muted()),
                ),
                Span::raw(chip.label.clone()),
                Span::styled(
                    format!(" · {}", chip.detail),
                    Style::default().fg(palette::muted()),
                ),
            ])
        })
        .collect()
}
