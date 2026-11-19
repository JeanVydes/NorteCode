//! The lines the chrome is drawn with: tabs, status and help.
//!
//! Every builder measures in display width, never in characters, so a wide
//! glyph cannot shift the columns the reader clicks on.

mod conversation;
mod data;
mod fit;
mod help;
pub(crate) mod host;
pub(crate) mod status;
mod tabs;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::components::{ComponentId, LifecyclePhase, palette};
use crate::ui_model::ThemeMode;

pub struct TextBuilder;

impl TextBuilder {
    /// The marker a selected row carries.
    pub(crate) const SELECTED_MARKER: &'static str = "▌ ";
}

impl TextBuilder {
    /// A token count short enough to read.
    pub(crate) fn human_count(value: u64) -> String {
        let (divisor, suffix) = if value >= 1_000_000 {
            (1_000_000, "M")
        } else if value >= 1_000 {
            (1_000, "k")
        } else {
            return value.to_string();
        };
        let whole = value / divisor;
        let tenth = (value % divisor) * 10 / divisor;
        format!("{whole}.{tenth}{suffix}")
    }

    /// A duration short enough to read.
    pub(crate) fn duration(milliseconds: u64) -> String {
        if milliseconds < 1_000 {
            format!("{milliseconds}ms")
        } else if milliseconds < 60_000 {
            format!("{}.{}s", milliseconds / 1_000, (milliseconds % 1_000) / 100)
        } else {
            format!(
                "{}m{:02}s",
                milliseconds / 60_000,
                (milliseconds % 60_000) / 1_000
            )
        }
    }

    /// The caption a panel carries over its content.
    #[must_use]
    pub const fn panel_title_for(view: ComponentId) -> &'static str {
        match view {
            ComponentId::Session => "conversation",
            ComponentId::Sessions => "sessions",
            ComponentId::Models => "providers and models",
            ComponentId::Plans => "plans",
            ComponentId::Jobs => "jobs",
            ComponentId::Telemetry => "requests",
            ComponentId::Host => "capabilities",
            ComponentId::Plugins => "plugins",
            ComponentId::Costs => "costs",
            ComponentId::PlanReview => "plan review",
        }
    }

    #[must_use]
    pub const fn view_label(view: ComponentId) -> &'static str {
        match view {
            ComponentId::Session => "Chat",
            ComponentId::Sessions => "Sessions",
            ComponentId::Models => "Models",
            ComponentId::Plans => "Plans",
            ComponentId::Jobs => "Jobs",
            ComponentId::Telemetry => "Telemetry",
            ComponentId::Host => "Host",
            ComponentId::Plugins => "Plugins",
            ComponentId::Costs => "Costs",
            ComponentId::PlanReview => "Review",
        }
    }

    #[must_use]
    pub const fn phase_label(phase: LifecyclePhase) -> &'static str {
        match phase {
            LifecyclePhase::Dormant => "inactive",
            LifecyclePhase::Mounting => "starting",
            LifecyclePhase::Loading => "loading",
            LifecyclePhase::Ready => "ready",
            LifecyclePhase::Empty => "empty",
            LifecyclePhase::Updating => "updating",
            LifecyclePhase::Failed => "failed",
        }
    }

    #[must_use]
    pub fn panel_title(view: ComponentId, phase: LifecyclePhase) -> Line<'static> {
        Line::from(vec![
            Span::raw(Self::panel_title_for(view)),
            Span::raw("  "),
            Span::styled(Self::phase_label(phase), Self::phase_style(phase)),
        ])
    }

    #[must_use]
    pub fn phase_style(phase: LifecyclePhase) -> Style {
        match phase {
            LifecyclePhase::Dormant | LifecyclePhase::Empty => {
                Style::default().fg(palette::text_muted(ThemeMode::Dark))
            }
            LifecyclePhase::Mounting | LifecyclePhase::Loading | LifecyclePhase::Updating => {
                Style::default().fg(palette::warning())
            }
            LifecyclePhase::Ready => Style::default().fg(palette::success()),
            LifecyclePhase::Failed => Style::default()
                .fg(palette::danger())
                .add_modifier(Modifier::BOLD),
        }
    }

    #[must_use]
    pub const fn empty_message(view: ComponentId) -> &'static str {
        match view {
            ComponentId::Session => "No active session",
            ComponentId::Sessions => "No sessions loaded",
            ComponentId::Models => "No models loaded",
            ComponentId::Plans => "No plans loaded",
            ComponentId::Jobs => "No jobs in this session",
            ComponentId::Telemetry => "No telemetry loaded",
            ComponentId::Host => "No host capabilities loaded",
            ComponentId::Plugins => "No plugins loaded",
            ComponentId::Costs => "No cost reports loaded",
            ComponentId::PlanReview => "No plan review is active",
        }
    }

    #[must_use]
    pub const fn host_title() -> &'static str {
        "host"
    }

    #[must_use]
    pub const fn details_title() -> &'static str {
        "plan detail"
    }

    #[must_use]
    pub const fn transcript_empty() -> &'static str {
        "No messages yet"
    }

    #[must_use]
    pub const fn no_model_matches() -> &'static str {
        "No providers or models match this filter"
    }

    /// What the composer says while it does not own the keys.
    ///
    /// It invites the reader in first, because typing here does nothing until
    /// the composer is focused.
    #[must_use]
    pub fn composer_placeholder() -> String {
        format!(
            "Press {} or click here to write",
            crate::keys::COMPOSER_CHAR
        )
    }

    /// What the composer says once it owns the keys.
    #[must_use]
    pub const fn composer_focused_placeholder() -> &'static str {
        "Type a message, Enter sends, Esc leaves the composer"
    }

    #[must_use]
    pub const fn composer_busy_placeholder() -> &'static str {
        "Wait for the current response to finish"
    }

    #[must_use]
    pub const fn child_session_read_only() -> &'static str {
        "Subagent session, view only"
    }

    #[must_use]
    pub const fn plan_selection_hint() -> &'static str {
        "Select a plan to inspect it"
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
