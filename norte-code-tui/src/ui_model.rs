//! The client-visible state of one attached session.
//!
//! The view reads this; only the client layer mutates it. Everything here
//! is plain data, free of any notion of terminal or layout.

use crate::context::ContextUsage;
use crate::diff::DiffMode;
use crate::theme::{ThemeCatalog, ThemeChoice, ThemeKind};
use crate::ui::RailPosition;
use norte_code_wire::SessionSummary;

/// Where the active turn is.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TurnState {
    /// Nothing is being produced.
    Idle,
    /// A turn is being dispatched to the server.
    Connecting,
    /// The server is thinking.
    Thinking,
    /// A turn is streaming in.
    Streaming,
    /// A question is waiting for the user's answer.
    AwaitingAnswer,
}

/// Where the connection to the raep server stands.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionAttachmentState {
    /// Never attached, or attachment was torn down.
    Disconnected,
    /// Attaching now; no messages will flow until this resolves.
    Connecting,
    /// Attached and streaming.
    Attached,
    /// The attach attempt failed; a reason is shown instead of the chat.
    Failed {
        /// What went wrong, in words a person can act on.
        reason: String,
    },
}

/// A question from the model, waiting for an answer.
#[derive(Clone)]
pub(crate) struct PendingQuestion {
    /// The host's id for the question.
    pub(crate) question: String,
    /// The question text.
    pub(crate) prompt: String,
    /// Closed choices, when the model offered them.
    pub(crate) options: Vec<String>,
    /// Whether more than one option may be chosen.
    pub(crate) multiple: bool,
}

impl ReasoningLevel {
    /// The label shown in the config page.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Read the label the selector reports.
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        match label {
            "auto" => Some(Self::Auto),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    /// The next level in the cycle.
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Auto => Self::Low,
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Auto,
        }
    }
}

/// The effort level the reasoning selector offers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReasoningLevel {
    /// Let the model decide.
    Auto,
    /// Low effort.
    Low,
    /// Medium effort.
    Medium,
    /// High effort.
    High,
}

impl SendMode {
    /// The label shown in the config page.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Queue => "queue",
            Self::Interrupt => "interrupt",
            Self::Reject => "reject",
        }
    }

    /// Read the label the selector reports.
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        match label {
            "steer" => Some(Self::Steer),
            "queue" => Some(Self::Queue),
            "interrupt" => Some(Self::Interrupt),
            "reject" => Some(Self::Reject),
            _ => None,
        }
    }

    /// The next mode in the cycle.
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Steer => Self::Queue,
            Self::Queue => Self::Interrupt,
            Self::Interrupt => Self::Reject,
            Self::Reject => Self::Steer,
        }
    }
}

/// What to do with a message sent while a turn is running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SendMode {
    /// Steer the running turn.
    Steer,
    /// Wait behind the running turn.
    Queue,
    /// Cancel the running turn, then run the message.
    Interrupt,
    /// Refuse to send while a turn is running.
    Reject,
}

/// Whether the floating performance overlay is shown.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum DebugOverlayMode {
    /// No overlay.
    #[default]
    Off,
    /// Show frame, event and memory metrics.
    On,
}

impl DebugOverlayMode {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
        }
    }

    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Off => Self::On,
            Self::On => Self::Off,
        }
    }

    pub(crate) const fn enabled(self) -> bool {
        matches!(self, Self::On)
    }
}

impl From<bool> for DebugOverlayMode {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

/// The preferences the client keeps across sessions.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Preferences {
    /// Which theme to paint, built-in or configured.
    pub(crate) theme: ThemeChoice,
    /// Every theme the interface can paint with.
    pub(crate) catalog: ThemeCatalog,
    /// Reasoning effort, sent with every turn.
    pub(crate) reasoning: ReasoningLevel,
    /// What to do with a message sent while a turn is running.
    pub(crate) send_mode: SendMode,
    /// How edit diffs are drawn.
    pub(crate) diff_mode: DiffMode,
    /// Where the session rail sits relative to the stream.
    pub(crate) rail_position: RailPosition,
    /// Whether the floating performance overlay is shown.
    pub(crate) debug: DebugOverlayMode,
}

