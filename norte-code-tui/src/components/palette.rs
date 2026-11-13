use ratatui::style::{Color, Style};
use unicode_width::UnicodeWidthChar;

use crate::ui_model::ThemeMode;

const DANGER: Color = Color::Rgb(239, 106, 126);
const SUCCESS: Color = Color::Rgb(132, 211, 170);
const WARNING: Color = Color::Rgb(245, 190, 117);
const INFO: Color = Color::Rgb(132, 184, 245);
const MUTED: Color = Color::Rgb(145, 151, 166);

pub(crate) const fn brand() -> Color {
    Color::Rgb(255, 145, 205)
}

pub(crate) const fn accent(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => brand(),
        ThemeMode::Light => Color::Rgb(181, 43, 125),
    }
}

pub(crate) const fn danger() -> Color {
    DANGER
}

pub(crate) const fn success() -> Color {
    SUCCESS
}

pub(crate) const fn warning() -> Color {
    WARNING
}

pub(crate) const fn info() -> Color {
    INFO
}

pub(crate) const fn muted() -> Color {
    MUTED
}

pub(crate) fn panel_style(theme: ThemeMode) -> Style {
    Style::default()
        .fg(foreground(theme))
        .bg(panel_background(theme))
}

pub(crate) fn transcript_style(theme: ThemeMode) -> Style {
    Style::default()
        .fg(foreground(theme))
        .bg(transcript_background(theme))
}

pub(crate) fn modal_style(theme: ThemeMode) -> Style {
    Style::default()
        .fg(foreground(theme))
        .bg(modal_background(theme))
}

pub(crate) fn selected_style(theme: ThemeMode) -> Style {
    Style::default()
        .fg(foreground(theme))
        .bg(selected_background(theme))
}

pub(crate) const fn user_message_background(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(63, 39, 57),
        ThemeMode::Light => Color::Rgb(252, 231, 241),
    }
}

pub(crate) const fn assistant_message_background(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(35, 39, 47),
        ThemeMode::Light => Color::Rgb(237, 240, 246),
    }
}

pub(crate) const fn thinking_foreground() -> Color {
    Color::Rgb(128, 130, 140)
}

pub(crate) fn thinking_background(phase: u16, column: u16, width: u16, theme: ThemeMode) -> Color {
    let head = if width == 0 { 0 } else { phase % width };
    let distance = column.abs_diff(head);
    match (theme, distance) {
        (ThemeMode::Dark, 0..=2) => Color::Rgb(105, 47, 83),
        (ThemeMode::Dark, 3..=7) => Color::Rgb(63, 39, 55),
        (ThemeMode::Dark, _) => Color::Rgb(35, 33, 40),
        (ThemeMode::Light, 0..=2) => Color::Rgb(239, 177, 211),
        (ThemeMode::Light, 3..=7) => Color::Rgb(248, 220, 236),
        (ThemeMode::Light, _) => Color::Rgb(244, 241, 245),
    }
}

pub(crate) fn style_thinking_line(
    line: &mut ratatui::text::Line<'static>,
    phase: u16,
    width: u16,
    theme: ThemeMode,
) {
    let spans = std::mem::take(&mut line.spans);
    line.style = Style::default()
        .fg(thinking_foreground())
        .bg(thinking_background(phase, 0, width, theme));
    let mut column = 0_u16;
    for span in spans {
        for character in span.content.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            line.spans.push(ratatui::text::Span::styled(
                character.to_string(),
                span.style
                    .fg(thinking_foreground())
                    .bg(thinking_background(phase, column, width, theme)),
            ));
            column = column.saturating_add(u16::try_from(character_width).unwrap_or(u16::MAX));
        }
    }
}

pub(crate) const fn foreground(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(242, 242, 247),
        ThemeMode::Light => Color::Black,
    }
}

const fn panel_background(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(30, 33, 40),
        ThemeMode::Light => Color::Rgb(236, 239, 244),
    }
}

const fn transcript_background(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(24, 27, 33),
        ThemeMode::Light => Color::Rgb(248, 249, 251),
    }
}

const fn modal_background(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(35, 38, 46),
        ThemeMode::Light => Color::Rgb(255, 255, 255),
    }
}

const fn selected_background(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(55, 59, 72),
        ThemeMode::Light => Color::Rgb(198, 215, 240),
    }
}
