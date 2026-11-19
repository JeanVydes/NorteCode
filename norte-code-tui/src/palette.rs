//! The color palette: the twelve roles the interface paints with.
//!
//! `PaletteConfig` is the partial form in `cli.json`; every missing or
//! invalid color falls back to the base palette. `Palette` is the complete,
//! validated form that becomes the library's render theme.

use ratatui::style::Color;
use serde::Deserialize;
use terminalui::Theme;

const MAGENTA: Color = Color::Rgb(255, 47, 179);
const MAGENTA_ON_LIGHT: Color = Color::Rgb(210, 20, 180);
const MAGENTA_ON_BONE: Color = Color::Rgb(204, 51, 153);

/// Color overrides as they appear in the UI config.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub(crate) struct PaletteConfig {
    /// Backdrop of every view.
    #[serde(default)]
    pub(crate) background: Option<String>,
    /// Normal text.
    #[serde(default)]
    pub(crate) foreground: Option<String>,
    /// Primary color.
    #[serde(default)]
    pub(crate) primary: Option<String>,
    /// Raised surfaces.
    #[serde(default)]
    pub(crate) secondary: Option<String>,
    /// De-emphasized text.
    #[serde(default)]
    pub(crate) muted: Option<String>,
    /// Highlights and carets.
    #[serde(default)]
    pub(crate) accent: Option<String>,
    /// Destructive actions and errors.
    #[serde(default)]
    pub(crate) destructive: Option<String>,
    /// Success and confirmation.
    #[serde(default)]
    pub(crate) success: Option<String>,
    /// Warnings.
    #[serde(default)]
    pub(crate) warning: Option<String>,
    /// Panel borders.
    #[serde(default)]
    pub(crate) border: Option<String>,
    /// Focused borders.
    #[serde(default)]
    pub(crate) ring: Option<String>,
    /// Chrome border accents.
    #[serde(default)]
    pub(crate) border_set: Option<String>,
}

/// The twelve colors the interface paints with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Palette {
    /// Backdrop of every view.
    pub(crate) background: Color,
    /// Normal text.
    pub(crate) foreground: Color,
    /// Primary color.
    pub(crate) primary: Color,
    /// Raised surfaces.
    pub(crate) secondary: Color,
    /// De-emphasized text.
    pub(crate) muted: Color,
    /// Highlights and carets.
    pub(crate) accent: Color,
    /// Destructive actions and errors.
    pub(crate) destructive: Color,
    /// Success and confirmation.
    pub(crate) success: Color,
    /// Warnings.
    pub(crate) warning: Color,
    /// Panel borders.
    pub(crate) border: Color,
    /// Focused borders.
    pub(crate) ring: Color,
    /// Chrome border accents.
    pub(crate) border_set: Color,
}

impl Palette {
    /// The built-in dark palette.
    pub(crate) fn dark() -> Self {
        Self {
            background: Color::Rgb(22, 24, 29),
            foreground: Color::Rgb(224, 227, 235),
            primary: MAGENTA,
            secondary: Color::Rgb(30, 33, 40),
            muted: Color::Rgb(138, 145, 160),
            accent: MAGENTA,
            destructive: Color::Rgb(232, 108, 122),
            success: Color::Rgb(126, 200, 154),
            warning: Color::Rgb(230, 180, 105),
            border: Color::Rgb(58, 62, 74),
            ring: MAGENTA,
            border_set: Color::Rgb(58, 62, 74),
        }
    }

    /// The built-in blinding white palette.
    pub(crate) fn cegador() -> Self {
        Self {
            background: Color::Rgb(255, 255, 255),
            foreground: Color::Rgb(14, 14, 14),
            primary: MAGENTA_ON_LIGHT,
            secondary: Color::Rgb(240, 240, 243),
            muted: Color::Rgb(105, 105, 112),
            accent: MAGENTA_ON_LIGHT,
            destructive: Color::Rgb(214, 40, 60),
            success: Color::Rgb(0, 140, 70),
            warning: Color::Rgb(190, 110, 0),
            border: Color::Rgb(220, 220, 226),
            ring: MAGENTA_ON_LIGHT,
            border_set: Color::Rgb(220, 220, 226),
        }
    }

    /// The built-in warm bone palette.
    pub(crate) fn pastel_hueso() -> Self {
        Self {
            background: Color::Rgb(247, 241, 231),
            foreground: Color::Rgb(66, 57, 48),
            primary: MAGENTA_ON_BONE,
            secondary: Color::Rgb(235, 228, 217),
            muted: Color::Rgb(145, 133, 120),
            accent: MAGENTA_ON_BONE,
            destructive: Color::Rgb(205, 95, 100),
            success: Color::Rgb(92, 152, 108),
            warning: Color::Rgb(188, 140, 64),
            border: Color::Rgb(222, 212, 197),
            ring: MAGENTA_ON_BONE,
            border_set: Color::Rgb(222, 212, 197),
        }
    }

    /// The built-in OLED black palette.
    pub(crate) fn amoled() -> Self {
        Self {
            background: Color::Rgb(0, 0, 0),
            foreground: Color::Rgb(245, 245, 245),
            primary: MAGENTA,
            secondary: Color::Rgb(14, 14, 14),
            muted: Color::Rgb(158, 158, 158),
            accent: MAGENTA,
            destructive: Color::Rgb(232, 108, 122),
            success: Color::Rgb(126, 200, 154),
            warning: Color::Rgb(230, 180, 105),
            border: Color::Rgb(42, 42, 42),
            ring: MAGENTA,
            border_set: Color::Rgb(42, 42, 42),
        }
    }

    /// Overlay the config values onto a base palette.
    pub(crate) fn from_config(config: PaletteConfig, base: &Palette) -> Self {
        let value = |text: Option<String>| text.as_deref().and_then(parse_color);
        Self {
            background: value(config.background).unwrap_or(base.background),
            foreground: value(config.foreground).unwrap_or(base.foreground),
            primary: value(config.primary).unwrap_or(base.primary),
            secondary: value(config.secondary).unwrap_or(base.secondary),
            muted: value(config.muted).unwrap_or(base.muted),
            accent: value(config.accent).unwrap_or(base.accent),
            destructive: value(config.destructive).unwrap_or(base.destructive),
            success: value(config.success).unwrap_or(base.success),
            warning: value(config.warning).unwrap_or(base.warning),
            border: value(config.border).unwrap_or(base.border),
            ring: value(config.ring).unwrap_or(base.ring),
            border_set: value(config.border_set).unwrap_or(base.border_set),
        }
    }
}

impl From<Palette> for Theme {
    fn from(palette: Palette) -> Self {
        Self {
            background: palette.background,
            foreground: palette.foreground,
            primary: palette.primary,
            secondary: palette.secondary,
            muted: palette.muted,
            accent: palette.accent,
            destructive: palette.destructive,
            success: palette.success,
            warning: palette.warning,
            border: palette.border,
            ring: palette.ring,
            border_set: palette.border_set,
        }
    }
}

fn parse_color(text: &str) -> Option<Color> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 {
        return None;
    }
    let red = u8::from_str_radix(digits.get(0..2)?, 16).ok()?;
    let green = u8::from_str_radix(digits.get(2..4)?, 16).ok()?;
    let blue = u8::from_str_radix(digits.get(4..6)?, 16).ok()?;
    Some(Color::Rgb(red, green, blue))
}

#[cfg(test)]
#[path = "palette_tests.rs"]
mod tests;
