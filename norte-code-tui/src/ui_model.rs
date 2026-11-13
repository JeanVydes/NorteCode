use norte_code_client::{
    CatalogDetail, CostReport, Liveness, McpCatalog, PluginSummary, Readiness, ServerInfo,
    SkillCatalog, ToolRule,
};
use norte_code_wire::{
    AgentSummary, BudgetAction, BudgetScope, BudgetUnit, ChildSessionSummary, JobInfo, ModelInfo,
    PlanSummary, QueueMode, QuotaReport, ServerEvent, ServerEventKind, SessionKey, SessionSummary,
    TelemetryReport, TodoInfo, TurnPhaseName,
};
use serde::{Deserialize, Serialize};

use crate::tools::DiffMode;
pub(crate) enum TranscriptItem {
    UserMessage {
        text: String,
        chips: Vec<crate::chips::Chip>,
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
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) cost_usd: Option<f64>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) tokens_per_second: Option<f64>,
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

impl ReasoningLevel {
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Automatic => Self::Disabled,
            Self::Disabled => Self::Low,
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Automatic,
        }
    }

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
    pub(crate) budget_approval: Option<BudgetApprovalRequest>,
    pub(crate) notice: Option<String>,
    pub(crate) transcript: Vec<TranscriptItem>,
    pub(crate) turn_state: TurnState,
    pub(crate) queue_mode: QueueMode,
    pub(crate) budget_action: BudgetAction,
    pub(crate) active_turn: Option<String>,
    pub(crate) session_model: Option<SessionModel>,
    pub(crate) reasoning: ReasoningLevel,
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
