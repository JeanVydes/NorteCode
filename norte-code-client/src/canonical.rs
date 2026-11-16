use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CanonicalKind {
    SessionCreated,
    SessionList,
    SessionDeleted,
    SessionRenamed,
    SessionForked,
    TurnStarted,
    TokenDelta,
    Committed,
    HistoryMessage,
    SessionTodos,
    PlanList,
    QuestionAsked,
    ToolCallStarted,
    JobProgressDelta,
    ToolCallFinished,
    OutputPartFinished,
    RequestCompleted,
    ApprovalResolved,
    UserInputAccepted,
    ProviderRetry,
    AgentChanged,
    AgentCatalog,
    SessionStats,
    JobList,
    TurnStateChanged,
    ContextUpdate,
    BudgetWarning,
    BudgetExceeded,
    BudgetChanged,
    CostUpdated,
    CostNotReported,
    PluginList,
    PluginChanged,
    ServiceData,
    ChildSessionStarted,
    ChildSessionUpdated,
    DelegatorStateChanged,
    ChildAwaitingInput,
    Error,
    Other,
}

impl CanonicalKind {
    pub(crate) fn from_wire(name: &str) -> Self {
        match name {
            "session_created" => Self::SessionCreated,
            "session_list" => Self::SessionList,
            "session_deleted" => Self::SessionDeleted,
            "session_renamed" => Self::SessionRenamed,
            "session_forked" => Self::SessionForked,
            "turn_started" => Self::TurnStarted,
            "token_delta" => Self::TokenDelta,
            "committed" => Self::Committed,
            "history_message" => Self::HistoryMessage,
            "session_todos" => Self::SessionTodos,
            "plan_list" => Self::PlanList,
            "question_asked" => Self::QuestionAsked,
            "tool_call_started" => Self::ToolCallStarted,
            "job_progress_delta" => Self::JobProgressDelta,
            "tool_call_finished" => Self::ToolCallFinished,
            "output_part_finished" => Self::OutputPartFinished,
            "request_completed" => Self::RequestCompleted,
            "approval_resolved" => Self::ApprovalResolved,
            "user_input_accepted" => Self::UserInputAccepted,
            "provider_retry" => Self::ProviderRetry,
            "agent_changed" => Self::AgentChanged,
            "agent_catalog" => Self::AgentCatalog,
            "session_stats" => Self::SessionStats,
            "job_list" => Self::JobList,
            "turn_state_changed" => Self::TurnStateChanged,
            "context_update" => Self::ContextUpdate,
            "budget_warning" => Self::BudgetWarning,
            "budget_exceeded" => Self::BudgetExceeded,
            "budget_changed" => Self::BudgetChanged,
            "cost_updated" => Self::CostUpdated,
            "cost_not_reported" => Self::CostNotReported,
            "plugin_list" => Self::PluginList,
            "plugin_changed" => Self::PluginChanged,
            "service_data" => Self::ServiceData,
            "child_session_started" => Self::ChildSessionStarted,
            "child_session_updated" => Self::ChildSessionUpdated,
            "delegator_state_changed" => Self::DelegatorStateChanged,
            "child_awaiting_input" => Self::ChildAwaitingInput,
            "error" => Self::Error,
            _ => Self::Other,
        }
    }
}

use crate::error::ClientError;
use norte_code_wire::{AgentChange, CatalogKind, ServiceDataKind};

/// How a turn state changed, in the canonical payload's spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CanonicalTurnState {
    Finished,
    Stopped,
}

/// How a context (compaction) update went, in the canonical spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CanonicalContextState {
    Started,
    Finished,
}

/// How a provider retry is going, in the canonical spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetryState {
    Scheduled,
    Started,
}

/// A canonical payload, decoded once into the pieces every arm needs.
///
/// The tag is an enum, the fields are read by serde into typed structs per
/// arm, and there is no `payload.get("...")` anywhere: a renamed wire field
/// is a serde error naming the field.
#[derive(Debug)]
pub(crate) struct CanonicalPayload {
    pub(crate) kind: CanonicalKind,
    pub(crate) value: Value,
}

impl CanonicalPayload {
    pub(crate) fn from_value(payload: Value) -> Result<Self, ClientError> {
        let kind = payload
            .get("event")
            .and_then(Value::as_str)
            .map(CanonicalKind::from_wire)
            .ok_or_else(|| ClientError::new("RAEP canonical payload omitted event"))?;
        Ok(Self {
            kind,
            value: payload,
        })
    }

    /// Decodes the whole payload into `T`.
    pub(crate) fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, ClientError> {
        serde_json::from_value(self.value.clone())
            .map_err(|error| ClientError::new(format!("invalid RAEP canonical payload: {error}")))
    }
}

