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
    /// Telemetry chart glyph density.
    ChartDensity,
    /// Telemetry chart contrast.
    ChartContrast,
    /// Telemetry chart dot size.
    ChartDotSize,
}

impl Setting {
    pub(crate) const ALL: [Self; 11] = [
        Self::Theme,
        Self::Thinking,
        Self::SendMode,
        Self::DiffMode,
        Self::RailPosition,
        Self::Debug,
        Self::ShowToolOutput,
        Self::ToolVerbosity,
        Self::ChartDensity,
        Self::ChartContrast,
        Self::ChartDotSize,
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
            Self::ChartDensity => "chart density",
            Self::ChartContrast => "chart contrast",
            Self::ChartDotSize => "chart dot size",
        }
    }
}

/// A request to advance one setting.
#[derive(Serialize)]
pub(crate) enum ConfigAction {
    /// Cycle the value of this setting.
    Cycle(Setting),
    /// Set a chart tuning value adjusted by its slider.
    SetChartValue(Setting, u8),
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
    /// Density from 2 through 6.
    pub(crate) chart_density: u8,
    /// Contrast in quarter steps, from 1 through 16.
    pub(crate) chart_contrast: u8,
    /// Dot size ordinal, 0 through 2.
    pub(crate) chart_dot_size: u8,
}
