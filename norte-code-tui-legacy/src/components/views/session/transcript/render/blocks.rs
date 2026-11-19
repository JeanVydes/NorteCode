//! The blocks a turn holds: messages, reasoning and chips.

use norte_code_wire::MessageRole;
use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::TranscriptOptions;
use crate::components::markdown;
use crate::{components::palette, text_builder::TextBuilder, ui_model::ThemeMode};

/// The bar that opens a user message.
const USER_BAR: &str = "▌ ";

/// The lines a message is drawn with.
///
/// The marker and the stamp are prepended to whole lines, so the text has to
/// wrap inside what they leave: wrapping first and prefixing afterwards is what
/// pushed every line two to eight columns past the panel.
pub(crate) fn message_lines(
    role: MessageRole,
    text: &str,
    width: u16,
    stamp: Option<&str>,
    theme: ThemeMode,
) -> Vec<Line<'static>> {
    let is_user = role == MessageRole::User;
    // The room the marker takes on every line, and the stamp on the first.
    let marker_width = 2;
    let stamp_width = stamp.map_or(0, |stamp| stamp.chars().count() + 2);
    let text_width = usize::from(width)
        .saturating_sub(marker_width)
        .saturating_sub(stamp_width);
    let mut lines = markdown::render(text, u16::try_from(text_width).unwrap_or(0).max(1));
    if lines.is_empty() {
        lines.push(Line::default());
    }
    let background = palette::background(theme);
    for (index, line) in lines.iter_mut().enumerate() {
        let lead = marker_text(is_user, index, stamp);
        if is_user {
            let mut spans = vec![Span::styled(
                USER_BAR.to_owned(),
                Style::default().fg(palette::accent(theme)),
            )];
            spans.push(match (index, stamp) {
                (0, Some(stamp)) => Span::styled(
                    format!("{stamp} "),
                    Style::default().fg(palette::text_muted(theme)),
                ),
                _ => Span::raw(" ".repeat(lead.saturating_sub(2))),
            });
            spans.append(&mut line.spans);
            for span in &mut spans {
                span.style = span.style.bg(background);
            }
            *line = Line::from(spans).style(Style::default().bg(background));
        } else {
            if index == 0
                && let Some(stamp) = stamp
            {
                line.spans.insert(
                    0,
                    Span::styled(
                        format!("{stamp}  "),
                        Style::default().fg(palette::text_muted(theme)),
                    ),
                );
            }
            line.spans.insert(0, Span::raw(" ".repeat(marker_width)));
        }
    }
    lines
}

/// How many columns the marker occupies on one line of a message.
fn marker_text(is_user: bool, index: usize, stamp: Option<&str>) -> usize {
    let _ = is_user;
    match (index, stamp) {
        (0, Some(stamp)) => 2 + stamp.chars().count() + 1,
        _ => 2,
    }
}

/// The lines the reasoning is drawn with. `phase` animates the block while
/// the reasoning streams; `None` leaves it frozen on the neutral background.
/// `expanded` says whether this block shows its text.
pub(crate) fn reasoning_lines(
    text: &str,
    options: TranscriptOptions<'_>,
    phase: Option<u16>,
    expanded: bool,
) -> Vec<Line<'static>> {
    let TranscriptOptions { width, theme, .. } = options;
    let mut body = Vec::new();
    if expanded {
        body = markdown::render(text, width.saturating_sub(4));
        for line in &mut body {
            line.spans.insert(0, Span::raw("  "));
        }
    }
    let mut lines = Vec::new();
    let mut summary = TextBuilder::thinking_summary(expanded, theme);
    // The beam travels across the text it lights, not across the panel: on a
    // short block a sweep measured against the panel would spend most of its
    // time past the last word, leaving the block dark.
    let sweep = body
        .iter()
        .map(display_width)
        .max()
        .unwrap_or_else(|| display_width(&summary))
        .max(1);
    let sweep = u16::try_from(sweep).unwrap_or(width).min(width.max(1));
    palette::style_thinking_line(&mut summary, phase, sweep, theme);
    lines.push(summary);
    for mut line in body {
        palette::style_thinking_line(&mut line, phase, sweep, theme);
        lines.push(line);
    }
    lines
}

/// The display width of a line.
fn display_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

/// The lines the chips of a message are drawn with.
pub(crate) fn chip_lines(chips: &[crate::chips::Chip], theme: ThemeMode) -> Vec<Line<'static>> {
    chips
        .iter()
        .map(|chip| crate::chips::chip_line(chip, theme))
        .collect()
}
