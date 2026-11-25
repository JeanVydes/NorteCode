//! Shared preference vocabulary for the settings page.

use serde::Serialize;

use crate::ui_model::DebugOverlayMode;

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
    /// Whether the floating performance overlay is shown.
    Debug,
    /// Whether tool results start expanded.
    ShowToolOutput,
    /// Whether reasoning and tool activity is grouped.
    ToolVerbosity,
}

impl Setting {
    pub(crate) const ALL: [Self; 8] = [
        Self::Theme,
        Self::Thinking,
        Self::SendMode,
        Self::DiffMode,
        Self::RailPosition,
        Self::Debug,
        Self::ShowToolOutput,
        Self::ToolVerbosity,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Thinking => "thinking",
            Self::SendMode => "send mode",
            Self::DiffMode => "diff mode",
            Self::RailPosition => "rail",
            Self::Debug => "debug overlay",
            Self::ShowToolOutput => "tool output expanded",
            Self::ToolVerbosity => "tool verbosity",
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
    /// The floating performance overlay.
    pub(crate) debug: DebugOverlayMode,
    /// Whether tool results start expanded.
    pub(crate) show_tool_output: bool,
    /// Whether non-text activity is grouped.
    pub(crate) tool_verbosity: bool,
}
