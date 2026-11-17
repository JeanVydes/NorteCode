use norte_code_client::{
    CatalogDetail, CostReport, Liveness, McpCatalog, PluginSummary, Readiness, ServerInfo,
    SkillCatalog, ToolRule,
};
use norte_code_wire::{
    AgentSummary, BudgetAction, BudgetScope, BudgetUnit, ChildSessionSummary, JobInfo, ModelInfo,
    PlanSummary, QuotaReport, ServerEvent, ServerEventKind, SessionKey, SessionSummary,
    TelemetryReport, TodoInfo, TurnPhaseName,
};
use serde::{Deserialize, Serialize};

use crate::tools::DiffMode;
pub(crate) enum TranscriptItem {
    UserMessage {
        text: String,
        chips: Vec<crate::chips::Chip>,
        /// The message was queued behind the running turn and has not been
        /// dispatched to it yet.
        queued: bool,
        /// The turn the host named for this message, filled in when it is
        /// accepted. The identity the history page shares, so the live
        /// copy of the message can be matched to its canonical version.
        turn_id: Option<String>,
    },
    Status(String),
    History(norte_code_wire::HistoryItem),
    ServerEvent(Box<ServerEvent>),
    Error(String),
    Usage(UsageSummary),
}

/// What one model request reported about itself.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UsageSummary {
    /// The turn this request belongs to, as the host numbers it.
    pub(crate) turn: Option<u64>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) cost_usd: Option<f64>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) tokens_per_second: Option<f64>,
}

impl UsageSummary {
    /// Folds what another request of the same turn reported into this one.
    ///
    /// A turn shows one footer, so the counts are summed and the names are
    /// kept from the last request that said them; the speed is what the
    /// whole turn averaged once both totals are known.
    pub(crate) fn absorb(&mut self, other: Self) {
        self.turn = self.turn.or(other.turn);
        if other.provider.is_some() {
            self.provider = other.provider;
        }
        if other.model.is_some() {
            self.model = other.model;
        }
        self.input_tokens = sum_counts(self.input_tokens, other.input_tokens);
        self.output_tokens = sum_counts(self.output_tokens, other.output_tokens);
        self.cache_read_tokens = sum_counts(self.cache_read_tokens, other.cache_read_tokens);
        self.cost_usd = sum_costs(self.cost_usd, other.cost_usd);
        self.duration_ms = sum_counts(self.duration_ms, other.duration_ms);
        self.tokens_per_second = match (self.output_tokens, self.duration_ms) {
            (Some(out), Some(ms)) if out > 0 && ms > 0 => {
                #[allow(clippy::cast_precision_loss)]
                let per_second = out as f64 / (ms as f64 / 1000.0);
                Some(per_second)
            }
            _ => self.tokens_per_second.or(other.tokens_per_second),
        };
    }
}

/// Two request counts as one: what either reported, summed, or the one that
/// reported when the other said nothing.
fn sum_counts(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        (Some(a), None) | (None, Some(a)) => Some(a),
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
    }
}

/// Two reported costs as one, the same way.
fn sum_costs(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, None) => None,
        (Some(a), None) | (None, Some(a)) => Some(a),
        (Some(a), Some(b)) => Some(a + b),
    }
}

/// A user turn the reader can rewind to or pin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TurnPoint {
    pub(crate) turn_id: String,
    pub(crate) preview: String,
}

/// The model a session runs on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionModel {
    pub(crate) provider: String,
    pub(crate) model: String,
}

/// How much the session reasons.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ReasoningLevel {
    #[default]
    Automatic,
    Disabled,
    Low,
    Medium,
    High,
}

/// The refusal one generation has already lowered to automatic.
#[derive(Default)]
pub(crate) struct ReasoningRecovery {
    refusal: Option<(u64, ReasoningLevel)>,
}

impl ReasoningRecovery {
    pub(crate) fn claim(&mut self, generation: u64, refused: ReasoningLevel) -> bool {
        let candidate = (generation, refused);
        if self.refusal == Some(candidate) {
            return false;
        }
        self.refusal = Some(candidate);
        true
    }

    pub(crate) fn clear(&mut self) {
        self.refusal = None;
    }
}

