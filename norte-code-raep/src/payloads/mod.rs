//! Canonical RAEP payloads emitted by the `NorteCode` adapter.

use norte_code_wire::payload::{
    AgentChange, AgentChanged, BudgetNotice, ChildAwaitingInput, ChildSessionStarted,
    ChildSessionUpdated, CompactionFinished, DelegatorStateChanged, JobProgress, JobStarted,
    JobsStopped, QuestionAnswered, QuestionAsked, ReasoningDowngraded, SessionStats, SessionTodos,
    TokenChannel, TokenDelta, ToolFinished, ToolOutput, ToolStarted, ToolStatus,
};
use norte_code_wire::{CatalogKind, ServiceDataKind};
use norte_code_wire::{Delta, JobInfo};
use raep_types::{ErrorPayload, HistoryItem, HistoryPage, StopReason, TurnState, TurnStateChanged};
use serde::Serialize;
use serde_json::Value;

mod canonical;
mod standard;

pub(crate) use canonical::canonical_event_payload;
#[cfg(test)]
use serde_json::json;
pub(crate) use standard::standard_event_payload;

/// Serializes a typed RAEP payload. The payloads are closed structs, so this
/// cannot fail; the helper exists so call sites never build wire JSON by hand.
///
/// The return is `Value` only because the envelope's payload slot is the
/// spec's generic `P` — every caller passes a closed struct, and the client
/// decodes it back into the matching struct. No payload is built as `Value`.
pub(crate) fn payload<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("typed RAEP payload is serializable")
}

/// How a turn ended, in the canonical payload's spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CanonicalTurnState {
    Queued,
    CallingProvider,
    ExecutingTool,
    AwaitingApproval,
    Finished,
    Stopped,
}

/// The canonical `turn_state_changed` state block.
#[derive(Debug, Serialize)]
struct TurnStateChangedPayload {
    state: CanonicalTurnState,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<norte_code_wire::Phase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
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

/// The `entries` of a `providers` catalogue: what can be edited and where
/// each entry came from.
#[derive(Debug, Serialize)]
struct CatalogDetailPayload {
    providers: Vec<norte_code_wire::CatalogProviderInfo>,
    models: Vec<norte_code_wire::CatalogModelInfo>,
}

/// The `entries` of a `skills` catalogue.
#[derive(Debug, Serialize)]
struct SkillCatalogEntries {
    skills: Vec<norte_code_wire::SkillInfo>,
    warnings: Vec<String>,
}

/// The `data` of an `mcp_list` service payload.
#[derive(Debug, Serialize)]
struct McpListData {
    servers: Vec<norte_code_wire::McpInfo>,
    failures: Vec<norte_code_wire::McpFailure>,
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

/// Converts a `NorteCode` event into the stable payload understood by RAEP
/// clients.
///
/// The complete `NorteCode` event remains available in the envelope extension.
/// That keeps the adapter lossless while preventing clients from depending on
/// `NorteCode`'s request/session wrapper for the common event families.
#[allow(clippy::too_many_lines)]
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
        role: norte_code_wire::MessageRole,
        kind: norte_code_wire::ReplayedKind,
        text: &'a str,
    },
    /// The current session todos.
    SessionTodos {
        todos: &'a Vec<norte_code_wire::TodoInfo>,
    },
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
        limits: &'a norte_code_wire::JobLimits,
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
    /// One model request finished: what it cost and how fast it ran.
    ///
    /// The name is Norte Code's own (`request_completed`): RAEP has no
    /// standard frame for provider metrics, and reusing
    /// `output_part_finished` made a client mistake a usage row for a media
    /// part.
    RequestCompleted {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Generated media collected by Norte Code for the RAEP output store.
    MediaPartCompleted {
        turn: u64,
        index: u32,
        media: &'a norte_code_wire::MediaPart,
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
        scope: norte_code_wire::NoticeScope,
        unit: norte_code_wire::BudgetUnit,
        spent: f64,
        limit: f64,
    },
    /// A budget was exhausted.
    BudgetExceeded {
        scope: norte_code_wire::NoticeScope,
        unit: norte_code_wire::BudgetUnit,
        spent: f64,
        limit: f64,
        action: norte_code_wire::BudgetAction,
    },
    /// The session budget changed.
    BudgetChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        budget: Option<&'a norte_code_wire::Budget>,
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
        ids: Vec<String>,
        messages: Vec<String>,
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
        code: &'a norte_code_wire::ErrorCode,
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
        state: &'a norte_code_wire::ChildState,
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
    /// One page of history, answering `HistoryGet`.
    HistoryPage { items: Value, has_more: bool },
    /// History was rewound to a turn.
    HistoryRewound {
        to_turn_id: &'a String,
        inclusive: bool,
        rewound_turn_ids: Value,
        side_effects: Value,
        workspace_restored: bool,
    },
    /// A non-text output part started on a turn.
    OutputPartStarted {
        turn: u64,
        part_id: &'a String,
        index: u32,
        modality: Value,
        mime_type: Option<String>,
        streamed: bool,
        bytes_expected: Option<u64>,
    },
    /// A non-text output part finished on a turn.
    OutputPartFinished {
        turn: u64,
        part_id: &'a String,
        index: u32,
        modality: Value,
        bytes: u64,
        sha256: &'a String,
        blob: Option<String>,
        error: Option<String>,
    },
}

#[cfg(test)]
#[path = "../payloads_tests.rs"]
mod tests;