/// A canonical `session_created` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalSessionCreated {
    #[serde(default)]
    pub(crate) title: Option<String>,
}

/// A canonical `session_list` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalSessionList {
    #[serde(default)]
    pub(crate) sessions: Vec<norte_code_wire::SessionInfo>,
}

/// A canonical `session_forked` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalSessionForked {
    #[serde(rename = "from_session_id", alias = "from")]
    pub(crate) from: String,
    #[serde(default)]
    pub(crate) from_turn_id: Option<String>,
    #[serde(default)]
    pub(crate) title: Option<String>,
}

/// A canonical `turn_started` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalTurnStarted {
    pub(crate) turn: u64,
}

/// A canonical `token_delta` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalTokenDelta {
    pub(crate) turn: u64,
    pub(crate) delta: norte_code_wire::Delta,
}

/// A canonical `committed` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalCommitted {
    pub(crate) turn: u64,
    pub(crate) version: u64,
}

/// A canonical `history_message` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalHistoryMessage {
    pub(crate) index: u64,
    pub(crate) role: norte_code_wire::MessageRole,
    pub(crate) kind: norte_code_wire::ReplayedKind,
    pub(crate) text: String,
}

/// A canonical `session_todos` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalSessionTodos {
    pub(crate) todos: Vec<norte_code_wire::TodoInfo>,
}

/// A canonical `plan_list` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalPlanList {
    #[serde(default)]
    pub(crate) plans: Vec<norte_code_wire::PlanInfo>,
}

/// A canonical `question_asked` payload (`ask_user`).
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalQuestionAsked {
    #[serde(default)]
    pub(crate) turn: u64,
    pub(crate) question: String,
    #[serde(default)]
    pub(crate) prompt: String,
    #[serde(default)]
    pub(crate) options: Vec<String>,
    #[serde(default)]
    pub(crate) multiple: bool,
}

/// A canonical `tool_call_started` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalToolCallStarted {
    pub(crate) turn: u64,
    pub(crate) call_id: String,
    pub(crate) tool: String,
    #[serde(default)]
    pub(crate) arguments: Option<Value>,
    #[serde(default)]
    pub(crate) source: Option<String>,
}

/// A canonical `job_progress_delta` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalJobProgressDelta {
    pub(crate) turn: u64,
    pub(crate) call_id: String,
    pub(crate) tool: String,
    pub(crate) stream: norte_code_wire::OutputStream,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) last: bool,
}

/// A canonical `tool_call_finished` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalToolCallFinished {
    pub(crate) turn: u64,
    pub(crate) call_id: String,
    pub(crate) tool: String,
    pub(crate) is_error: bool,
    #[serde(default)]
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) output: Option<String>,
    pub(crate) truncated: bool,
    pub(crate) detached: bool,
    pub(crate) duration_ms: u64,
}

/// A canonical `output_part_finished` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalOutputPartFinished {
    pub(crate) turn: u64,
    pub(crate) part_id: String,
    pub(crate) index: u32,
    pub(crate) modality: norte_code_wire::OutputModality,
    #[serde(default)]
    pub(crate) bytes: u64,
    #[serde(default)]
    pub(crate) sha256: String,
    #[serde(default)]
    pub(crate) blob: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// A canonical `request_completed` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalRequestCompleted {
    pub(crate) turn: u64,
    pub(crate) index: u64,
    pub(crate) usage: norte_code_wire::Usage,
    #[serde(default)]
    pub(crate) cost_usd: Option<f64>,
    pub(crate) truncated: bool,
    pub(crate) duration_ms: u64,
    #[serde(default)]
    pub(crate) first_token_ms: Option<u64>,
    #[serde(default)]
    pub(crate) tokens_per_second: Option<f64>,
    pub(crate) cache_hit: bool,
    pub(crate) tools: Vec<String>,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
}

/// A canonical `approval_resolved` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalApprovalResolved {
    pub(crate) approval_id: String,
}

/// A canonical `user_input_accepted` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalUserInputAccepted {
    pub(crate) ids: Vec<String>,
    pub(crate) messages: Vec<String>,
}

/// A canonical `provider_retry` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalProviderRetry {
    pub(crate) state: RetryState,
    pub(crate) turn: u64,
    pub(crate) attempt: u32,
    #[serde(default)]
    pub(crate) delay_ms: u64,
    #[serde(default)]
    pub(crate) reason: String,
}

/// A canonical `agent_changed` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalAgentChanged {
    pub(crate) change: AgentChange,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) details: Option<Value>,
}

/// The `details` of a `reasoning_downgraded` change.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ReasoningDowngradedDetails {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) reason: String,
}

