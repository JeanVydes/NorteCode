//! The colors the interface is drawn with.
//!
//! The palette is organized by role, not by hue: a caller asks for `surface` or
//! `danger`, never for a literal color. That keeps the hierarchy in one place:
//! `accent` marks what is focused or selected, the semantic colors say what
//! happened, and everything else is a neutral.

use ratatui::style::{Color, Modifier, Style};

use crate::ui_model::ThemeMode;

const ACCENT_DARK: Color = Color::Rgb(122, 162, 247);
const ACCENT_LIGHT: Color = Color::Rgb(45, 96, 200);

const SUCCESS: Color = Color::Rgb(126, 200, 154);
const WARNING: Color = Color::Rgb(230, 180, 105);
const DANGER: Color = Color::Rgb(232, 108, 122);
const INFO: Color = Color::Rgb(124, 176, 232);

/// How full a gauge is, as one of the four bands it paints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GaugeBand {
    Calm,
    Filling,
    Close,
    Full,
}

impl GaugeBand {
    /// The band a fraction belongs to.
    #[must_use]
    pub(crate) fn of(fraction: f64) -> Self {
        if fraction >= 0.90 {
            Self::Full
        } else if fraction >= 0.75 {
            Self::Close
        } else if fraction >= 0.50 {
            Self::Filling
        } else {
            Self::Calm
        }
    }

    /// The color the band paints with.
    #[must_use]
    pub(crate) const fn color(self) -> Color {
        match self {
            Self::Calm => SUCCESS,
            Self::Filling => WARNING,
            Self::Close => Color::Rgb(235, 145, 90),
            Self::Full => DANGER,
        }
    }
}

/// The background of the whole screen.
#[must_use]
pub(crate) const fn background(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(22, 24, 29),
        ThemeMode::Light => Color::Rgb(250, 250, 252),
    }
}

/// The background of a panel or a band.
#[must_use]
pub(crate) const fn surface(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(30, 33, 40),
        ThemeMode::Light => Color::Rgb(238, 240, 245),
    }
}

/// The background of a block that needs to stand out from a panel.
#[must_use]
pub(crate) const fn surface_alt(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(38, 42, 51),
        ThemeMode::Light => Color::Rgb(228, 232, 240),
    }
}

/// The color of a rule or a border.
#[must_use]
pub(crate) const fn border(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(58, 62, 74),
        ThemeMode::Light => Color::Rgb(205, 210, 220),
    }
}

/// The color of ordinary text.
#[must_use]
pub(crate) const fn text(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(224, 227, 235),
        ThemeMode::Light => Color::Rgb(34, 38, 46),
    }
}

/// The color of secondary text: labels, metadata and tool output.
#[must_use]
pub(crate) const fn text_muted(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(138, 145, 160),
        ThemeMode::Light => Color::Rgb(110, 118, 133),
    }
}

/// The color that marks focus, selection and the wordmark.
#[must_use]
pub(crate) const fn accent(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => ACCENT_DARK,
        ThemeMode::Light => ACCENT_LIGHT,
    }
}

#[must_use]
pub(crate) const fn success() -> Color {
    SUCCESS
}

#[must_use]
pub(crate) const fn warning() -> Color {
    WARNING
}

#[must_use]
pub(crate) const fn danger() -> Color {
    DANGER
}

#[must_use]
pub(crate) const fn info() -> Color {
    INFO
}

/// The foreground a dimmed cell keeps while a modal is open.
#[must_use]
pub(crate) const fn scrim_foreground(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(70, 74, 86),
        ThemeMode::Light => Color::Rgb(196, 200, 210),
    }
}

/// The background a dimmed cell keeps while a modal is open.
#[must_use]
pub(crate) const fn scrim_background(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(15, 16, 20),
        ThemeMode::Light => Color::Rgb(232, 234, 240),
    }
}

/// The style of a panel.
#[must_use]
pub(crate) fn panel_style(theme: ThemeMode) -> Style {
    Style::default().fg(text(theme)).bg(surface(theme))
}

/// The style of the conversation area.
#[must_use]
pub(crate) fn transcript_style(theme: ThemeMode) -> Style {
    Style::default().fg(text(theme)).bg(background(theme))
}

/// The style of a modal.
#[must_use]
pub(crate) fn modal_style(theme: ThemeMode) -> Style {
    Style::default().fg(text(theme)).bg(surface(theme))
}

/// The style of the selected row of a list.
#[must_use]
pub(crate) fn selected_style(theme: ThemeMode) -> Style {
    Style::default().fg(text(theme)).bg(surface_alt(theme))
}

/// The style of a focused border.
#[must_use]
pub(crate) fn focus_style(theme: ThemeMode) -> Style {
    Style::default().fg(accent(theme))
}

/// The style of a resting border.
#[must_use]
pub(crate) fn border_style(theme: ThemeMode) -> Style {
    Style::default().fg(border(theme))
}

/// The style of ordinary text.
#[must_use]
pub(crate) fn text_style(theme: ThemeMode) -> Style {
    Style::default().fg(text(theme))
}

/// The style of secondary text.
#[must_use]
pub(crate) fn muted_style(theme: ThemeMode) -> Style {
    Style::default().fg(text_muted(theme))
}

/// The style of the selected tab.
#[must_use]
pub(crate) fn active_tab_style(theme: ThemeMode) -> Style {
    Style::default()
        .fg(accent(theme))
        .add_modifier(Modifier::BOLD)
}

