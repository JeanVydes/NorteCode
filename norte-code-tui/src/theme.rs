//! Norte themes, all built on a vivid magenta primary.
//!
//! The four built-in themes cover the four common preferences: a dark
//! default, a blinding white, a warm bone pastel, and a true OLED black.
//! Everything else in the interface derives from `terminalui::Theme`.

use ratatui::style::Color;
use terminalui::Theme;

const MAGENTA: Color = Color::Rgb(255, 47, 179);
const MAGENTA_ON_LIGHT: Color = Color::Rgb(210, 20, 180);
const MAGENTA_ON_BONE: Color = Color::Rgb(204, 51, 153);

/// Which built-in theme is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    /// Dark background, the default.
    Dark,
    /// Blinding white, maximum contrast.
    Cegador,
    /// Warm bone pastel.
    PastelHueso,
    /// True black, #000, for OLED panels.
    Amoled,
}

impl ThemeKind {
    /// The display name used by the status chrome.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Cegador => "cegador",
            Self::PastelHueso => "pastel hueso",
            Self::Amoled => "amoled",
        }
    }

    /// The next theme in the cycle order.
    pub fn next(self) -> Self {
        match self {
            Self::Dark => Self::Cegador,
            Self::Cegador => Self::PastelHueso,
            Self::PastelHueso => Self::Amoled,
            Self::Amoled => Self::Dark,
        }
    }
}

/// The `Theme` for one of the built-in variants.
pub fn theme(kind: ThemeKind) -> Theme {
    match kind {
        ThemeKind::Dark => Theme {
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
        },
        ThemeKind::Cegador => Theme {
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
        },
        ThemeKind::PastelHueso => Theme {
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
        },
        ThemeKind::Amoled => Theme {
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
        },
    }
}
