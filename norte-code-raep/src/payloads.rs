//! Canonical RAEP payloads emitted by the `NorteCode` adapter.

use norte_code_protocol::{Delta, JobInfo, OutputStream, ServerEventKind, TodoInfo};
use raep_types::{ErrorPayload, HistoryItem, HistoryPage, StopReason, TurnState, TurnStateChanged};
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

/// Serializes a typed RAEP payload. The payloads are closed structs, so this
/// cannot fail; the helper exists so call sites never build wire JSON by hand.
///
/// The return is `Value` only because the envelope's payload slot is the
/// spec's generic `P` — every caller passes a closed struct, and the client
/// decodes it back into the matching struct. No payload is built as `Value`.
pub(crate) fn payload<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("typed RAEP payload is serializable")
}

/// A token delta frame. RAEP has no struct for this, so the adapter owns one
/// here and serializes it; the shape is the wire contract, not a literal.
#[derive(Debug, Serialize)]
struct TokenDeltaPayload {
    delta: String,
    index: u32,
    channel: TokenChannel,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum TokenChannel {
    Text,
    Reasoning,
}

/// A tool call start frame.
#[derive(Debug, Serialize)]
struct ToolCallStartedPayload<'a> {
    tool_call_id: &'a str,
    tool_name: &'a str,
    arguments: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,
    requires_approval: bool,
    executor: ToolExecutor,
    attempt: u32,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum ToolExecutor {
    Server,
}

/// A tool call finish frame.
#[derive(Debug, Serialize)]
struct ToolCallFinishedPayload<'a> {
    tool_call_id: &'a str,
    tool_name: &'a str,
    status: ToolCallStatus,
    duration_ms: u64,
    result_summary: String,
    result_truncated: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
enum ToolCallStatus {
    #[serde(rename = "OK")]
    Ok,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "CANCELLED")]
    Cancelled,
}

/// A tool output delta frame.
#[derive(Debug, Serialize)]
struct ToolOutputDeltaPayload<'a> {
    tool_call_id: &'a str,
    tool_name: &'a str,
    stream: &'a norte_code_protocol::types::OutputStream,
    delta: &'a str,
    index: u32,
}

/// One job in a job list frame.
#[derive(Debug, Serialize)]
struct JobEntryPayload {
    job_id: String,
    command: String,
    status: super::host::JobStatus,
}

/// A job list frame.
#[derive(Debug, Serialize)]
struct JobListPayload {
    jobs: Vec<JobEntryPayload>,
}

/// A detached tool call became a job.
#[derive(Debug, Serialize)]
struct JobStartedPayload<'a> {
    job_id: &'a str,
    tool_call_id: &'a str,
    command: &'a str,
    detached: bool,
    started_at: u64,
    limits: &'a norte_code_protocol::JobLimits,
}

/// One output chunk from a detached job.
#[derive(Debug, Serialize)]
struct JobProgressDeltaPayload<'a> {
    stream: &'a OutputStream,
    delta: &'a str,
    index: u64,
}

/// How a turn ended, in the canonical payload's spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CanonicalTurnState {
    Finished,
    Stopped,
}

/// The canonical `turn_state_changed` state block.
#[derive(Debug, Serialize)]
struct TurnStateChangedPayload {
    state: CanonicalTurnState,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<norte_code_protocol::Phase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// What kind of agent change a canonical payload describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AgentChange {
    ModelChanged,
    ModelQueued,
    ReasoningDowngraded,
    CatalogUpdated,
    SkillsUpdated,
}

/// The `details` of a `reasoning_downgraded` change.
#[derive(Debug, Serialize)]
struct ReasoningDowngradedDetails {
    from: String,
    to: String,
    reason: String,
}

/// The `details` of a `catalog_updated` change.
#[derive(Debug, Serialize)]
struct CatalogUpdatedDetails {
    models: usize,
}

/// The `details` of a `skills_updated` change.
#[derive(Debug, Serialize)]
struct SkillsUpdatedDetails {
    skills: usize,
}

/// What kind of catalogue a canonical payload carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CatalogKind {
    Models,
    Skills,
}

/// The `entries` of a `skills` catalogue.
#[derive(Debug, Serialize)]
struct SkillCatalogEntries {
    skills: Vec<norte_code_protocol::SkillInfo>,
    warnings: Vec<String>,
}

/// What kind of service data a canonical payload carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ServiceDataKind {
    Telemetry,
    ToolList,
    McpList,
}

/// The `data` of an `mcp_list` service payload.
#[derive(Debug, Serialize)]
struct McpListData {
    servers: Vec<norte_code_protocol::McpInfo>,
    failures: Vec<norte_code_protocol::McpFailure>,
}

/// The `content` of a replayed history item.
#[derive(Debug, Serialize)]
struct HistoryContent<'a> {
    role: HistorySpeaker,
    content: &'a str,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum HistorySpeaker {
    Tool,
    SystemEvent,
    User,
    Assistant,
}

/// Who originated an accepted input, in the standard payload's spelling.
///
/// RAEP says `kind` is required and the rest is only meaningful for the
/// kind that names it; `delegator` and `system` can reach the CLI from a
/// server (a steering message from a parent agent, a server-generated
/// turn), so the struct has to spell them even though Norte Code never
/// sends them.
#[derive(Debug, Serialize)]
struct OriginPayload<'a> {
    kind: OriginKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum OriginKind {
    User,
}

/// One part of the accepted content.
#[derive(Debug, Serialize)]
struct AcceptedPart<'a> {
    #[serde(rename = "type")]
    kind: AcceptedContentKind,
    text: &'a str,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum AcceptedContentKind {
    Text,
}

