//! Shared preference vocabulary for the settings page.

use serde::Serialize;

/// The setting a row controls.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum Setting {
    /// The palette set.
    Theme,
    /// The reasoning effort sent with turns.
    Thinking,
    /// What a message sent during a turn does.
    SendMode,
    /// How edit diffs are drawn.
    DiffMode,
    /// Where the session rail sits.
    RailPosition,
}

impl Setting {
    pub(crate) const ALL: [Self; 5] = [
        Self::Theme,
        Self::Thinking,
        Self::SendMode,
        Self::DiffMode,
        Self::RailPosition,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Thinking => "thinking",
            Self::SendMode => "send mode",
            Self::DiffMode => "diff mode",
            Self::RailPosition => "rail",
        }
    }
}

/// A request to advance one setting.
#[derive(Serialize)]
pub(crate) enum ConfigAction {
    /// Cycle the value of this setting.
    Cycle(Setting),
}

/// The values to show, supplied by the host.
pub(crate) struct ConfigData {
    /// The selected theme's name.
    pub(crate) theme: String,
    /// The reasoning effort.
    pub(crate) thinking: String,
    /// The send mode.
    pub(crate) send_mode: String,
    /// The diff layout.
    pub(crate) diff_mode: String,
    /// The rail position.
    pub(crate) rail_position: String,
}