/// A canonical `agent_catalog` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalAgentCatalog {
    pub(crate) kind: CatalogKind,
    #[serde(default)]
    pub(crate) entries: Value,
}

/// The `entries` of a `skills` catalogue.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct SkillCatalogEntries {
    #[serde(default)]
    pub(crate) skills: Vec<norte_code_wire::SkillInfo>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
}

/// A canonical `session_renamed` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalSessionRenamed {
    pub(crate) title: String,
}

/// A canonical `session_stats` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalSessionStats {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_write_tokens: u64,
    pub(crate) cost_usd: f64,
    pub(crate) requests: u64,
    pub(crate) context_used: u64,
    #[serde(default)]
    pub(crate) context_window: Option<u64>,
}

/// A canonical `job_list` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalJobList {
    #[serde(default)]
    pub(crate) jobs: Vec<norte_code_wire::JobInfo>,
}

/// A canonical `turn_state_changed` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalTurnStateChanged {
    pub(crate) state: CanonicalTurnState,
    pub(crate) turn: u64,
    #[serde(default)]
    pub(crate) phase: Option<norte_code_wire::Phase>,
    #[serde(default)]
    pub(crate) committed: Option<bool>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

/// A canonical `context_update` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalContextUpdate {
    pub(crate) state: CanonicalContextState,
    #[serde(default)]
    pub(crate) reason: Option<norte_code_wire::CompactionReason>,
    #[serde(default)]
    pub(crate) compacted: Option<bool>,
    #[serde(default)]
    pub(crate) north_session: Option<String>,
    #[serde(default)]
    pub(crate) summary: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// A canonical `budget_warning` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalBudgetWarning {
    pub(crate) scope: norte_code_wire::BudgetScope,
    pub(crate) unit: norte_code_wire::BudgetUnit,
    pub(crate) spent: f64,
    pub(crate) limit: f64,
}

/// A canonical `budget_exceeded` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalBudgetExceeded {
    pub(crate) scope: norte_code_wire::BudgetScope,
    pub(crate) unit: norte_code_wire::BudgetUnit,
    pub(crate) spent: f64,
    pub(crate) limit: f64,
    pub(crate) action: norte_code_wire::BudgetAction,
}

/// A canonical `budget_changed` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalBudgetChanged {
    #[serde(default)]
    pub(crate) budget: Option<norte_code_wire::Budget>,
}

/// A canonical `cost_updated` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalCostUpdated {
    pub(crate) total_usd: f64,
    pub(crate) requests: u64,
}

/// A canonical `cost_not_reported` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalCostNotReported {
    pub(crate) requests: u64,
}

/// A canonical `plugin_list` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalPluginList {
    pub(crate) plugins: Vec<norte_code_wire::PluginInfo>,
}

/// A canonical `plugin_changed` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalPluginChanged {
    pub(crate) operation: String,
    pub(crate) name: String,
}

/// A canonical `service_data` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalServiceData {
    pub(crate) kind: ServiceDataKind,
    pub(crate) data: Value,
}

/// A canonical `child_session_started` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalChildSessionStarted {
    pub(crate) child_session_id: String,
    #[serde(default)]
    pub(crate) agent_id: String,
    #[serde(default)]
    pub(crate) tool_call_id: String,
    #[serde(default)]
    pub(crate) lifetime: String,
    #[serde(default)]
    pub(crate) task_summary: String,
}

/// A canonical `child_session_updated` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalChildSessionUpdated {
    pub(crate) child_session_id: String,
    pub(crate) state: norte_code_wire::ChildState,
    #[serde(default)]
    pub(crate) reason: String,
    #[serde(default)]
    pub(crate) result: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) output_schema_valid: Option<bool>,
}

/// A canonical `delegator_state_changed` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalDelegatorStateChanged {
    pub(crate) child_session_id: String,
    #[serde(default)]
    pub(crate) delegator_suspended: bool,
    #[serde(default)]
    pub(crate) by: Option<String>,
    #[serde(default)]
    pub(crate) queued_messages: Option<usize>,
}

/// A canonical `child_awaiting_input` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalChildAwaitingInput {
    pub(crate) child_session_id: String,
    #[serde(default)]
    pub(crate) question: String,
    #[serde(default)]
    pub(crate) prompt: String,
}

/// The `data` of an `mcp_list` service payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct McpListData {
    #[serde(default)]
    pub(crate) servers: Vec<norte_code_wire::McpInfo>,
    #[serde(default)]
    pub(crate) failures: Vec<norte_code_wire::McpFailure>,
}

/// A canonical `error` payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalError {
    #[serde(default)]
    pub(crate) code: Value,
    #[serde(default)]
    pub(crate) message: String,
}