/// A standard `USER_INPUT_ACCEPTED` payload (RFC 12.4): `turn_id`, `origin`,
/// `content`, plus `position` when the input waits in the queue and
/// `replaces_turn_id` when it re-runs a turn.
#[derive(Debug, Serialize)]
struct UserInputAcceptedPayload<'a> {
    turn_id: &'a str,
    origin: OriginPayload<'a>,
    content: Vec<AcceptedPart<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replaces_turn_id: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct BudgetNoticePayload {
    scope: RaepBudgetScope,
    metric: RaepBudgetUnit,
    used: f64,
    limit: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<RaepBudgetAction>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum RaepBudgetScope {
    Message,
    Session,
    #[serde(rename = "globaldaily")]
    GlobalDaily,
}

impl From<norte_code_protocol::BudgetScope> for RaepBudgetScope {
    fn from(value: norte_code_protocol::BudgetScope) -> Self {
        match value {
            norte_code_protocol::BudgetScope::Message => Self::Message,
            norte_code_protocol::BudgetScope::Session => Self::Session,
            norte_code_protocol::BudgetScope::GlobalDaily => Self::GlobalDaily,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum RaepBudgetUnit {
    Usd,
    Tokens,
    Requests,
}

impl From<norte_code_protocol::BudgetUnit> for RaepBudgetUnit {
    fn from(value: norte_code_protocol::BudgetUnit) -> Self {
        match value {
            norte_code_protocol::BudgetUnit::Usd => Self::Usd,
            norte_code_protocol::BudgetUnit::Tokens => Self::Tokens,
            norte_code_protocol::BudgetUnit::Requests => Self::Requests,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum RaepBudgetAction {
    Warn,
    Ask,
    Stop,
}

impl From<norte_code_protocol::BudgetAction> for RaepBudgetAction {
    fn from(value: norte_code_protocol::BudgetAction) -> Self {
        match value {
            norte_code_protocol::BudgetAction::Warn => Self::Warn,
            norte_code_protocol::BudgetAction::Ask => Self::Ask,
            norte_code_protocol::BudgetAction::Stop => Self::Stop,
        }
    }
}

#[derive(Debug, Serialize)]
struct RetryStartedPayload {
    turn_id: u64,
    attempt: u32,
}

#[derive(Debug, Serialize)]
struct RetryScheduledPayload<'a> {
    turn_id: u64,
    attempt: u32,
    delay_ms: u64,
    reason: &'a str,
}

#[derive(Debug, Serialize)]
struct CompactionFinishedPayload<'a> {
    trigger: CompactionTrigger,
    summary: &'a Option<String>,
    error: &'a Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum CompactionTrigger {
    Auto,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum CompactionState {
    Started,
    Finished,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum ProviderRetryState {
    Scheduled,
    Started,
}

#[derive(Debug, Serialize)]
struct SessionStatsPayload {
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    requests: u64,
}

#[derive(Debug, Serialize)]
struct QuestionAskedPayload<'a> {
    turn_id: u64,
    question: &'a str,
    prompt: &'a str,
    options: &'a [String],
    multiple: bool,
}

#[derive(Debug, Serialize)]
struct QuestionAnsweredPayload<'a> {
    question: &'a str,
}

#[derive(Debug, Serialize)]
struct SessionTodosPayload<'a> {
    todos: &'a [TodoInfo],
}

#[derive(Debug, Serialize)]
struct AgentReferencePayload<'a> {
    id: &'a str,
    version: &'a str,
}

#[derive(Debug, Serialize)]
struct AgentChangedPayload<'a> {
    from_agent: AgentReferencePayload<'a>,
    to_agent: AgentReferencePayload<'a>,
}

#[derive(Debug, Serialize)]
struct ReasoningDowngradedPayload<'a> {
    from: &'a str,
    to: &'a str,
    reason: &'a str,
}

#[derive(Debug, Serialize)]
struct JobsStoppedPayload {
    stopped: usize,
}

#[derive(Debug, Serialize)]
struct ChildSessionStartedPayload<'a> {
    child_session_id: &'a str,
    agent_id: &'a str,
    tool_call_id: &'a str,
    lifetime: &'a str,
    task_summary: &'a str,
}

#[derive(Debug, Serialize)]
struct ChildSessionUpdatedPayload<'a> {
    child_session_id: &'a str,
    state: &'a str,
    reason: &'a str,
    result: Option<&'a Value>,
    output_schema_valid: Option<&'a bool>,
}

#[derive(Debug, Serialize)]
struct DelegatorStateChangedPayload<'a> {
    child_session_id: &'a str,
    suspended: bool,
    by: &'a Option<String>,
    queued_messages: &'a Option<usize>,
}

#[derive(Debug, Serialize)]
struct ChildAwaitingInputPayload<'a> {
    child_session_id: &'a str,
    question: &'a str,
    prompt: &'a str,
}