/// The style of a tab that is not selected.
#[must_use]
pub(crate) fn inactive_tab_style(theme: ThemeMode) -> Style {
    Style::default().fg(text_muted(theme))
}

/// The background a thinking block paints at one column.
///
/// The sweep is a beam of light travelling left to right: the column under the
/// head is the brightest, the columns behind it fade through two steps, and
/// everything else stays on the neutral background. Reading a block that is
/// being written feels like the text is lit as the model reaches it.
#[must_use]
#[allow(clippy::match_same_arms)]
pub(crate) fn thinking_background(
    phase: Option<u16>,
    column: u16,
    width: u16,
    theme: ThemeMode,
) -> Color {
    let Some(phase) = phase else {
        return thinking_background_neutral(theme);
    };
    if width == 0 {
        return thinking_background_neutral(theme);
    }
    let head = phase % width;
    let distance = distance_from_head(column, head);
    match (theme, distance) {
        (ThemeMode::Dark, 0..=1) => Color::Rgb(74, 96, 150),
        (ThemeMode::Dark, 2..=4) => Color::Rgb(52, 64, 96),
        (ThemeMode::Dark, 5..=9) => Color::Rgb(38, 45, 62),
        (ThemeMode::Dark, _) => thinking_background_neutral(theme),
        (ThemeMode::Light, 0..=1) => Color::Rgb(186, 208, 244),
        (ThemeMode::Light, 2..=4) => Color::Rgb(212, 226, 246),
        (ThemeMode::Light, 5..=9) => Color::Rgb(228, 236, 248),
        (ThemeMode::Light, _) => thinking_background_neutral(theme),
    }
}

/// How far a column sits from the head of the beam.
///
/// The light has a long tail behind it and a short glow ahead, so the block
/// reads as illuminated from left to right rather than as one lit band.
fn distance_from_head(column: u16, head: u16) -> u16 {
    if column <= head {
        head - column
    } else {
        (column - head).saturating_mul(2)
    }
}

/// The foreground a thinking block paints at one column.
///
/// The head is near white, the tail fades to the muted gray the block keeps
/// when it is frozen, so the light reads as light and not as a second color.
#[must_use]
#[allow(clippy::match_same_arms)]
pub(crate) fn thinking_foreground_at(
    phase: Option<u16>,
    column: u16,
    width: u16,
    theme: ThemeMode,
) -> Color {
    let Some(phase) = phase else {
        return thinking_foreground(theme);
    };
    if width == 0 {
        return thinking_foreground(theme);
    }
    let head = phase % width;
    let distance = distance_from_head(column, head);
    match (theme, distance) {
        (ThemeMode::Dark, 0..=1) => Color::Rgb(228, 234, 248),
        (ThemeMode::Dark, 2..=4) => Color::Rgb(186, 196, 220),
        (ThemeMode::Dark, _) => thinking_foreground(theme),
        (ThemeMode::Light, 0..=1) => Color::Rgb(40, 52, 76),
        (ThemeMode::Light, 2..=4) => Color::Rgb(64, 78, 104),
        (ThemeMode::Light, _) => thinking_foreground(theme),
    }
}

/// The background a thinking block keeps once the sweep stopped.
#[must_use]
pub(crate) const fn thinking_background_neutral(theme: ThemeMode) -> Color {
    surface(theme)
}

/// The foreground of thinking text.
#[must_use]
pub(crate) const fn thinking_foreground(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(150, 158, 175),
        ThemeMode::Light => Color::Rgb(96, 104, 120),
    }
}

/// Paints a thinking block at a phase of the sweep.
///
/// The background is computed per column, so an expanded block costs one span
/// per character but no per-character allocation of its own style.
pub(crate) fn style_thinking_line(
    line: &mut ratatui::text::Line<'static>,
    phase: Option<u16>,
    width: u16,
    theme: ThemeMode,
) {
    let spans = std::mem::take(&mut line.spans);
    line.style = Style::default()
        .fg(thinking_foreground_at(phase, 0, width, theme))
        .bg(thinking_background(phase, 0, width, theme));
    let mut column = 0_u16;
    for span in spans {
        for character in span.content.chars() {
            let character_width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
            line.spans.push(ratatui::text::Span::styled(
                character.to_string(),
                span.style
                    .fg(thinking_foreground_at(phase, column, width, theme))
                    .bg(thinking_background(phase, column, width, theme)),
            ));
            column = column.saturating_add(u16::try_from(character_width).unwrap_or(u16::MAX));
        }
    }
}

/// The background of the composer while it owns the keys.
#[must_use]
pub(crate) const fn composer_background(theme: ThemeMode) -> Color {
    match theme {
        ThemeMode::Dark => Color::Rgb(34, 38, 47),
        ThemeMode::Light => Color::Rgb(244, 246, 250),
    }
}

/// The background of the composer while it does not own the keys.
///
/// It falls back to the screen background, so an idle input reads as part of
/// the conversation instead of a band waiting for attention.
#[must_use]
pub(crate) const fn composer_background_idle(theme: ThemeMode) -> Color {
    background(theme)
}

/// The rule above the composer.
#[must_use]
pub(crate) fn composer_border_style(theme: ThemeMode, focused: bool) -> Style {
    if focused {
        Style::default().fg(accent(theme))
    } else {
        Style::default().fg(border(theme))
    }
}
