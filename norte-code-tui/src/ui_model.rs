//! The client-visible state of one attached session.
//!
//! The view reads this; only the client layer mutates it. Everything here
//! is plain data, free of any notion of terminal or layout.

use crate::diff::DiffMode;
use crate::theme::{ThemeCatalog, ThemeChoice, ThemeKind};
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
    /// Telemetry of the attached session.
    pub(crate) telemetry: Option<norte_code_wire::TelemetryReport>,
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
    /// The telemetry for the attached session.
    pub(crate) telemetry_session: Option<norte_code_wire::TelemetryReport>,
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
    /// The plans.
    Plans,
    /// The jobs.
    Jobs,
    /// The telemetry page.
    Telemetry,
    /// The host overview.
    Host,
    /// The plugins page.
    Plugins,
    /// The costs page.
    Costs,
    /// The preferences page.
    Config,
}

impl Tab {
    /// Cycle to the next tab.
    #[must_use]
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Chat => Self::Sessions,
            Self::Sessions => Self::Models,
            Self::Models => Self::Plans,
            Self::Plans => Self::Jobs,
            Self::Jobs => Self::Telemetry,
            Self::Telemetry => Self::Host,
            Self::Host => Self::Plugins,
            Self::Plugins => Self::Costs,
            Self::Costs => Self::Config,
            Self::Config => Self::Chat,
        }
    }

    /// Cycle to the previous tab.
    #[must_use]
    pub(crate) const fn previous(self) -> Self {
        match self {
            Self::Chat => Self::Config,
            Self::Config => Self::Costs,
            Self::Sessions => Self::Chat,
            Self::Models => Self::Sessions,
            Self::Plans => Self::Models,
            Self::Jobs => Self::Plans,
            Self::Telemetry => Self::Jobs,
            Self::Host => Self::Telemetry,
            Self::Plugins => Self::Host,
            Self::Costs => Self::Plugins,
        }
    }

    /// The label shown in the tab bar.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Sessions => "Sessions",
            Self::Models => "Models",
            Self::Plans => "Plans",
            Self::Jobs => "Jobs",
            Self::Telemetry => "Telemetry",
            Self::Host => "Host",
            Self::Plugins => "Plugins",
            Self::Costs => "Costs",
            Self::Config => "Config",
        }
    }

    /// A short label for narrow screens.
    pub(crate) const fn short_label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Sessions => "Sess",
            Self::Models => "Models",
            Self::Plans => "Plans",
            Self::Jobs => "Jobs",
            Self::Telemetry => "Tele",
            Self::Host => "Host",
            Self::Plugins => "Plug",
            Self::Costs => "Cost",
            Self::Config => "Cfg",
        }
    }

    /// The tab for a digit, when one matches.
    pub(crate) const fn from_digit(digit: char) -> Option<Self> {
        match digit {
            '1' => Some(Self::Chat),
            '2' => Some(Self::Sessions),
            '3' => Some(Self::Models),
            '4' => Some(Self::Plans),
            '5' => Some(Self::Jobs),
            '6' => Some(Self::Telemetry),
            '7' => Some(Self::Host),
            '8' => Some(Self::Plugins),
            '9' => Some(Self::Costs),
            '0' => Some(Self::Config),
            _ => None,
        }
    }
}

impl UiModel {
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
            plugins: Vec::new(),
            catalog: None,
            agents: Vec::new(),
            telemetry_session: None,
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