/// The turn the host is running, when known.
#[derive(Debug, Default, Clone)]
pub(crate) struct ActiveTurn {
    /// The turn id the host assigned.
    pub(crate) id: String,
}

/// The one big piece of state the interface reads from.
pub(crate) struct UiModel {
    pub(crate) preferences: Preferences,
    pub(crate) attachment: SessionAttachmentState,
    pub(crate) turn: TurnState,
    /// The turn the host is running, when known; cleared on turn end.
    pub(crate) active_turn: Option<ActiveTurn>,
    pub(crate) sessions: Vec<SessionSummary>,
    /// The session the interface is attached to; set on attach.
    pub(crate) active: Option<norte_code_wire::SessionKey>,
    /// The model catalogue of the attached server.
    pub(crate) models: Vec<norte_code_wire::ModelInfo>,
    /// The attachment's plans.
    pub(crate) plans: Vec<norte_code_wire::PlanSummary>,
    /// Background jobs of the attached session.
    pub(crate) jobs: Vec<norte_code_wire::JobSummary>,
    /// Fleet telemetry shared with the views.
    pub(crate) telemetry: Option<crate::shared_telemetry::SharedTelemetry>,
    /// Session rollups computed once for each telemetry report.
    pub(crate) telemetry_usage: crate::session_stats::SharedUsage,
    /// Installed plugins.
    pub(crate) plugins: Vec<norte_code_client::PluginSummary>,
    /// The model the host reports for the attached session, when known.
    pub(crate) active_model: Option<norte_code_wire::AgentRef>,
    /// The provider the host reports for the attached session, when known.
    pub(crate) active_provider: Option<String>,
    /// The question the model is parked on, when one is open.
    pub(crate) question: Option<PendingQuestion>,
    /// The catalogue detail for the HOST view.
    pub(crate) catalog: Option<norte_code_client::CatalogDetail>,
    /// The agents the server reported.
    pub(crate) agents: Vec<norte_code_wire::AgentSummary>,
    /// What the server says about itself.
    pub(crate) server_info: Option<norte_code_client::ServerInfo>,
    /// The liveness probe answer.
    pub(crate) liveness: Option<norte_code_client::Liveness>,
    /// The readiness probe answer.
    pub(crate) readiness: Option<norte_code_client::Readiness>,
    /// The tenant's quota report.
    pub(crate) quota: Option<norte_code_wire::QuotaReport>,
    /// The configured service URL, shown as the host address.
    pub(crate) service_url: String,
    /// How full the session's context window is.
    pub(crate) context: ContextUsage,
    /// The fleet-wide cost total, when the providers report one.
    pub(crate) cost: Option<norte_code_client::CostReport>,
    /// Which tab is showing.
    pub(crate) tab: Tab,
    /// The title of the session the stream is attached to, for the attach notice.
    pub(crate) attached_title: Option<String>,
    /// A plugin operation in flight, when one is.
    pub(crate) plugin_job: Option<PluginJob>,
}

/// A plugin operation the host is running right now.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct PluginJob {
    /// The operation: `install`, `update` or `remove`.
    pub(crate) operation: String,
    /// The step it reached.
    pub(crate) step: u64,
    /// How many steps the operation has.
    pub(crate) total: u64,
    /// What the step is doing, in a few words.
    pub(crate) stage: String,
}

/// The tabs of the interface.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(crate) enum Tab {
    /// The conversation.
    #[default]
    Chat,
    /// The sessions list.
    Sessions,
    /// The model catalogue.
    Models,
    /// The telemetry page.
    Telemetry,
    /// The settings page: host, agents, plugins and preferences.
    Settings,
}

impl Tab {
    /// Every tab, in display order.
    pub(crate) const ALL: [Self; 5] = [
        Self::Chat,
        Self::Sessions,
        Self::Models,
        Self::Telemetry,
        Self::Settings,
    ];