impl ReasoningLevel {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Automatic => "auto",
            Self::Disabled => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub(crate) fn from_wire_name(name: &str) -> Option<Self> {
        let normalized = name.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "auto" | "automatic" => Some(Self::Automatic),
            "off" | "disabled" | "none" => Some(Self::Disabled),
            "low" | "minimal" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" | "max" | "xhigh" => Some(Self::High),
            _ => None,
        }
    }

    /// Restores the local level the host reports for a session.
    pub(crate) fn from_wire(setting: &norte_code_wire::Reasoning) -> Self {
        match setting {
            norte_code_wire::Reasoning::Disabled => Self::Disabled,
            norte_code_wire::Reasoning::Effort { effort } => {
                Self::from_wire_name(effort).unwrap_or(Self::Automatic)
            }
            norte_code_wire::Reasoning::Automatic | norte_code_wire::Reasoning::Budget { .. } => {
                Self::Automatic
            }
        }
    }

    pub(crate) fn incompatible_with(
        self,
        capabilities: &norte_code_wire::ReasoningInfo,
    ) -> Option<String> {
        match self {
            Self::Disabled if capabilities.supported && !capabilities.can_disable => {
                Some("the model does not allow reasoning to be turned off".to_owned())
            }
            Self::Automatic | Self::Disabled => None,
            Self::Low | Self::Medium | Self::High if !capabilities.supported => {
                Some("the model does not support reasoning".to_owned())
            }
            Self::Low | Self::Medium | Self::High => {
                if capabilities
                    .efforts
                    .iter()
                    .any(|effort| Self::from_wire_name(effort) == Some(self))
                {
                    None
                } else if capabilities.efforts.is_empty() {
                    Some("the model does not accept any reasoning efforts".to_owned())
                } else {
                    Some(format!(
                        "the model does not support effort `{}`; it supports {}",
                        self.label(),
                        capabilities.efforts.join(", ")
                    ))
                }
            }
        }
    }

    pub(crate) fn to_wire(self) -> norte_code_wire::Reasoning {
        match self {
            Self::Automatic => norte_code_wire::Reasoning::Automatic,
            Self::Disabled => norte_code_wire::Reasoning::Disabled,
            Self::Low => norte_code_wire::Reasoning::Effort {
                effort: "low".to_owned(),
            },
            Self::Medium => norte_code_wire::Reasoning::Effort {
                effort: "medium".to_owned(),
            },
            Self::High => norte_code_wire::Reasoning::Effort {
                effort: "high".to_owned(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TurnState {
    #[default]
    Idle,
    Connecting,
    Thinking,
    Streaming,
    AwaitingAnswer,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SessionAttachmentState {
    #[default]
    Disconnected,
    Connecting,
    Attached,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BudgetApprovalRequest {
    pub(crate) scope: BudgetScope,
    pub(crate) unit: BudgetUnit,
    pub(crate) spent: f64,
    pub(crate) limit: f64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TelemetryScope {
    #[default]
    Host,
    Session,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemeMode {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThinkingVisibility {
    #[default]
    Collapsed,
    Expanded,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub(crate) enum Toggle {
    #[default]
    Off,
    On,
}

impl Toggle {
    pub(crate) const fn is_on(self) -> bool {
        matches!(self, Self::On)
    }

    pub(crate) const fn toggled(self) -> Self {
        match self {
            Self::Off => Self::On,
            Self::On => Self::Off,
        }
    }
}

impl From<bool> for Toggle {
    fn from(value: bool) -> Self {
        if value { Self::On } else { Self::Off }
    }
}

impl From<Toggle> for bool {
    fn from(value: Toggle) -> Self {
        value.is_on()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TuiPreferences {
    pub(crate) theme: ThemeMode,
    pub(crate) thinking: ThinkingVisibility,
    pub(crate) show_timestamps: Toggle,
    pub(crate) show_tool_output: Toggle,
    pub(crate) auto_refresh_sessions: Toggle,
    pub(crate) developer_mode: Toggle,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TuiCliPreferences {
    pub(crate) diff_style: DiffMode,
}

impl Default for TuiPreferences {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            thinking: ThinkingVisibility::Collapsed,
            show_timestamps: Toggle::On,
            show_tool_output: Toggle::On,
            auto_refresh_sessions: Toggle::Off,
            developer_mode: Toggle::Off,
        }
    }
}

impl TurnState {
    pub(crate) const fn is_busy(self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub(crate) fn apply_event(&mut self, event: &ServerEventKind) {
        match event {
            ServerEventKind::TurnFinished { .. }
            | ServerEventKind::TurnStopped { .. }
            | ServerEventKind::Error { .. } => *self = Self::Idle,
            ServerEventKind::QuestionAsked { .. } => *self = Self::AwaitingAnswer,
            ServerEventKind::QuestionAnswered { .. } | ServerEventKind::TurnStarted { .. } => {
                *self = Self::Thinking;
            }
            ServerEventKind::InputAccepted { queued, .. } => {
                *self = if *queued {
                    Self::Connecting
                } else {
                    Self::Thinking
                };
            }
            ServerEventKind::TurnPhase { phase, .. } => {
                *self = match phase {
                    TurnPhaseName::Queued => Self::Connecting,
                    TurnPhaseName::CallingProvider => Self::Thinking,
                    TurnPhaseName::ExecutingTool | TurnPhaseName::AwaitingApproval => {
                        Self::Streaming
                    }
                };
            }
            ServerEventKind::Delta { delta, .. } => match delta {
                norte_code_wire::Delta::Reasoning { .. } => *self = Self::Thinking,
                norte_code_wire::Delta::Text { .. } | norte_code_wire::Delta::Refusal { .. } => {
                    *self = Self::Streaming;
                }
                norte_code_wire::Delta::ToolArguments { .. }
                | norte_code_wire::Delta::Media { .. } => {}
            },
            ServerEventKind::ToolStarted { .. } | ServerEventKind::ToolOutput { .. } => {
                *self = Self::Streaming;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "ui_model_tests.rs"]
mod tests;

/// How the composer sends the next input while a turn is running.
///
/// RAEP spells the same choice as its `QueuePolicy`; the client's default is
/// `steer`, which reaches the request in flight instead of refusing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SendMode {
    /// Reach the request in flight, or start a turn when none is running.
    #[default]
    Steer,
    /// Wait behind the turn in flight.
    Queue,
    /// Cancel the turn in flight, then run this input.
    Interrupt,
    /// Refuse the input while a turn is running.
    Reject,
}

#[derive(Default)]
pub(crate) struct UiModel {
    pub(crate) endpoint: String,
    pub(crate) authentication_configured: bool,
    pub(crate) host_errors: Vec<(String, String)>,
    pub(crate) preferences: TuiPreferences,
    pub(crate) cli: TuiCliPreferences,
    pub(crate) server: Option<ServerInfo>,
    pub(crate) liveness: Option<Liveness>,
    pub(crate) readiness: Option<Readiness>,
    pub(crate) quota: Option<QuotaReport>,
    pub(crate) agents: Vec<AgentSummary>,
    pub(crate) skills: Option<SkillCatalog>,
    pub(crate) mcp: Option<McpCatalog>,
    pub(crate) tool_rules: Vec<ToolRule>,
    pub(crate) sessions: Vec<SessionSummary>,
    pub(crate) active_session: Option<SessionKey>,
    pub(crate) active_child: Option<SessionKey>,
    pub(crate) attachment_state: SessionAttachmentState,
    /// Whether the host is known to be unreachable: the reader was told
    /// once, so the retries that keep failing stay out of the transcript.
    pub(crate) service_down: bool,
    pub(crate) budget_approval: Option<BudgetApprovalRequest>,
    pub(crate) notice: Option<String>,
    pub(crate) transcript: Vec<TranscriptItem>,
    pub(crate) turn_state: TurnState,
    pub(crate) send_mode: SendMode,
    pub(crate) budget_action: BudgetAction,
    pub(crate) active_turn: Option<String>,
    pub(crate) session_model: Option<SessionModel>,
    pub(crate) reasoning: ReasoningLevel,
    pub(crate) queued_model: Option<SessionModel>,
    pub(crate) todos: Vec<TodoInfo>,
    pub(crate) jobs: Vec<JobInfo>,
    pub(crate) turn_points: Vec<TurnPoint>,
    pub(crate) pinned_turns: Vec<String>,
    pub(crate) plans: Vec<PlanSummary>,
    pub(crate) children: Vec<ChildSessionSummary>,
    pub(crate) telemetry: Option<TelemetryReport>,
    pub(crate) telemetry_scope: TelemetryScope,
    pub(crate) costs: Option<CostReport>,
    pub(crate) session_costs: Option<CostReport>,
    pub(crate) plugins: Vec<PluginSummary>,
    pub(crate) models: Vec<ModelInfo>,
    pub(crate) catalog: CatalogDetail,
}