/// Converts a `NorteCode` event into the stable payload understood by RAEP
/// clients.
///
/// The complete `NorteCode` event remains available in the envelope extension.
/// That keeps the adapter lossless while preventing clients from depending on
/// `NorteCode`'s request/session wrapper for the common event families.
#[allow(clippy::too_many_lines)]
pub(crate) fn canonical_event_payload(kind: &ServerEventKind) -> Value {
    let payload = match kind {
        ServerEventKind::SessionCreated { title } => CanonicalEvent::SessionCreated { title },
        ServerEventKind::SessionList { sessions } => CanonicalEvent::SessionList {
            sessions: serde_json::to_value(sessions).expect("session list is serializable"),
        },
        ServerEventKind::SessionDeleted => CanonicalEvent::SessionDeleted,
        ServerEventKind::SessionRenamed { title } => CanonicalEvent::SessionRenamed { title },
        ServerEventKind::SessionForked {
            from,
            from_turn_id,
            title,
        } => CanonicalEvent::SessionForked {
            from_session_id: from.as_str(),
            from_turn_id: from_turn_id.as_deref(),
            title: title.as_deref(),
        },
        ServerEventKind::TurnStarted { turn } => CanonicalEvent::TurnStarted { turn: *turn },
        ServerEventKind::Delta { turn, delta } => CanonicalEvent::TokenDelta {
            turn: *turn,
            delta: serde_json::to_value(delta).expect("protocol delta is serializable"),
        },
        ServerEventKind::ToolStarted {
            turn,
            call_id,
            tool,
            arguments,
            source,
        } => CanonicalEvent::ToolCallStarted {
            turn: *turn,
            call_id,
            tool,
            arguments,
            source,
        },
        ServerEventKind::ToolOutput {
            turn,
            call_id,
            tool,
            stream,
            text,
            last,
        } => CanonicalEvent::JobProgressDelta {
            turn: *turn,
            call_id,
            tool,
            stream: serde_json::to_value(stream).expect("output stream is serializable"),
            text,
            last: *last,
        },
        ServerEventKind::ToolFinished {
            turn,
            call_id,
            tool,
            is_error,
            error,
            output,
            truncated,
            detached,
            duration_ms,
        } => CanonicalEvent::ToolCallFinished {
            turn: *turn,
            call_id,
            tool,
            is_error: *is_error,
            error,
            output,
            truncated: *truncated,
            detached: *detached,
            duration_ms: *duration_ms,
        },
        ServerEventKind::JobList { jobs } => CanonicalEvent::JobList {
            jobs: serde_json::to_value(jobs).expect("job list is serializable"),
        },
        ServerEventKind::JobsStopped { stopped } => {
            CanonicalEvent::JobsStopped { stopped: *stopped }
        }
        ServerEventKind::JobStarted {
            job_id,
            tool_call_id,
            command,
            started_at,
            limits,
        } => CanonicalEvent::JobStarted {
            job_id,
            tool_call_id,
            command,
            started_at: *started_at,
            limits,
        },
        ServerEventKind::JobProgressDelta {
            job_id,
            stream,
            delta,
            index,
        } => CanonicalEvent::DetachedJobProgress {
            job_id,
            stream: serde_json::to_value(stream).expect("job output stream is serializable"),
            delta,
            index: *index,
        },
        ServerEventKind::JobFinished {
            job,
            triggered_new_turn,
            new_turn_id,
            injected_into_turn_id,
        } => CanonicalEvent::JobFinished {
            job_id: &job.id,
            job,
            triggered_new_turn: *triggered_new_turn,
            new_turn_id: (*new_turn_id).map(|turn| turn.to_string()),
            injected_into_turn_id: (*injected_into_turn_id).map(|turn| turn.to_string()),
        },
        ServerEventKind::QuestionAsked {
            turn,
            question,
            prompt,
            options,
            multiple,
        } => CanonicalEvent::QuestionAsked {
            turn: *turn,
            question,
            prompt,
            options,
            multiple: *multiple,
        },
        ServerEventKind::QuestionAnswered { question } => CanonicalEvent::ApprovalResolved {
            approval_id: question,
        },
        ServerEventKind::TurnFinished {
            turn,
            phase,
            committed,
        } => CanonicalEvent::TurnStateChanged {
            turn: Some(*turn),
            state: TurnStateChangedPayload {
                state: CanonicalTurnState::Finished,
                phase: Some(*phase),
                committed: Some(*committed),
                reason: None,
            },
        },
        ServerEventKind::TurnStopped { reason } => CanonicalEvent::TurnStateChanged {
            turn: None,
            state: TurnStateChangedPayload {
                state: CanonicalTurnState::Stopped,
                phase: None,
                committed: None,
                reason: Some(reason.clone()),
            },
        },
        ServerEventKind::RequestReconstructed {
            turn,
            index,
            usage,
            cost_usd,
            truncated,
            duration_ms,
            first_token_ms,
            tokens_per_second,
            cache_hit,
            tools,
        } => CanonicalEvent::OutputPartFinished {
            turn: *turn,
            index: *index,
            usage: serde_json::to_value(usage).expect("usage is serializable"),
            cost_usd: *cost_usd,
            truncated: *truncated,
            duration_ms: *duration_ms,
            first_token_ms: *first_token_ms,
            tokens_per_second: *tokens_per_second,
            cache_hit: *cache_hit,
            tools,
        },
        ServerEventKind::Committed { turn, version } => CanonicalEvent::Committed {
            turn: *turn,
            version: *version,
        },
        ServerEventKind::MessageReplayed {
            index,
            role,
            kind,
            text,
        } => CanonicalEvent::HistoryMessage {
            index: *index,
            role: *role,
            kind: *kind,
            text,
        },
        ServerEventKind::SessionTodos { todos } => CanonicalEvent::SessionTodos { todos },
        ServerEventKind::CompactionStarted { reason } => CanonicalEvent::ContextUpdate {
            state: CompactionState::Started,
            reason: Some(serde_json::to_value(reason).expect("compaction reason is serializable")),
            compacted: None,
            summary: None,
            error: None,
            north_session: None,
        },
        ServerEventKind::CompactionFinished {
            compacted,
            north_session,
            summary,
            error,
        } => CanonicalEvent::ContextUpdate {
            state: CompactionState::Finished,
            reason: None,
            compacted: Some(*compacted),
            summary: summary.as_deref(),
            error: error.as_deref(),
            north_session: north_session.as_deref(),
        },
        ServerEventKind::BudgetWarning {
            scope,
            unit,
            spent,
            limit,
        } => CanonicalEvent::BudgetWarning {
            scope: (*scope).into(),
            unit: (*unit).into(),
            spent: *spent,
            limit: *limit,
        },
        ServerEventKind::BudgetExceeded {
            scope,
            unit,
            spent,
            limit,
            action,
        } => CanonicalEvent::BudgetExceeded {
            scope: (*scope).into(),
            unit: (*unit).into(),
            spent: *spent,
            limit: *limit,
            action: (*action).into(),
        },
        ServerEventKind::BudgetChanged { budget } => CanonicalEvent::BudgetChanged {
            budget: budget.as_ref(),
        },
        ServerEventKind::SessionStats {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cost_usd,
            requests,
            context_used,
            context_window,
        } => CanonicalEvent::SessionStats {
            input_tokens: *input_tokens,
            output_tokens: *output_tokens,
            cache_read_tokens: *cache_read_tokens,
            cache_write_tokens: *cache_write_tokens,
            cost_usd: *cost_usd,
            requests: *requests,
            context_used: *context_used,
            context_window: *context_window,
        },
        ServerEventKind::CostUpdated {
            total_usd,
            requests,
        } => CanonicalEvent::CostUpdated {
            total_usd: *total_usd,
            requests: *requests,
        },
        ServerEventKind::CostNotReported { requests } => CanonicalEvent::CostNotReported {
            requests: *requests,
        },
        ServerEventKind::PluginList { plugins } => CanonicalEvent::PluginList {
            plugins: serde_json::to_value(plugins).expect("plugin list is serializable"),
        },
        ServerEventKind::PluginChanged { operation, name } => {
            CanonicalEvent::PluginChanged { operation, name }
        }
        ServerEventKind::SteerApplied { ids, messages } => {
            CanonicalEvent::UserInputAccepted { ids, messages }
        }
        ServerEventKind::ModelChanged { provider, model } => CanonicalEvent::AgentChanged {
            change: AgentChange::ModelChanged,
            provider: Some(provider),
            model: Some(model),
            details: None,
        },
        ServerEventKind::ModelQueued { provider, model } => CanonicalEvent::AgentChanged {
            change: AgentChange::ModelQueued,
            provider: Some(provider),
            model: Some(model),
            details: None,
        },
        ServerEventKind::ReasoningDowngraded { from, to, reason } => CanonicalEvent::AgentChanged {
            change: AgentChange::ReasoningDowngraded,
            provider: None,
            model: None,
            details: Some(payload(ReasoningDowngradedDetails {
                from: from.clone(),
                to: to.clone(),
                reason: reason.clone(),
            })),
        },
        ServerEventKind::CatalogUpdated { models } => CanonicalEvent::AgentChanged {
            change: AgentChange::CatalogUpdated,
            provider: None,
            model: None,
            details: Some(payload(CatalogUpdatedDetails { models: *models })),
        },
        ServerEventKind::ModelList { models } => CanonicalEvent::AgentCatalog {
            kind: CatalogKind::Models,
            entries: serde_json::to_value(models).expect("model list is serializable"),
        },
        ServerEventKind::SkillList { skills, warnings } => CanonicalEvent::AgentCatalog {
            kind: CatalogKind::Skills,
            entries: payload(SkillCatalogEntries {
                skills: skills.clone(),
                warnings: warnings.clone(),
            }),
        },
        ServerEventKind::SkillsUpdated { skills } => CanonicalEvent::AgentChanged {
            change: AgentChange::SkillsUpdated,
            provider: None,
            model: None,
            details: Some(payload(SkillsUpdatedDetails { skills: *skills })),
        },
        ServerEventKind::RetryScheduled {
            turn,
            attempt,
            delay_ms,
            reason,
        } => CanonicalEvent::ProviderRetry {
            turn: *turn,
            state: ProviderRetryState::Scheduled,
            attempt: *attempt,
            delay_ms: Some(*delay_ms),
            reason: Some(reason),
        },
        ServerEventKind::RetryStarted { turn, attempt } => CanonicalEvent::ProviderRetry {
            turn: *turn,
            state: ProviderRetryState::Started,
            attempt: *attempt,
            delay_ms: None,
            reason: None,
        },
        ServerEventKind::Error { code, message } => CanonicalEvent::Error { code, message },
        ServerEventKind::Telemetry { records } => CanonicalEvent::ServiceData {
            kind: ServiceDataKind::Telemetry,
            data: serde_json::to_value(records).expect("telemetry is serializable"),
        },
        ServerEventKind::ToolList { tools } => CanonicalEvent::ServiceData {
            kind: ServiceDataKind::ToolList,
            data: serde_json::to_value(tools).expect("tool list is serializable"),
        },
        ServerEventKind::McpList { servers, failures } => CanonicalEvent::ServiceData {
            kind: ServiceDataKind::McpList,
            data: payload(McpListData {
                servers: servers.clone(),
                failures: failures.clone(),
            }),
        },
        ServerEventKind::SessionExportReady { export_id, bundle } => {
            CanonicalEvent::SessionExportReady {
                export_id,
                bundle: serde_json::to_value(bundle).expect("export bundle is serializable"),
            }
        }
        ServerEventKind::ImportResult { session_id, id_map } => CanonicalEvent::ImportResult {
            session_id,
            id_map: serde_json::to_value(id_map).expect("id map is serializable"),
        },
        ServerEventKind::ChildSessionStarted {
            child_session_id,
            agent_id,
            tool_call_id,
            lifetime,
            task_summary,
        } => CanonicalEvent::ChildSessionStarted {
            child_session_id,
            agent_id,
            tool_call_id,
            lifetime,
            task_summary,
        },
        ServerEventKind::ChildSessionUpdated {
            child_session_id,
            state,
            reason,
            result,
            output_schema_valid,
        } => CanonicalEvent::ChildSessionUpdated {
            child_session_id,
            state,
            reason,
            result: result.as_ref(),
            output_schema_valid: output_schema_valid.unwrap_or(false),
        },
        ServerEventKind::DelegatorStateChanged {
            child_session_id,
            delegator_suspended,
            by,
            queued_messages,
        } => CanonicalEvent::DelegatorStateChanged {
            child_session_id,
            delegator_suspended: *delegator_suspended,
            by: by.as_deref().unwrap_or(""),
            queued_messages: queued_messages.unwrap_or(0),
        },
        ServerEventKind::ChildAwaitingInput {
            child_session_id,
            question,
            prompt,
        } => CanonicalEvent::ChildAwaitingInput {
            child_session_id,
            question,
            prompt,
        },
        ServerEventKind::MediaPartCompleted { turn, index, media } => {
            CanonicalEvent::MediaPartCompleted {
                turn: *turn,
                index: *index,
                media,
            }
        }
    };
    serde_json::to_value(payload).expect("canonical event is serializable")
}