    /// The tab's position in display order.
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Chat => 0,
            Self::Sessions => 1,
            Self::Models => 2,
            Self::Telemetry => 3,
            Self::Settings => 4,
        }
    }

    /// Whether this tab has two lists that Tab switches between.
    pub(crate) const fn is_list_tab(self) -> bool {
        matches!(self, Self::Telemetry | Self::Settings)
    }

    /// Cycle to the next tab.
    #[must_use]
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Chat => Self::Sessions,
            Self::Sessions => Self::Models,
            Self::Models => Self::Telemetry,
            Self::Telemetry => Self::Settings,
            Self::Settings => Self::Chat,
        }
    }

    /// Cycle to the previous tab.
    #[must_use]
    pub(crate) const fn previous(self) -> Self {
        match self {
            Self::Chat => Self::Settings,
            Self::Sessions => Self::Chat,
            Self::Models => Self::Sessions,
            Self::Telemetry => Self::Models,
            Self::Settings => Self::Telemetry,
        }
    }

    /// The label shown in the tab bar.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Sessions => "Sessions",
            Self::Models => "Models",
            Self::Telemetry => "Telemetry",
            Self::Settings => "Settings",
        }
    }

    /// A short label for narrow screens.
    pub(crate) const fn short_label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Sessions => "Sess",
            Self::Models => "Models",
            Self::Telemetry => "Tele",
            Self::Settings => "Config",
        }
    }

    /// The tab for a digit, when one matches.
    pub(crate) const fn from_digit(digit: char) -> Option<Self> {
        match digit {
            '1' => Some(Self::Chat),
            '2' => Some(Self::Sessions),
            '3' => Some(Self::Models),
            '4' => Some(Self::Telemetry),
            '5' => Some(Self::Settings),
            _ => None,
        }
    }
}

impl UiModel {
    /// The context to show: reported usage, with the window filled from the
    /// active model when the host never sent session stats.
    pub(crate) fn effective_context(&self) -> ContextUsage {
        let mut usage = self.context;
        if usage.window.is_none() {
            usage.window = self
                .active_model_info()
                .and_then(|info| info.max_context)
                .filter(|window| *window > 0);
        }
        usage
    }

    /// The catalogue entry for the model the attached session is using.
    pub(crate) fn active_model_info(&self) -> Option<&norte_code_wire::ModelInfo> {
        let agent = self.active_model.as_ref()?;
        self.models
            .iter()
            .find(|info| {
                info.model == agent.id
                    && self
                        .active_provider
                        .as_ref()
                        .is_none_or(|provider| provider == &info.provider)
            })
            .or_else(|| self.models.iter().find(|info| info.model == agent.id))
    }

    /// Whether the attached session has a model configured.
    pub(crate) fn active_session_has_model(&self) -> bool {
        if let Some(active) = self.active.as_ref()
            && let Some(found) = self
                .sessions
                .iter()
                .find(|candidate| candidate.session_id == active.as_str())
        {
            return found.model.is_some();
        }
        self.active_model.is_some()
    }

    /// A fresh model for a client that never attached yet.
    pub(crate) fn detached() -> Self {
        Self {
            preferences: Preferences {
                theme: ThemeChoice::BuiltIn(ThemeKind::Dark),
                catalog: ThemeCatalog::default(),
                reasoning: ReasoningLevel::Auto,
                send_mode: SendMode::Steer,
                diff_mode: DiffMode::Unified,
                rail_position: RailPosition::Left,
                debug: DebugOverlayMode::Off,
            },
            attachment: SessionAttachmentState::Disconnected,
            turn: TurnState::Idle,
            active_turn: None,
            sessions: Vec::new(),
            active: None,
            models: Vec::new(),
            plans: Vec::new(),
            jobs: Vec::new(),
            telemetry: None,
            telemetry_usage: crate::session_stats::SharedUsage::default(),
            plugins: Vec::new(),
            catalog: None,
            agents: Vec::new(),
            server_info: None,
            liveness: None,
            readiness: None,
            quota: None,
            service_url: String::new(),
            context: ContextUsage::default(),
            cost: None,
            active_model: None,
            active_provider: None,
            question: None,
            tab: Tab::Chat,
            attached_title: None,
            plugin_job: None,
        }
    }
}

#[cfg(test)]
#[path = "ui_model_tests.rs"]
mod tests;