/// Builds the standard RAEP payload for events that have a faithful standard
/// representation. Returns `None` for NorteCode-specific events; those keep
/// the adapter's canonical payload and are sent as extensions.
#[allow(clippy::too_many_lines)]
pub(crate) fn standard_event_payload(kind: &ServerEventKind) -> Option<Value> {
    match kind {
        ServerEventKind::TurnStarted { .. } => Some(payload(TurnStateChanged {
            state: TurnState::CallingProvider,
            stop_reason: None,
            usage: None,
            error: None,
        })),
        ServerEventKind::Delta { delta, .. } => {
            let (text, index, channel) = match delta {
                Delta::Text { index, text } | Delta::Refusal { index, text } => {
                    (text.clone(), *index, TokenChannel::Text)
                }
                Delta::Reasoning { index, text } => (text.clone(), *index, TokenChannel::Reasoning),
                Delta::ToolArguments {
                    index, arguments, ..
                } => (arguments.clone(), *index, TokenChannel::Text),
                Delta::Media { index, .. } => (String::new(), *index, TokenChannel::Text),
            };
            Some(payload(TokenDeltaPayload {
                delta: text,
                index,
                channel,
            }))
        }
        ServerEventKind::ToolStarted {
            call_id,
            tool,
            arguments,
            source,
            ..
        } => {
            let arguments = arguments
                .as_ref()
                .and_then(|arguments| arguments.as_object())
                .cloned()
                .unwrap_or_default();
            Some(payload(ToolCallStartedPayload {
                tool_call_id: call_id,
                tool_name: tool,
                arguments: Value::Object(arguments),
                source: source.as_deref(),
                requires_approval: false,
                executor: ToolExecutor::Server,
                attempt: 1,
            }))
        }
        ServerEventKind::ToolFinished {
            call_id,
            tool,
            is_error,
            detached,
            output,
            error,
            truncated,
            duration_ms,
            ..
        } => {
            let status = if *detached {
                ToolCallStatus::Cancelled
            } else if *is_error {
                ToolCallStatus::Error
            } else {
                ToolCallStatus::Ok
            };
            let summary = error.clone().or_else(|| output.clone()).unwrap_or_default();
            Some(payload(ToolCallFinishedPayload {
                tool_call_id: call_id,
                tool_name: tool,
                status,
                duration_ms: *duration_ms,
                result_summary: summary,
                result_truncated: *truncated,
            }))
        }
        ServerEventKind::ToolOutput {
            call_id,
            tool,
            stream,
            text,
            ..
        } => Some(payload(ToolOutputDeltaPayload {
            tool_call_id: call_id,
            tool_name: tool,
            stream,
            delta: text,
            index: 0,
        })),
        ServerEventKind::JobStarted {
            job_id,
            tool_call_id,
            command,
            started_at,
            limits,
        } => Some(payload(JobStartedPayload {
            job_id,
            tool_call_id,
            command,
            detached: true,
            started_at: *started_at,
            limits,
        })),
        ServerEventKind::JobProgressDelta {
            stream,
            delta,
            index,
            ..
        } => Some(payload(JobProgressDeltaPayload {
            stream,
            delta,
            index: *index,
        })),
        ServerEventKind::JobFinished {
            job,
            triggered_new_turn,
            new_turn_id,
            injected_into_turn_id,
        } => {
            let new_turn_id = new_turn_id.map(|turn| turn.to_string());
            let injected_into_turn_id = injected_into_turn_id.map(|turn| turn.to_string());
            Some(super::host::job_finished_payload(
                job,
                *triggered_new_turn,
                new_turn_id.as_deref(),
                injected_into_turn_id.as_deref(),
            ))
        }
        // RAEP `TurnStateChanged` carries only `state` and `stop_reason`;
        // `phase`/`committed` are NorteCode's own and travel in the
        // compatibility extension (see `canonical_event_payload`), never
        // smuggled into the canonical payload.
        ServerEventKind::TurnFinished { .. } => Some(payload(TurnStateChanged {
            state: TurnState::Completed,
            stop_reason: Some(StopReason::EndTurn),
            usage: None,
            error: None,
        })),
        ServerEventKind::TurnStopped { reason } => Some(payload(TurnStateChanged {
            state: TurnState::Failed,
            stop_reason: Some(StopReason::Error),
            usage: None,
            error: Some(ErrorPayload {
                code: "INTERNAL".to_owned(),
                message: format!("turn stopped: {reason}"),
                retryable: Some(false),
                details: None,
            }),
        })),
        ServerEventKind::JobList { jobs } => {
            let jobs = jobs
                .iter()
                .map(|job| {
                    let command = if job.script.is_empty() {
                        job.label.clone()
                    } else {
                        job.script.clone()
                    };
                    JobEntryPayload {
                        job_id: job.id.clone(),
                        command,
                        status: super::host::job_status(job),
                    }
                })
                .collect::<Vec<_>>();
            Some(payload(JobListPayload { jobs }))
        }
        ServerEventKind::MessageReplayed {
            index,
            role,
            kind,
            text,
        } => {
            let role = match role {
                norte_code_protocol::MessageRole::Tool => HistorySpeaker::Tool,
                norte_code_protocol::MessageRole::System
                | norte_code_protocol::MessageRole::Developer
                | norte_code_protocol::MessageRole::Unknown => HistorySpeaker::SystemEvent,
                norte_code_protocol::MessageRole::User => HistorySpeaker::User,
                norte_code_protocol::MessageRole::Assistant => HistorySpeaker::Assistant,
            };
            let kind = match kind {
                norte_code_protocol::ReplayedKind::Text => match role {
                    HistorySpeaker::Tool => "tool",
                    HistorySpeaker::SystemEvent => "system_event",
                    HistorySpeaker::User => "user",
                    HistorySpeaker::Assistant => "assistant",
                },
                norte_code_protocol::ReplayedKind::Reasoning => "reasoning",
                norte_code_protocol::ReplayedKind::ToolCall => "tool_call",
                norte_code_protocol::ReplayedKind::ToolResult => "tool_result",
                norte_code_protocol::ReplayedKind::Refusal => "refusal",
            };
            Some(payload(HistoryPage {
                items: vec![HistoryItem {
                    id: index.to_string(),
                    kind: kind.to_owned(),
                    content: payload(HistoryContent {
                        role,
                        content: text,
                    }),
                    created_at: 0,
                }],
                next: None,
            }))
        }
        ServerEventKind::Error { code, message } => Some(payload(ErrorPayload {
            code: match serde_json::to_value(code)
                .and_then(|value| serde_json::from_value::<String>(value.clone()))
            {
                Ok(text) => text,
                Err(_) => "INTERNAL".to_owned(),
            },
            message: message.clone(),
            retryable: Some(false),
            details: None,
        })),
        ServerEventKind::SteerApplied { ids, messages } => {
            let mut content = Vec::with_capacity(messages.len());
            for text in messages {
                content.push(AcceptedPart {
                    kind: AcceptedContentKind::Text,
                    text,
                });
            }
            Some(payload(UserInputAcceptedPayload {
                turn_id: ids.first().map_or("", String::as_str),
                origin: OriginPayload {
                    kind: OriginKind::User,
                    user_id: None,
                    display_name: None,
                },
                content,
                position: None,
                replaces_turn_id: None,
            }))
        }
        // Notifications the spec asks for as events, mapped from the
        // service's own events. Output parts are emitted by the host after it
        // stores `MediaPartCompleted`; `ProviderRetry`, `QuotaWarning`,
        // `ServerDraining` and `Presence` have no service-side source yet.
        ServerEventKind::BudgetWarning {
            scope,
            unit,
            spent,
            limit,
        } => Some(payload(BudgetNoticePayload {
            scope: (*scope).into(),
            metric: (*unit).into(),
            used: *spent,
            limit: *limit,
            action: None,
        })),
        ServerEventKind::BudgetExceeded {
            scope,
            unit,
            spent,
            limit,
            action,
        } => Some(payload(BudgetNoticePayload {
            scope: (*scope).into(),
            metric: (*unit).into(),
            used: *spent,
            limit: *limit,
            action: Some((*action).into()),
        })),
        ServerEventKind::RetryStarted { turn, attempt } => Some(payload(RetryStartedPayload {
            turn_id: *turn,
            attempt: *attempt,
        })),
        ServerEventKind::RetryScheduled {
            turn,
            attempt,
            delay_ms,
            reason,
        } => Some(payload(RetryScheduledPayload {
            turn_id: *turn,
            attempt: *attempt,
            delay_ms: *delay_ms,
            reason,
        })),
        ServerEventKind::CompactionFinished {
            compacted,
            summary,
            error,
            ..
        } => {
            if *compacted {
                Some(payload(CompactionFinishedPayload {
                    trigger: CompactionTrigger::Auto,
                    summary,
                    error,
                }))
            } else {
                None
            }
        }
        ServerEventKind::SessionStats {
            input_tokens,
            output_tokens,
            cost_usd,
            requests,
            ..
        } => Some(payload(SessionStatsPayload {
            input_tokens: *input_tokens,
            output_tokens: *output_tokens,
            cost_usd: *cost_usd,
            requests: *requests,
        })),
        ServerEventKind::QuestionAsked {
            turn,
            question,
            prompt,
            options,
            multiple,
        } => Some(payload(QuestionAskedPayload {
            turn_id: *turn,
            question,
            prompt,
            options,
            multiple: *multiple,
        })),
        ServerEventKind::QuestionAnswered { question } => {
            Some(payload(QuestionAnsweredPayload { question }))
        }
        ServerEventKind::SessionTodos { todos } => Some(payload(SessionTodosPayload { todos })),
        ServerEventKind::ModelChanged { provider, model }
        | ServerEventKind::ModelQueued { provider, model } => Some(payload(AgentChangedPayload {
            from_agent: AgentReferencePayload {
                id: "",
                version: "",
            },
            to_agent: AgentReferencePayload {
                id: provider,
                version: model,
            },
        })),
        ServerEventKind::ReasoningDowngraded { from, to, reason } => {
            Some(payload(ReasoningDowngradedPayload { from, to, reason }))
        }
        ServerEventKind::JobsStopped { stopped } => {
            Some(payload(JobsStoppedPayload { stopped: *stopped }))
        }
        ServerEventKind::ChildSessionStarted {
            child_session_id,
            agent_id,
            tool_call_id,
            lifetime,
            task_summary,
        } => Some(payload(ChildSessionStartedPayload {
            child_session_id,
            agent_id,
            tool_call_id,
            lifetime,
            task_summary,
        })),
        ServerEventKind::ChildSessionUpdated {
            child_session_id,
            state,
            reason,
            result,
            output_schema_valid,
        } => Some(payload(ChildSessionUpdatedPayload {
            child_session_id,
            state,
            reason,
            result: result.as_ref(),
            output_schema_valid: output_schema_valid.as_ref(),
        })),
        ServerEventKind::DelegatorStateChanged {
            child_session_id,
            delegator_suspended,
            by,
            queued_messages,
        } => Some(payload(DelegatorStateChangedPayload {
            child_session_id,
            suspended: *delegator_suspended,
            by,
            queued_messages,
        })),
        ServerEventKind::ChildAwaitingInput {
            child_session_id,
            question,
            prompt,
        } => Some(payload(ChildAwaitingInputPayload {
            child_session_id,
            question,
            prompt,
        })),
        _ => None,
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum CanonicalEvent<'a> {
    /// Norte Code service-specific data exposed through the RAEP adapter.
    ServiceData { kind: ServiceDataKind, data: Value },
    /// The plugins installed on the remote host.
    PluginList { plugins: Value },
    /// A plugin lifecycle operation completed.
    PluginChanged { operation: &'a str, name: &'a str },
    /// A session was created.
    SessionCreated {
        #[serde(skip_serializing_if = "Option::is_none")]
        title: &'a Option<String>,
    },
    /// The current session list.
    SessionList { sessions: Value },
    /// A session was deleted.
    SessionDeleted,
    /// A session was renamed.
    SessionRenamed { title: &'a str },
    /// A session was forked.
    SessionForked {
        from_session_id: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_turn_id: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<&'a str>,
    },
    /// A turn began.
    TurnStarted { turn: u64 },
    /// A model token delta.
    TokenDelta { turn: u64, delta: Value },
    /// A committed turn.
    Committed { turn: u64, version: u64 },
    /// A replayed history message.
    HistoryMessage {
        index: u64,
        role: norte_code_protocol::MessageRole,
        kind: norte_code_protocol::ReplayedKind,
        text: &'a str,
    },
    /// The current session todos.
    SessionTodos { todos: &'a Vec<TodoInfo> },
    /// A tool call began.
    ToolCallStarted {
        turn: u64,
        call_id: &'a str,
        tool: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: &'a Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: &'a Option<String>,
    },
    /// A detached tool/job produced output.
    JobProgressDelta {
        turn: u64,
        call_id: &'a str,
        tool: &'a str,
        stream: Value,
        text: &'a str,
        last: bool,
    },
    /// A tool call ended.
    ToolCallFinished {
        turn: u64,
        call_id: &'a str,
        tool: &'a str,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: &'a Option<String>,
        truncated: bool,
        detached: bool,
        duration_ms: u64,
    },
    /// The jobs currently known by the session.
    JobList { jobs: Value },
    /// The result of stopping jobs.
    JobsStopped { stopped: usize },
    /// A detached tool call became a job.
    JobStarted {
        job_id: &'a str,
        tool_call_id: &'a str,
        command: &'a str,
        started_at: u64,
        limits: &'a norte_code_protocol::JobLimits,
    },
    /// A detached job wrote a chunk of output.
    DetachedJobProgress {
        job_id: &'a str,
        stream: Value,
        delta: &'a str,
        index: u64,
    },
    /// A background job reached a terminal state.
    JobFinished {
        job_id: &'a str,
        job: &'a JobInfo,
        triggered_new_turn: bool,
        new_turn_id: Option<String>,
        injected_into_turn_id: Option<String>,
    },
    /// A question requiring client input.
    QuestionAsked {
        turn: u64,
        question: &'a str,
        prompt: &'a str,
        options: &'a Vec<String>,
        multiple: bool,
    },
    /// An approval/question was resolved.
    ApprovalResolved { approval_id: &'a str },
    /// A turn changed lifecycle state.
    TurnStateChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        turn: Option<u64>,
        state: TurnStateChangedPayload,
    },
    /// Provider usage and cost for one output part.
    OutputPartFinished {
        turn: u64,
        index: u64,
        usage: Value,
        cost_usd: Option<f64>,
        truncated: bool,
        duration_ms: u64,
        first_token_ms: Option<u64>,
        tokens_per_second: Option<f64>,
        cache_hit: bool,
        tools: &'a Vec<String>,
    },
    /// Generated media collected by Norte Code for the RAEP output store.
    MediaPartCompleted {
        turn: u64,
        index: u32,
        media: &'a norte::MediaPart,
    },
    /// A context compaction lifecycle update.
    ContextUpdate {
        state: CompactionState,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        compacted: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        north_session: Option<&'a str>,
    },
    /// A budget warning.
    BudgetWarning {
        scope: RaepBudgetScope,
        unit: RaepBudgetUnit,
        spent: f64,
        limit: f64,
    },
    /// A budget was exhausted.
    BudgetExceeded {
        scope: RaepBudgetScope,
        unit: RaepBudgetUnit,
        spent: f64,
        limit: f64,
        action: RaepBudgetAction,
    },
    /// The session budget changed.
    BudgetChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        budget: Option<&'a norte_code_protocol::Budget>,
    },
    /// Session usage totals.
    SessionStats {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        cost_usd: f64,
        requests: u64,
        context_used: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    /// Running cost total changed.
    CostUpdated { total_usd: f64, requests: u64 },
    /// Provider did not report cost.
    CostNotReported { requests: u64 },
    /// Steering input was accepted.
    UserInputAccepted {
        ids: &'a Vec<String>,
        messages: &'a Vec<String>,
    },
    /// Agent configuration changed.
    AgentChanged {
        change: AgentChange,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    /// An agent catalogue was returned.
    AgentCatalog { kind: CatalogKind, entries: Value },
    /// A provider retry changed state.
    ProviderRetry {
        turn: u64,
        state: ProviderRetryState,
        attempt: u32,
        delay_ms: Option<u64>,
        reason: Option<&'a String>,
    },
    /// A protocol error.
    Error {
        code: &'a norte_code_protocol::ErrorCode,
        message: &'a str,
    },
    /// A session was exported.
    SessionExportReady {
        export_id: &'a String,
        bundle: Value,
    },
    /// A session was imported.
    ImportResult {
        session_id: &'a String,
        id_map: Value,
    },
    /// A child session was started.
    ChildSessionStarted {
        child_session_id: &'a String,
        agent_id: &'a String,
        tool_call_id: &'a String,
        lifetime: &'a String,
        task_summary: &'a String,
    },
    /// A child session changed state.
    ChildSessionUpdated {
        child_session_id: &'a String,
        state: &'a String,
        reason: &'a String,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<&'a Value>,
        output_schema_valid: bool,
    },
    /// Delegator state changed.
    DelegatorStateChanged {
        child_session_id: &'a String,
        delegator_suspended: bool,
        by: &'a str,
        queued_messages: usize,
    },
    /// A child is waiting for its delegator's answer.
    ChildAwaitingInput {
        child_session_id: &'a String,
        question: &'a String,
        prompt: &'a String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_output_uses_canonical_job_payload_and_preserves_wire_event() {
        let payload = canonical_event_payload(&ServerEventKind::ToolOutput {
            turn: 3,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: OutputStream::Stdout,
            text: "hello".to_owned(),
            last: true,
        });

        assert_eq!(payload["event"], "job_progress_delta");
        assert_eq!(payload["turn"], 3);
        assert_eq!(payload["call_id"], "call-1");
        assert_eq!(payload["stream"], "stdout");
        assert_eq!(payload["last"], true);
    }

    #[test]
    fn session_forked_uses_the_raep_turn_and_source_fields() {
        let payload = canonical_event_payload(&ServerEventKind::SessionForked {
            from: norte_code_protocol::SessionKey::new("source"),
            from_turn_id: Some("turn_4".to_owned()),
            title: Some("branch".to_owned()),
        });

        assert_eq!(
            payload,
            json!({
                "event": "session_forked",
                "from_session_id": "source",
                "from_turn_id": "turn_4",
                "title": "branch"
            })
        );
    }

    #[test]
    fn standard_payloads_match_the_raep_schemas() {
        let budget = standard_event_payload(&ServerEventKind::BudgetExceeded {
            scope: norte_code_protocol::BudgetScope::GlobalDaily,
            unit: norte_code_protocol::BudgetUnit::Usd,
            spent: 4.5,
            limit: 5.0,
            action: norte_code_protocol::BudgetAction::Stop,
        })
        .expect("standard budget payload");
        assert_eq!(
            budget,
            json!({
                "scope": "globaldaily",
                "metric": "usd",
                "used": 4.5,
                "limit": 5.0,
                "action": "stop"
            })
        );

        let tool_output = standard_event_payload(&ServerEventKind::ToolOutput {
            turn: 2,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: OutputStream::Stdout,
            text: "hello".to_owned(),
            last: false,
        })
        .expect("standard tool output");
        assert_eq!(
            tool_output,
            json!({
                "tool_call_id": "call-1",
                "tool_name": "shell",
                "stream": "stdout",
                "delta": "hello",
                "index": 0
            })
        );

        let history = standard_event_payload(&ServerEventKind::MessageReplayed {
            index: 1,
            role: norte_code_protocol::MessageRole::User,
            kind: norte_code_protocol::ReplayedKind::Text,
            text: "hi".to_owned(),
        })
        .expect("standard history page");
        assert_eq!(
            history,
            json!({
                "items": [{
                    "id": "1",
                    "kind": "user",
                    "content": {"role": "user", "content": "hi"},
                    "created_at": 0
                }]
            })
        );

        let child = standard_event_payload(&ServerEventKind::ChildSessionUpdated {
            child_session_id: "child-1".to_owned(),
            state: "IDLE".to_owned(),
            reason: "turn_finished".to_owned(),
            result: Some(json!({
                "status": "completed",
                "summary": r#"{"answer":42}"#,
                "output": {"answer": 42},
                "output_schema_valid": true,
            })),
            output_schema_valid: Some(true),
        })
        .expect("child update");
        assert_eq!(child["result"]["output"]["answer"], 42);
        assert_eq!(child["result"]["output_schema_valid"], true);
        assert_eq!(child["output_schema_valid"], true);

        let error = standard_event_payload(&ServerEventKind::Error {
            code: norte_code_protocol::ErrorCode::Internal,
            message: "boom".to_owned(),
        })
        .expect("standard error");
        assert_eq!(error["message"], "boom");
        assert_eq!(error["retryable"], false);
        assert!(error["code"].is_string());

        let jobs = standard_event_payload(&ServerEventKind::JobList {
            jobs: vec![norte_code_protocol::JobInfo {
                id: "job-1".to_owned(),
                label: "sleep 1".to_owned(),
                script: String::new(),
                state: norte_code_protocol::JobState::Running,
                exit_code: None,
                timed_out: false,
                elapsed_ms: 10,
                output: String::new(),
                dropped: false,
                output_bytes_total: 0,
            }],
        })
        .expect("standard job list");
        assert_eq!(
            jobs,
            json!({
                "jobs": [{"job_id": "job-1", "command": "sleep 1", "status": "RUNNING"}]
            })
        );
    }

    #[test]
    fn standard_token_and_tool_payloads_match_the_raep_schemas() {
        let delta = standard_event_payload(&ServerEventKind::Delta {
            turn: 2,
            delta: Delta::Text {
                index: 3,
                text: "hello".to_owned(),
            },
        })
        .expect("standard token delta");
        assert_eq!(
            delta,
            json!({"delta": "hello", "index": 3, "channel": "text"})
        );

        let finished = standard_event_payload(&ServerEventKind::TurnFinished {
            turn: 2,
            phase: norte_code_protocol::Phase::Idle,
            committed: true,
        })
        .expect("standard turn state");
        assert_eq!(
            finished,
            json!({"state": "COMPLETED", "stop_reason": "end_turn"})
        );
        assert_eq!(
            standard_event_payload(&ServerEventKind::TurnStarted { turn: 2 }).expect("turn start"),
            json!({"state": "CALLING_PROVIDER"})
        );
        let failed = standard_event_payload(&ServerEventKind::TurnStopped {
            reason: "provider_failure".to_owned(),
        })
        .expect("failed turn");
        assert_eq!(failed["state"], "FAILED");
        assert_eq!(failed["stop_reason"], "error");
        assert_eq!(failed["error"]["code"], "INTERNAL");

        let started = standard_event_payload(&ServerEventKind::ToolStarted {
            turn: 2,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            arguments: Some(json!({"command": "ls"})),
            source: None,
        })
        .expect("standard tool start");
        assert_eq!(
            started,
            json!({
                "tool_call_id": "call-1",
                "tool_name": "shell",
                "arguments": {"command": "ls"},
                "requires_approval": false,
                "executor": "server",
                "attempt": 1
            })
        );

        let tool_finished = standard_event_payload(&ServerEventKind::ToolFinished {
            turn: 2,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            is_error: false,
            error: None,
            output: Some("done".to_owned()),
            truncated: false,
            detached: false,
            duration_ms: 12,
        })
        .expect("standard tool finish");
        assert_eq!(
            tool_finished,
            json!({
                "tool_call_id": "call-1",
                "tool_name": "shell",
                "status": "OK",
                "duration_ms": 12,
                "result_summary": "done",
                "result_truncated": false
            })
        );
    }

    #[test]
    fn detached_job_lifecycle_payloads_match_the_raep_schemas() {
        let started = standard_event_payload(&ServerEventKind::JobStarted {
            job_id: "job-1".to_owned(),
            tool_call_id: "call-1".to_owned(),
            command: "make".to_owned(),
            started_at: 1_700_000_000_000,
            limits: norte_code_protocol::JobLimits {
                timeout_s: Some(120),
                max_output_bytes: Some(1_048_576),
            },
        })
        .expect("job started");
        assert_eq!(started["job_id"], "job-1");
        assert_eq!(started["tool_call_id"], "call-1");
        assert_eq!(started["command"], "make");
        assert_eq!(started["detached"], true);
        assert_eq!(started["started_at"], 1_700_000_000_000_u64);
        assert_eq!(
            started["limits"],
            json!({"timeout_s": 120, "max_output_bytes": 1_048_576})
        );

        let progress = standard_event_payload(&ServerEventKind::JobProgressDelta {
            job_id: "job-1".to_owned(),
            stream: OutputStream::Stderr,
            delta: "warning\n".to_owned(),
            index: 17,
        })
        .expect("job progress");
        assert_eq!(
            progress,
            json!({"stream": "stderr", "delta": "warning\n", "index": 17})
        );

        let finished = standard_event_payload(&ServerEventKind::JobFinished {
            job: JobInfo {
                id: "job-1".to_owned(),
                label: "make".to_owned(),
                script: "make".to_owned(),
                state: norte_code_protocol::JobState::Finished,
                exit_code: Some(0),
                timed_out: false,
                elapsed_ms: 21,
                output: "done\n".to_owned(),
                dropped: false,
                output_bytes_total: 5,
            },
            triggered_new_turn: true,
            new_turn_id: Some(12),
            injected_into_turn_id: None,
        })
        .expect("job finished");
        assert_eq!(finished["status"], "COMPLETED");
        assert_eq!(finished["exit_code"], 0);
        assert_eq!(finished["summary"], "make");
        assert_eq!(finished["output_snapshot"], "done\n");
        assert_eq!(finished["triggered_new_turn"], true);
        assert_eq!(finished["new_turn_id"], "12");
        assert!(finished["injected_into_turn_id"].is_null());
    }
}
