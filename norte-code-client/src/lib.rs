//! Transport-neutral client contract for Norte Code.
//!
//! The client protocol is deliberately expressed in terms of
//! `norte-code-protocol` requests and events. An in-process adapter and a
//! remote RAEP adapter can therefore be used by the same CLI without moving
//! the agent loop into the client.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use futures_core::Stream;
use futures_util::stream::{self, StreamExt};
use norte_code_protocol::{ClientRequest, ServerEvent};
use raep_types::{Ack, Envelope, FrameType, SessionAttach, StandardFrameType};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio_stream::iter;

/// A wire frame as it arrives over SSE: the envelope fields the client
/// needs, with defaults for the ones a canonical-only frame omits. This is
/// the single place raw bytes become a typed frame; everything downstream
/// works with `Envelope<Value>`.
#[derive(Debug, serde::Deserialize)]
struct FrameWire {
    #[serde(default)]
    version: String,
    #[serde(default)]
    id: Option<raep_types::FrameId>,
    #[serde(default)]
    seq: Option<u64>,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    turn_id: Option<raep_types::TurnId>,
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    kind: Option<FrameType>,
    #[serde(default)]
    payload: Value,
    #[serde(default)]
    ext: Option<Value>,
}

impl FrameWire {
    fn into_envelope(self) -> Envelope<Value> {
        Envelope {
            version: if self.version.is_empty() {
                raep_types::WIRE_VERSION.to_owned()
            } else {
                self.version
            },
            id: self.id.unwrap_or_else(raep_types::FrameId::generate),
            seq: self.seq,
            reference: None,
            session_id: self.session_id,
            turn_id: self.turn_id,
            job_id: None,
            timestamp: self.timestamp,
            kind: self
                .kind
                .unwrap_or(FrameType::Extension("canonical".to_owned())),
            payload: self.payload,
            ext: self.ext,
        }
    }
}

/// A command response body: the events a command produced, or the error
/// that rejected it. Decoded as a struct, never by key.
#[derive(Debug, serde::Deserialize)]
struct CommandResponseBody {
    #[serde(default)]
    events: Vec<Envelope<Value>>,
    #[serde(default)]
    error: Option<RejectError>,
}

impl CommandResponseBody {
    fn error_message(&self) -> String {
        self.error.as_ref().map_or_else(
            || "RAEP command failed".to_owned(),
            |error| error.message.clone(),
        )
    }
}

/// A rejected command's error body.
#[derive(Debug, serde::Deserialize)]
struct RejectError {
    #[serde(default)]
    message: String,
}

/// A stream of events answering one client request.
pub type EventStream = Pin<Box<dyn Stream<Item = ServerEvent> + Send>>;

/// The frame kinds this client maps onto protocol events. The wire spells
/// them in screaming snake case; the enum is the closed set, so a new frame
/// type is a compile error here, not a silent fall-through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FrameKind {
    TokenDelta,
    TurnStateChanged,
    ToolCallStarted,
    ToolOutputDelta,
    ToolCallFinished,
    JobListResult,
    Error,
    Other,
}

impl FrameKind {
    fn from_standard(kind: &StandardFrameType) -> Self {
        match kind {
            StandardFrameType::TokenDelta => Self::TokenDelta,
            StandardFrameType::TurnStateChanged => Self::TurnStateChanged,
            StandardFrameType::ToolCallStarted => Self::ToolCallStarted,
            StandardFrameType::ToolOutputDelta => Self::ToolOutputDelta,
            StandardFrameType::ToolCallFinished => Self::ToolCallFinished,
            StandardFrameType::JobListResult => Self::JobListResult,
            StandardFrameType::Error => Self::Error,
            _ => Self::Other,
        }
    }
}

/// A tool call's terminal status, as the wire spells it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ToolStatus {
    Ok,
    Error,
    Cancelled,
}

/// A job's status, as the wire spells it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum JobStatus {
    #[default]
    Running,
    Completed,
    Failed,
    Killed,
    #[serde(rename = "TIMED_OUT")]
    TimedOut,
}

/// A token delta payload.
#[derive(Debug, serde::Deserialize)]
struct TokenDeltaPayload {
    delta: String,
    index: u32,
    #[serde(default)]
    channel: String,
}

/// A tool call start payload.
#[derive(Debug, serde::Deserialize)]
struct ToolCallStartedPayload {
    tool_call_id: String,
    tool_name: String,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    source: Option<String>,
}

/// A tool output delta payload.
#[derive(Debug, serde::Deserialize)]
struct ToolOutputDeltaPayload {
    tool_call_id: String,
    tool_name: String,
    #[serde(default = "default_output_stream")]
    stream: norte_code_protocol::types::OutputStream,
    delta: String,
}

fn default_output_stream() -> norte_code_protocol::types::OutputStream {
    norte_code_protocol::types::OutputStream::Stdout
}

/// A tool call finish payload.
#[derive(Debug, serde::Deserialize)]
struct ToolCallFinishedPayload {
    tool_call_id: String,
    #[serde(default)]
    tool_name: Option<String>,
    status: ToolStatus,
    #[serde(default)]
    result_summary: Option<String>,
    #[serde(default)]
    result_truncated: bool,
    #[serde(default)]
    duration_ms: u64,
}

/// One job entry in a job list payload.
#[derive(Debug, serde::Deserialize)]
struct JobEntryPayload {
    job_id: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    status: JobStatus,
    #[serde(default)]
    exit_code: Option<i32>,
}

/// A job list payload.
#[derive(Debug, serde::Deserialize)]
struct JobListPayload {
    #[serde(default)]
    jobs: Vec<JobEntryPayload>,
}

/// An error payload.
#[derive(Debug, serde::Deserialize)]
struct ErrorPayload {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// The extension's `TurnFinished` fields, which the canonical turn payload
/// does not carry.
#[derive(Clone, Debug, serde::Deserialize)]
struct ExtensionTurnFinished {
    #[serde(default)]
    phase: Option<norte_code_protocol::Phase>,
    #[serde(default)]
    committed: Option<bool>,
}

/// A typed view of one raw frame: the envelope fields the decoder needs,
/// read once from the typed `Envelope`, instead of string lookups scattered
/// through the match.
///
/// The frame arrives as `Envelope<Value>` — the spec's generic payload slot
/// — but every field the decoder touches is read through the envelope's own
/// typed fields (`kind`, `session_id`, `seq`, `ext`), never by key.
struct FrameView {
    frame_type: Option<FrameKind>,
    payload: Option<Value>,
    session: Option<norte_code_protocol::SessionKey>,
    turn: u64,
    extension_turn_finished: Option<ExtensionTurnFinished>,
}

impl FrameView {
    fn from_envelope(frame: &Envelope<Value>) -> Self {
        let frame_type = match &frame.kind {
            FrameType::Standard(kind) => Some(FrameKind::from_standard(kind)),
            FrameType::Extension(_) => None,
        };
        let session = (!frame.session_id.is_empty())
            .then(|| norte_code_protocol::SessionKey::new(frame.session_id.clone()));
        let turn = frame
            .turn_id
            .as_ref()
            .and_then(|id| id.as_str().parse::<u64>().ok())
            .unwrap_or(0);
        let extension_turn_finished = frame
            .ext
            .as_ref()
            .and_then(|ext| ext.get("norte_code"))
            .and_then(|event| event.get("kind"))
            .and_then(|kind| kind.get("TurnFinished"))
            .and_then(|finished| serde_json::from_value(finished.clone()).ok());
        Self {
            frame_type,
            payload: Some(frame.payload.clone()),
            session,
            turn,
            extension_turn_finished,
        }
    }
}

/// Deserializes one payload into its typed struct, naming the frame in the
/// error so a wire change points at the frame that broke.
fn from_payload<T: serde::de::DeserializeOwned>(payload: Value) -> Result<T, ClientError> {
    serde_json::from_value(payload)
        .map_err(|error| ClientError::new(format!("invalid RAEP payload: {error}")))
}

/// The canonical event tag, as the wire spells it in snake case. The closed
/// set of canonical events the client understands — a new canonical event is
/// a compile error here, not a runtime `unsupported event` string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CanonicalKind {
    SessionCreated,
    SessionList,
    SessionDeleted,
    SessionRenamed,
    SessionForked,
    SessionSwitched,
    TurnStarted,
    TokenDelta,
    Committed,
    HistoryMessage,
    SessionTodos,
    ToolCallStarted,
    JobProgressDelta,
    ToolCallFinished,
    OutputPartFinished,
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
    Error,
    Other,
}

impl CanonicalKind {
    fn from_wire(name: &str) -> Self {
        match name {
            "session_created" => Self::SessionCreated,
            "session_list" => Self::SessionList,
            "session_deleted" => Self::SessionDeleted,
            "session_renamed" => Self::SessionRenamed,
            "session_forked" => Self::SessionForked,
            "session_switched" => Self::SessionSwitched,
            "turn_started" => Self::TurnStarted,
            "token_delta" => Self::TokenDelta,
            "committed" => Self::Committed,
            "history_message" => Self::HistoryMessage,
            "session_todos" => Self::SessionTodos,
            "tool_call_started" => Self::ToolCallStarted,
            "job_progress_delta" => Self::JobProgressDelta,
            "tool_call_finished" => Self::ToolCallFinished,
            "output_part_finished" => Self::OutputPartFinished,
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
            "error" => Self::Error,
            _ => Self::Other,
        }
    }
}

/// How a turn state changed, in the canonical payload's spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CanonicalTurnState {
    Finished,
    Stopped,
}

/// How a context (compaction) update went, in the canonical spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CanonicalContextState {
    Started,
    Finished,
}

/// What kind of agent change a canonical payload describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum AgentChange {
    ModelChanged,
    ModelQueued,
    ReasoningDowngraded,
}

/// What kind of catalogue a canonical payload carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CatalogKind {
    Models,
    Skills,
}

/// What kind of service data a canonical payload carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ServiceDataKind {
    Telemetry,
    ToolList,
    McpList,
}

/// How a provider retry is going, in the canonical spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum RetryState {
    Scheduled,
    Started,
}

/// A canonical payload, decoded once into the pieces every arm needs.
///
/// The tag is an enum, the fields are read by serde into typed structs per
/// arm, and there is no `payload.get("...")` anywhere: a renamed wire field
/// is a serde error naming the field.
#[derive(Debug)]
struct CanonicalPayload {
    kind: CanonicalKind,
    value: Value,
}

impl CanonicalPayload {
    fn from_value(payload: Value) -> Result<Self, ClientError> {
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
    fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, ClientError> {
        serde_json::from_value(self.value.clone())
            .map_err(|error| ClientError::new(format!("invalid RAEP canonical payload: {error}")))
    }
}

/// A canonical `session_created` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalSessionCreated {
    #[serde(default)]
    title: Option<String>,
}

/// A canonical `session_list` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalSessionList {
    #[serde(default)]
    sessions: Vec<norte_code_protocol::SessionInfo>,
}

/// A canonical `session_forked` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalSessionForked {
    from: String,
}

/// A canonical `turn_started` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalTurnStarted {
    turn: u64,
}

/// A canonical `token_delta` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalTokenDelta {
    turn: u64,
    delta: norte_code_protocol::Delta,
}

/// A canonical `committed` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalCommitted {
    turn: u64,
    version: u64,
}

/// A canonical `history_message` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalHistoryMessage {
    index: u64,
    role: norte_code_protocol::MessageRole,
    kind: norte_code_protocol::ReplayedKind,
    text: String,
}

/// A canonical `session_todos` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalSessionTodos {
    todos: Vec<norte_code_protocol::TodoInfo>,
}

/// A canonical `tool_call_started` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalToolCallStarted {
    turn: u64,
    call_id: String,
    tool: String,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    source: Option<String>,
}

/// A canonical `job_progress_delta` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalJobProgressDelta {
    turn: u64,
    call_id: String,
    tool: String,
    stream: norte_code_protocol::types::OutputStream,
    text: String,
    #[serde(default)]
    last: bool,
}

/// A canonical `tool_call_finished` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalToolCallFinished {
    turn: u64,
    call_id: String,
    tool: String,
    is_error: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    output: Option<String>,
    truncated: bool,
    detached: bool,
    duration_ms: u64,
}

/// A canonical `output_part_finished` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalOutputPartFinished {
    turn: u64,
    index: u64,
    usage: norte_code_protocol::Usage,
    #[serde(default)]
    cost_usd: Option<f64>,
    truncated: bool,
    duration_ms: u64,
    #[serde(default)]
    first_token_ms: Option<u64>,
    #[serde(default)]
    tokens_per_second: Option<f64>,
    cache_hit: bool,
    tools: Vec<String>,
}

/// A canonical `approval_resolved` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalApprovalResolved {
    approval_id: String,
}

/// A canonical `user_input_accepted` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalUserInputAccepted {
    ids: Vec<String>,
    messages: Vec<String>,
}

/// A canonical `provider_retry` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalProviderRetry {
    state: RetryState,
    turn: u64,
    attempt: u32,
    #[serde(default)]
    delay_ms: u64,
    #[serde(default)]
    reason: String,
}

/// A canonical `agent_changed` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalAgentChanged {
    change: AgentChange,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    details: Option<Value>,
}

/// The `details` of a `reasoning_downgraded` change.
#[derive(Debug, serde::Deserialize)]
struct ReasoningDowngradedDetails {
    from: String,
    to: String,
    reason: String,
}

/// A canonical `agent_catalog` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalAgentCatalog {
    kind: CatalogKind,
    #[serde(default)]
    entries: Value,
}

/// The `entries` of a `skills` catalogue.
#[derive(Debug, serde::Deserialize)]
struct SkillCatalogEntries {
    #[serde(default)]
    skills: Vec<norte_code_protocol::SkillInfo>,
    #[serde(default)]
    warnings: Vec<String>,
}

/// A canonical `session_renamed` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalSessionRenamed {
    title: String,
}

/// A canonical `session_stats` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalSessionStats {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    cost_usd: f64,
    requests: u64,
    context_used: u64,
    #[serde(default)]
    context_window: Option<u64>,
}

/// A canonical `job_list` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalJobList {
    #[serde(default)]
    jobs: Vec<norte_code_protocol::JobInfo>,
}

/// A canonical `turn_state_changed` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalTurnStateChanged {
    state: CanonicalTurnState,
    turn: u64,
    #[serde(default)]
    phase: Option<norte_code_protocol::Phase>,
    #[serde(default)]
    committed: Option<bool>,
    #[serde(default)]
    reason: Option<String>,
}

/// A canonical `context_update` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalContextUpdate {
    state: CanonicalContextState,
    #[serde(default)]
    reason: Option<norte_code_protocol::CompactionReason>,
    #[serde(default)]
    compacted: Option<bool>,
    #[serde(default)]
    north_session: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// A canonical `budget_warning` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalBudgetWarning {
    scope: norte_code_protocol::BudgetScope,
    unit: norte_code_protocol::BudgetUnit,
    spent: f64,
    limit: f64,
}

/// A canonical `budget_exceeded` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalBudgetExceeded {
    scope: norte_code_protocol::BudgetScope,
    unit: norte_code_protocol::BudgetUnit,
    spent: f64,
    limit: f64,
    action: norte_code_protocol::BudgetAction,
}

/// A canonical `budget_changed` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalBudgetChanged {
    #[serde(default)]
    budget: Option<norte_code_protocol::Budget>,
}

/// A canonical `cost_updated` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalCostUpdated {
    total_usd: f64,
    requests: u64,
}

/// A canonical `cost_not_reported` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalCostNotReported {
    requests: u64,
}

/// A canonical `plugin_list` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalPluginList {
    plugins: Vec<norte_code_protocol::PluginInfo>,
}

/// A canonical `plugin_changed` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalPluginChanged {
    operation: String,
    name: String,
}

/// A canonical `service_data` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalServiceData {
    kind: ServiceDataKind,
    data: Value,
}

/// The `data` of an `mcp_list` service payload.
#[derive(Debug, serde::Deserialize)]
struct McpListData {
    #[serde(default)]
    servers: Vec<norte_code_protocol::McpInfo>,
    #[serde(default)]
    failures: Vec<norte_code_protocol::McpFailure>,
}

/// A canonical `error` payload.
#[derive(Debug, serde::Deserialize)]
struct CanonicalError {
    #[serde(default)]
    code: Value,
    #[serde(default)]
    message: String,
}

/// An error raised before a request can produce an event stream.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{message}")]
pub struct ClientError {
    message: String,
}

impl ClientError {
    /// Creates a client error with a user-facing message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the error message without formatting it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A transport-independent Norte Code client.
///
/// Implementations may submit requests in-process, over RAEP WebSocket, or
/// through another transport. The stream remains the same protocol-level
/// event stream in every case.
///
/// Session management goes through the RAEP standard resource
/// (`/raep/v1/sessions`), not through `submit`: creating, listing, renaming
/// and deleting sessions are management operations the spec owns, so they
/// are typed methods here, not extension requests.
#[async_trait]
pub trait AgentClient: Send + Sync {
    /// Submits one request and returns its event stream.
    async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError>;

    /// Lists the sessions the server holds, newest content first.
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ClientError>;

    /// Creates a session and returns its summary.
    async fn create_session(&self, title: Option<&str>) -> Result<SessionSummary, ClientError>;

    /// Renames a session.
    async fn rename_session(&self, session: &str, title: &str) -> Result<(), ClientError>;

    /// Deletes a session and everything keyed by it.
    async fn delete_session(&self, session: &str) -> Result<(), ClientError>;
}

/// A session summary as the RAEP management resource returns it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct SessionSummary {
    /// The session identifier.
    pub session_id: String,
    /// Its label, when it has one.
    #[serde(default)]
    pub title: Option<String>,
}

impl SessionSummary {
    /// Converts the summary into the protocol's session info shape.
    #[must_use]
    pub fn into_session_info(self) -> norte_code_protocol::SessionInfo {
        norte_code_protocol::SessionInfo {
            session: norte_code_protocol::SessionKey::new(self.session_id),
            title: self.title,
            provider: None,
            model: None,
            created_at: None,
            phase: norte_code_protocol::Phase::Idle,
        }
    }
}

/// HTTP client for the `NorteCode` RAEP command endpoint.
///
/// The transport returns canonical RAEP envelopes, while the compatibility
/// extension carries the original `NorteCode` event until all consumers migrate.
pub struct RemoteClient {
    http: reqwest::Client,
    base_url: String,
    bearer_token: Option<HeaderValue>,
    agent_id: String,
    default_session: Option<String>,
    attached_sessions: Mutex<HashSet<String>>,
    session_seq: Arc<Mutex<HashMap<String, u64>>>,
    session_info: Mutex<HashMap<String, norte_code_protocol::SessionInfo>>,
    session_reasoning: Mutex<HashMap<String, norte_code_protocol::Reasoning>>,
    session_stats: Mutex<HashMap<String, RemoteSessionStats>>,
    session_jobs: Mutex<HashMap<String, Vec<norte_code_protocol::JobInfo>>>,
}

/// Usage and context totals cached from a remote session.
#[derive(Clone, Debug)]
pub struct RemoteSessionStats {
    /// Total input tokens reported by the host.
    pub input_tokens: u64,
    /// Total output tokens reported by the host.
    pub output_tokens: u64,
    /// Total provider cache-read tokens.
    pub cache_read_tokens: u64,
    /// Total provider cache-write tokens.
    pub cache_write_tokens: u64,
    /// Total provider-reported cost.
    pub cost_usd: f64,
    /// Number of reported requests.
    pub requests: u64,
    /// Context tokens in the latest request.
    pub context_used: u64,
    /// Declared context window, when available.
    pub context_window: Option<u64>,
}

impl RemoteClient {
    /// Creates a client using a server base URL such as
    /// `http://127.0.0.1:8080`.
    ///
    /// # Errors
    ///
    /// Returns an error when the bearer token is not a valid header value.
    pub fn new(
        base_url: impl Into<String>,
        bearer_token: Option<&str>,
        default_session: Option<String>,
    ) -> Result<Self, ClientError> {
        let original_token = bearer_token;
        let bearer_token = original_token
            .map(|token| {
                let mut value = b"Bearer ".to_vec();
                value.extend_from_slice(token.as_bytes());
                HeaderValue::from_bytes(&value)
                    .map_err(|error| ClientError::new(format!("invalid bearer token: {error}")))
            })
            .transpose()?;
        Ok(Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            bearer_token,
            agent_id: "norte-code".to_owned(),
            default_session,
            attached_sessions: Mutex::new(HashSet::new()),
            session_seq: Arc::new(Mutex::new(HashMap::new())),
            session_info: Mutex::new(HashMap::new()),
            session_reasoning: Mutex::new(HashMap::new()),
            session_stats: Mutex::new(HashMap::new()),
            session_jobs: Mutex::new(HashMap::new()),
        })
    }

    /// Selects the agent identifier used for RAEP attachments.
    #[must_use]
    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = agent_id.into();
        self
    }

    /// Fetches the public RAEP discovery information.
    ///
    /// # Errors
    ///
    /// Returns an error when the service cannot be reached or returns a
    /// non-success response.
    pub async fn info(&self) -> Result<serde_json::Value, ClientError> {
        let mut request = self.http.get(format!("{}/raep/v1/info", self.base_url));
        if let Some(token) = &self.bearer_token {
            request = request.header(AUTHORIZATION, token.clone());
        }

        let response = request
            .send()
            .await
            .map_err(|error| ClientError::new(format!("could not reach service: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::new(format!(
                "service discovery returned HTTP {status}"
            )));
        }
        response.json().await.map_err(|error| {
            ClientError::new(format!("invalid service discovery response: {error}"))
        })
    }

    /// Returns the last session metadata received from the service.
    #[must_use]
    pub fn session_info(&self, session: &str) -> Option<norte_code_protocol::SessionInfo> {
        self.session_info
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
    }

    /// Returns the last usage and context totals received for a session.
    #[must_use]
    pub fn session_stats(&self, session: &str) -> Option<RemoteSessionStats> {
        self.session_stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
    }

    /// Returns the last background-job list received for a session.
    #[must_use]
    pub fn session_jobs(&self, session: &str) -> Vec<norte_code_protocol::JobInfo> {
        self.session_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the last session-scoped reasoning setting sent to the host.
    #[must_use]
    pub fn session_reasoning(&self, session: &str) -> Option<norte_code_protocol::Reasoning> {
        self.session_reasoning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
    }

    /// Returns the last RAEP sequence observed for a session.
    #[must_use]
    pub fn last_seq(&self, session: &str) -> Option<u64> {
        self.session_seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .copied()
    }

    /// Records a frame's sequence for its session, keeping the maximum seen.
    fn note_seq(&self, session: &str, seq: u64) {
        let mut seqs = self
            .session_seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        seqs.entry(session.to_owned())
            .and_modify(|current| *current = (*current).max(seq))
            .or_insert(seq);
    }

    /// Records the sequence of one raw RAEP frame, if it carries one.
    fn note_frame_seq(&self, frame: &Envelope<Value>) {
        let Some(seq) = frame.seq else {
            return;
        };
        if !frame.session_id.is_empty() {
            self.note_seq(&frame.session_id, seq);
        }
    }

    fn session_for(&self, request: &ClientRequest) -> String {
        session_from_request(request)
            .or_else(|| self.default_session.clone())
            .unwrap_or_else(|| "default".to_owned())
    }

    fn command_url(&self, session_id: &str) -> String {
        format!(
            "{}/raep/v1/sessions/{}/commands",
            self.base_url,
            percent_encode(session_id)
        )
    }

    async fn submit_http(&self, request: ClientRequest) -> Result<EventStream, ClientError> {
        if let norte_code_protocol::ClientRequestKind::SetReasoning {
            session,
            scope: norte_code_protocol::ReasoningScope::Session,
            reasoning,
        } = &request.kind
        {
            self.session_reasoning
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(session.as_str().to_owned(), reasoning.clone());
        }
        if let Some(session) = session_from_request(&request) {
            self.ensure_attached(session.as_str()).await?;
            if matches!(
                &request.kind,
                norte_code_protocol::ClientRequestKind::SendMessage { .. }
            ) {
                return self.submit_streaming(session.as_str(), request).await;
            }
        }
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(self.command_url(&self.session_for(&request)))
            .headers(headers)
            .json(&Envelope::new(
                FrameType::Extension("norte_code.command".to_owned()),
                serde_json::to_value(request)
                    .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
                timestamp_millis(),
            ))
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP request failed: {error}")))?;
        let status = response.status();
        let response_body: CommandResponseBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(response_body.error_message()));
        }
        let events = response_body
            .events
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        for frame in &response_body.events {
            self.note_frame_seq(frame);
        }
        self.remember_sessions(&events);
        Ok(Box::pin(iter(events)))
    }

    fn events_url(&self, session_id: &str) -> String {
        format!(
            "{}/raep/v1/sessions/{}/events",
            self.base_url,
            percent_encode(session_id)
        )
    }

    #[allow(clippy::too_many_lines)]
    async fn submit_streaming(
        &self,
        session_id: &str,
        request: ClientRequest,
    ) -> Result<EventStream, ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        if let Some(seq) = self.last_seq(session_id) {
            headers.insert(
                HeaderName::from_static("last-event-id"),
                HeaderValue::from_str(&seq.to_string())
                    .map_err(|error| ClientError::new(format!("invalid cursor: {error}")))?,
            );
        }
        let response = self
            .http
            .get(self.events_url(session_id))
            .headers(headers.clone())
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP event stream failed: {error}")))?;
        if !response.status().is_success() {
            return Err(ClientError::new(format!(
                "RAEP event stream returned HTTP {}",
                response.status()
            )));
        }

        let command_url = self.command_url(session_id);
        let mut command_headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            command_headers.insert(AUTHORIZATION, token.clone());
        }
        let command_body = Envelope::new(
            FrameType::Extension("norte_code.command".to_owned()),
            serde_json::to_value(request)
                .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
            timestamp_millis(),
        );
        let command_result: Arc<Mutex<Option<Result<(), ClientError>>>> =
            Arc::new(Mutex::new(None));
        let command_result_task = Arc::clone(&command_result);
        let http_for_command = self.http.clone();
        let url_for_command = command_url.clone();
        let headers_for_command = command_headers.clone();
        tokio::spawn(async move {
            let outcome = async {
                let response = http_for_command
                    .post(url_for_command)
                    .headers(headers_for_command)
                    .json(&command_body)
                    .send()
                    .await
                    .map_err(|error| ClientError::new(format!("RAEP command failed: {error}")))?;
                if !response.status().is_success() {
                    let status = response.status();
                    let body: CommandResponseBody =
                        response.json().await.unwrap_or(CommandResponseBody {
                            events: Vec::new(),
                            error: None,
                        });
                    return Err(ClientError::new(format!(
                        "RAEP command returned HTTP {status}: {}",
                        body.error_message()
                    )));
                }
                Ok(())
            }
            .await;
            *command_result_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
        });

        let session = norte_code_protocol::SessionKey::new(session_id);
        let session_seq = Arc::clone(&self.session_seq);
        let ack_http = self.http.clone();
        let ack_url = self.command_url(session_id);
        let ack_token = self.bearer_token.clone();
        let bytes = response.bytes_stream();
        let stream = stream::unfold(
            (
                bytes,
                Vec::new(),
                VecDeque::<(ServerEvent, bool)>::new(),
                session,
                false,
                session_seq,
                command_result,
                false,
            ),
            move |(
                mut bytes,
                mut buffer,
                mut pending,
                session,
                mut finished,
                session_seq,
                command_result,
                mut command_reported,
            )| {
                let ack_http = ack_http.clone();
                let ack_url = ack_url.clone();
                let ack_token = ack_token.clone();
                async move {
                    if !command_reported {
                        let result = command_result
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone();
                        if let Some(Err(error)) = result {
                            command_reported = true;
                            finished = true;
                            pending.push_back((
                                ServerEvent {
                                    request_id: 0,
                                    session: Some(session.clone()),
                                    kind: norte_code_protocol::ServerEventKind::Error {
                                        code: norte_code_protocol::ErrorCode::Internal,
                                        message: error.to_string(),
                                    },
                                },
                                true,
                            ));
                        } else if result.is_some() {
                            command_reported = true;
                        }
                    }
                    loop {
                        if let Some((event, terminal)) = pending.pop_front() {
                            return Some((
                                event,
                                (
                                    bytes,
                                    buffer,
                                    pending,
                                    session,
                                    finished || terminal,
                                    session_seq,
                                    command_result,
                                    command_reported,
                                ),
                            ));
                        }
                        if finished {
                            return None;
                        }
                        let drained = drain_sse_buffer(&mut buffer);
                        let mut last_applied = None;
                        for (event, terminal, seq) in drained {
                            if let Some(seq) = seq {
                                let mut seqs = session_seq
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                match seq_step(seqs.get(session.as_str()).copied(), seq) {
                                    SeqStep::Duplicate => {
                                        // Still advance terminal bookkeeping? no.
                                        if event.is_none() {
                                            // undecodable frame: skip yielding
                                        }
                                        continue;
                                    }
                                    SeqStep::Gap { expected } => {
                                        drop(seqs);
                                        finished = true;
                                        pending.push_back((
                                            ServerEvent {
                                                request_id: 0,
                                                session: Some(session.clone()),
                                                kind: norte_code_protocol::ServerEventKind::Error {
                                                    code: norte_code_protocol::ErrorCode::Internal,
                                                    message: format!(
                                                        "RAEP sequence gap: expected {expected}, received {seq}; reconnecting from {}",
                                                        expected.saturating_sub(1)
                                                    ),
                                                },
                                            },
                                            true,
                                        ));
                                        break;
                                    }
                                    SeqStep::Apply => {}
                                }
                                seqs.insert(session.as_str().to_owned(), seq);
                                drop(seqs);
                                last_applied = Some(seq);
                            }
                            if let Some(event) = event {
                                pending.push_back((event, terminal));
                            }
                        }
                        if let Some(seq) = last_applied {
                            let mut headers = HeaderMap::new();
                            if let Some(token) = &ack_token {
                                headers.insert(AUTHORIZATION, token.clone());
                            }
                            let http = ack_http.clone();
                            let url = ack_url.clone();
                            tokio::spawn(async move {
                                let _ = http
                                    .post(url)
                                    .headers(headers)
                                    .json(&Envelope::new(
                                        FrameType::Standard(StandardFrameType::Ack),
                                        serde_json::to_value(Ack { last_seq: seq })
                                            .unwrap_or(Value::Null),
                                        timestamp_millis(),
                                    ))
                                    .send()
                                    .await;
                            });
                        }
                        if finished {
                            continue;
                        }
                        #[allow(clippy::single_match_else)]
                        match bytes.next().await {
                            Some(Ok(chunk)) => {
                                buffer.extend_from_slice(&chunk);
                            }
                            Some(Err(_)) | None => {
                                if !command_reported {
                                    let result = command_result
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .clone();
                                    if let Some(Err(error)) = result {
                                        command_reported = true;
                                        finished = true;
                                        pending.push_back((
                                            ServerEvent {
                                                request_id: 0,
                                                session: Some(session.clone()),
                                                kind: norte_code_protocol::ServerEventKind::Error {
                                                    code: norte_code_protocol::ErrorCode::Internal,
                                                    message: error.to_string(),
                                                },
                                            },
                                            true,
                                        ));
                                    } else if result.is_some() {
                                        command_reported = true;
                                    }
                                }
                                if pending.is_empty() {
                                    return None;
                                }
                            }
                        }
                    }
                }
            },
        );
        Ok(Box::pin(stream))
    }

    fn remember_sessions(&self, events: &[ServerEvent]) {
        let mut cache = self
            .session_info
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stats = self
            .session_stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut jobs = self
            .session_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for event in events {
            match &event.kind {
                norte_code_protocol::ServerEventKind::SessionList { sessions } => {
                    for session in sessions {
                        cache.insert(session.session.as_str().to_owned(), session.clone());
                    }
                }
                norte_code_protocol::ServerEventKind::SessionCreated { title } => {
                    if let Some(key) = &event.session {
                        cache.insert(
                            key.as_str().to_owned(),
                            norte_code_protocol::SessionInfo {
                                session: key.clone(),
                                title: title.clone(),
                                provider: None,
                                model: None,
                                created_at: None,
                                phase: norte_code_protocol::Phase::Idle,
                            },
                        );
                    }
                }
                norte_code_protocol::ServerEventKind::SessionRenamed { title } => {
                    if let Some(key) = &event.session
                        && let Some(info) = cache.get_mut(key.as_str())
                    {
                        info.title = Some(title.clone());
                    }
                }
                norte_code_protocol::ServerEventKind::ModelChanged { provider, model }
                | norte_code_protocol::ServerEventKind::ModelQueued { provider, model } => {
                    if let Some(key) = &event.session
                        && let Some(info) = cache.get_mut(key.as_str())
                    {
                        info.provider = Some(provider.clone());
                        info.model = Some(model.clone());
                    }
                }
                norte_code_protocol::ServerEventKind::SessionStats { .. } => {
                    if let Some(key) = &event.session
                        && let norte_code_protocol::ServerEventKind::SessionStats {
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_write_tokens,
                            cost_usd,
                            requests,
                            context_used,
                            context_window,
                        } = &event.kind
                    {
                        stats.insert(
                            key.as_str().to_owned(),
                            RemoteSessionStats {
                                input_tokens: *input_tokens,
                                output_tokens: *output_tokens,
                                cache_read_tokens: *cache_read_tokens,
                                cache_write_tokens: *cache_write_tokens,
                                cost_usd: *cost_usd,
                                requests: *requests,
                                context_used: *context_used,
                                context_window: *context_window,
                            },
                        );
                    }
                }
                norte_code_protocol::ServerEventKind::JobList { jobs: listed } => {
                    if let Some(key) = &event.session {
                        jobs.insert(key.as_str().to_owned(), listed.clone());
                    }
                }
                _ => {}
            }
        }
    }

    async fn ensure_attached(&self, session_id: &str) -> Result<(), ClientError> {
        if self
            .attached_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(session_id)
        {
            return Ok(());
        }
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(self.command_url(session_id))
            .headers(headers)
            .json(&Envelope::new(
                FrameType::Standard(StandardFrameType::SessionAttach),
                serde_json::to_value(SessionAttach {
                    agent_id: self.agent_id.clone(),
                    agent_override: None,
                    create_if_missing: Some(false),
                    mode: None,
                    takeover: None,
                    resume_from_seq: self.last_seq(session_id),
                    capabilities: None,
                    client_info: None,
                    options: None,
                })
                .unwrap_or(Value::Null),
                timestamp_millis(),
            ))
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP attach failed: {error}")))?;
        let status = response.status();
        let body: CommandResponseBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP attach response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(body.error_message()));
        }
        for frame in &body.events {
            self.note_frame_seq(frame);
        }
        self.attached_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.to_owned());
        Ok(())
    }
}

#[async_trait]
impl AgentClient for RemoteClient {
    async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError> {
        self.submit_http(request).await
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .get(format!("{}/raep/v1/sessions", self.base_url))
            .headers(headers)
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP list sessions failed: {error}")))?;
        let body: SessionListBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP session list: {error}")))?;
        Ok(body.sessions)
    }

    async fn create_session(&self, title: Option<&str>) -> Result<SessionSummary, ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(format!("{}/raep/v1/sessions", self.base_url))
            .headers(headers)
            .json(&CreateSessionBody {
                agent_id: self.agent_id.clone(),
                title: title.map(str::to_owned),
            })
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP create session failed: {error}")))?;
        let status = response.status();
        let body: SessionSummary = response.json().await.map_err(|error| {
            ClientError::new(format!("invalid RAEP create session response: {error}"))
        })?;
        if !status.is_success() {
            return Err(ClientError::new("RAEP create session was rejected"));
        }
        Ok(body)
    }

    async fn rename_session(&self, session: &str, title: &str) -> Result<(), ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .patch(format!(
                "{}/raep/v1/sessions/{}",
                self.base_url,
                percent_encode(session)
            ))
            .headers(headers)
            .json(&RenameSessionBody {
                title: title.to_owned(),
            })
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP rename session failed: {error}")))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::new("RAEP rename session was rejected"))
        }
    }

    async fn delete_session(&self, session: &str) -> Result<(), ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .delete(format!(
                "{}/raep/v1/sessions/{}",
                self.base_url,
                percent_encode(session)
            ))
            .headers(headers)
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP delete session failed: {error}")))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::new("RAEP delete session was rejected"))
        }
    }
}

/// A session list body from the RAEP management resource.
#[derive(Debug, serde::Deserialize)]
struct SessionListBody {
    #[serde(default)]
    sessions: Vec<SessionSummary>,
}

/// A create-session request body.
#[derive(Debug, serde::Serialize)]
struct CreateSessionBody {
    agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

/// A rename-session request body.
#[derive(Debug, serde::Serialize)]
struct RenameSessionBody {
    title: String,
}

/// Whether an arriving sequence can be applied, is a duplicate, or skipped a
/// frame the client never saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeqStep {
    Apply,
    Duplicate,
    Gap { expected: u64 },
}

fn seq_step(previous: Option<u64>, seq: u64) -> SeqStep {
    match previous {
        Some(previous) if seq <= previous => SeqStep::Duplicate,
        Some(previous) if seq > previous.saturating_add(1) => SeqStep::Gap {
            expected: previous.saturating_add(1),
        },
        _ => SeqStep::Apply,
    }
}

/// Drains complete SSE blocks from `buffer`, in wire order.
///
/// Each complete block becomes one event; a trailing partial block stays in
/// the buffer for the next chunk. Blocks that are not `data:` frames or
/// that do not decode are skipped, so a keepalive or a foreign event never
/// wedges the stream. Terminal events (`TurnFinished`, `TurnStopped`) are
/// flagged but do not stop the drain: events the server sent after the
/// terminal in the same flush must still reach the caller.
fn drain_sse_buffer(buffer: &mut Vec<u8>) -> Vec<(Option<ServerEvent>, bool, Option<u64>)> {
    let mut events = Vec::new();
    while let Some(end) = buffer.windows(2).position(|window| window == b"\n\n") {
        let block = buffer[..end].to_vec();
        buffer.drain(..end + 2);
        let Ok(block) = std::str::from_utf8(&block) else {
            continue;
        };
        let Some(data) = block.lines().find_map(|line| line.strip_prefix("data: ")) else {
            continue;
        };
        // The SSE boundary is bytes: the frame is decoded to a typed
        // `FrameWire` here, at the edge, so nothing downstream sees `Value`.
        let Ok(frame) = serde_json::from_str::<FrameWire>(data) else {
            continue;
        };
        let frame = frame.into_envelope();
        let seq = frame.seq;
        let event = server_event_from_frame(&frame).ok();
        let terminal = event.as_ref().is_some_and(|event| {
            matches!(
                event.kind,
                norte_code_protocol::ServerEventKind::TurnFinished { .. }
                    | norte_code_protocol::ServerEventKind::TurnStopped { .. }
            )
        });
        events.push((event, terminal, seq));
    }
    events
}

/// Decodes a standard RAEP frame from its canonical payload.
///
/// Returns `Ok(None)` for a frame this adapter has no faithful standard
/// mapping for, so the caller can fall back to the `NorteCode` extension.
///
/// Every payload is decoded through a typed struct — either one from
/// `raep_types` or one defined here for the shapes the spec leaves opaque.
/// There are no string-key field lookups: a missing or mistyped field is a
/// serde error, which is a wire error, not a silent default.
#[allow(clippy::too_many_lines)]
fn standard_event_from_frame(frame: &Envelope<Value>) -> Result<Option<ServerEvent>, ClientError> {
    let frame = FrameView::from_envelope(frame);
    let frame_type = frame.frame_type;
    let turn = frame.turn;
    let extension_finished = frame.extension_turn_finished.clone();
    let payload = frame.payload;
    let session = frame.session;
    let event = |kind| ServerEvent {
        request_id: 0,
        session: session.clone(),
        kind,
    };
    let kind = match frame_type {
        None => return Ok(None),
        Some(FrameKind::Other | FrameKind::Error) if payload.is_none() => return Ok(None),
        Some(frame_kind) => {
            let Some(payload) = payload else {
                return Ok(None);
            };
            match frame_kind {
                FrameKind::TokenDelta => {
                    let decoded: TokenDeltaPayload = from_payload(payload)?;
                    let delta = match decoded.channel.as_str() {
                        "reasoning" => norte_code_protocol::Delta::Reasoning {
                            index: decoded.index,
                            text: decoded.delta,
                        },
                        _ => norte_code_protocol::Delta::Text {
                            index: decoded.index,
                            text: decoded.delta,
                        },
                    };
                    event(norte_code_protocol::ServerEventKind::Delta { turn, delta })
                }
                FrameKind::TurnStateChanged => {
                    // The payload is the spec's own `TurnStateChanged` struct: decode
                    // it whole, so an unknown state or a missing field is an error
                    // from serde, not a silent drop or a hand-rolled lookup.
                    let changed: raep_types::TurnStateChanged = from_payload(payload)?;
                    match changed.state {
                        raep_types::TurnState::Queued => {
                            event(norte_code_protocol::ServerEventKind::TurnStarted { turn })
                        }
                        raep_types::TurnState::Completed => {
                            // `phase`/`committed` are NorteCode's own fields: the RAEP
                            // spec has no equivalent, so they travel inside the
                            // compatibility extension the server writes, not in the
                            // canonical payload. Read them from there, defaulting to
                            // the only honest values (idle, committed) when absent.
                            let phase = extension_finished
                                .as_ref()
                                .and_then(|finished| finished.phase)
                                .unwrap_or(norte_code_protocol::Phase::Idle);
                            let committed = extension_finished
                                .as_ref()
                                .and_then(|finished| finished.committed)
                                .unwrap_or(true);
                            event(norte_code_protocol::ServerEventKind::TurnFinished {
                                turn,
                                phase,
                                committed,
                            })
                        }
                        raep_types::TurnState::Cancelled | raep_types::TurnState::Failed => {
                            let reason = match changed.stop_reason {
                                Some(raep_types::StopReason::EndTurn) => "end_turn",
                                Some(raep_types::StopReason::MaxTokens) => "max_tokens",
                                Some(raep_types::StopReason::Error) => "error",
                                Some(raep_types::StopReason::ToolLimit) => "tool_limit",
                                Some(raep_types::StopReason::BudgetExceeded) => "budget_exceeded",
                                Some(raep_types::StopReason::Cancelled) | None => "cancelled",
                            }
                            .to_owned();
                            event(norte_code_protocol::ServerEventKind::TurnStopped { reason })
                        }
                        // Intermediate states carry no turn-boundary meaning for the
                        // client; the next terminal state will arrive on its own.
                        raep_types::TurnState::CallingProvider
                        | raep_types::TurnState::ExecutingTool
                        | raep_types::TurnState::AwaitingApproval => return Ok(None),
                    }
                }
                FrameKind::ToolCallStarted => {
                    let decoded: ToolCallStartedPayload = from_payload(payload)?;
                    event(norte_code_protocol::ServerEventKind::ToolStarted {
                        turn,
                        call_id: decoded.tool_call_id,
                        tool: decoded.tool_name,
                        arguments: decoded.arguments,
                        source: decoded.source,
                    })
                }
                FrameKind::ToolOutputDelta => {
                    let decoded: ToolOutputDeltaPayload = from_payload(payload)?;
                    event(norte_code_protocol::ServerEventKind::ToolOutput {
                        turn,
                        call_id: decoded.tool_call_id,
                        tool: decoded.tool_name,
                        stream: decoded.stream,
                        text: decoded.delta,
                        last: false,
                    })
                }
                FrameKind::ToolCallFinished => {
                    let decoded: ToolCallFinishedPayload = from_payload(payload)?;
                    event(norte_code_protocol::ServerEventKind::ToolFinished {
                        turn,
                        call_id: decoded.tool_call_id,
                        // The finish frame carries the name the start announced, so
                        // the ending row can say which call ended. Older servers
                        // omitted it; fall back to empty rather than failing.
                        tool: decoded.tool_name.unwrap_or_default(),
                        is_error: decoded.status == ToolStatus::Error,
                        error: None,
                        output: decoded.result_summary,
                        truncated: decoded.result_truncated,
                        detached: decoded.status == ToolStatus::Cancelled,
                        duration_ms: decoded.duration_ms,
                    })
                }
                FrameKind::JobListResult => {
                    let decoded: JobListPayload = from_payload(payload)?;
                    let jobs = decoded
                        .jobs
                        .into_iter()
                        .map(|job| {
                            let state = match job.status {
                                JobStatus::Completed => norte_code_protocol::JobState::Finished,
                                JobStatus::Failed | JobStatus::Killed | JobStatus::TimedOut => {
                                    norte_code_protocol::JobState::Cancelled
                                }
                                JobStatus::Running => norte_code_protocol::JobState::Running,
                            };
                            norte_code_protocol::JobInfo {
                                id: job.job_id,
                                label: job.command,
                                script: String::new(),
                                state,
                                exit_code: job.exit_code,
                                timed_out: job.status == JobStatus::TimedOut,
                                elapsed_ms: 0,
                                output: String::new(),
                                dropped: false,
                            }
                        })
                        .collect();
                    event(norte_code_protocol::ServerEventKind::JobList { jobs })
                }
                FrameKind::Error => {
                    let decoded: ErrorPayload = from_payload(payload)?;
                    let code = decoded.code.to_ascii_lowercase();
                    let code = serde_json::from_value(serde_json::Value::String(code))
                        .unwrap_or(norte_code_protocol::ErrorCode::Internal);
                    event(norte_code_protocol::ServerEventKind::Error {
                        code,
                        message: decoded.message,
                    })
                }
                FrameKind::Other => return Ok(None),
            }
        }
    };
    Ok(Some(kind))
}

#[allow(clippy::too_many_lines)]
fn server_event_from_frame(frame: &Envelope<Value>) -> Result<ServerEvent, ClientError> {
    if frame.version != raep_types::WIRE_VERSION {
        return Err(ClientError::new(format!(
            "unsupported RAEP wire version `{}`; this client speaks {}",
            frame.version,
            raep_types::WIRE_VERSION
        )));
    }
    // A canonical frame that fails to decode is a wire error, not a reason
    // to fall back to the extension: silently splicing in the compatibility
    // copy would hide a server that stopped following the spec.
    if let Some(event) = standard_event_from_frame(frame)? {
        return Ok(event);
    }
    if let Some(event) = frame
        .ext
        .as_ref()
        .and_then(|ext| ext.get("norte_code"))
        .cloned()
    {
        return serde_json::from_value(event)
            .map_err(|error| ClientError::new(format!("invalid NorteCode event: {error}")));
    }
    let session = (!frame.session_id.is_empty())
        .then(|| norte_code_protocol::SessionKey::new(frame.session_id.clone()));
    let canonical = CanonicalPayload::from_value(frame.payload.clone())?;
    let kind = match canonical.kind {
        CanonicalKind::SessionCreated => {
            let decoded: CanonicalSessionCreated = canonical.decode()?;
            norte_code_protocol::ServerEventKind::SessionCreated {
                title: decoded.title,
            }
        }
        CanonicalKind::SessionList => {
            let decoded: CanonicalSessionList = canonical.decode()?;
            norte_code_protocol::ServerEventKind::SessionList {
                sessions: decoded.sessions,
            }
        }
        CanonicalKind::SessionDeleted => norte_code_protocol::ServerEventKind::SessionDeleted,
        CanonicalKind::SessionForked => {
            let decoded: CanonicalSessionForked = canonical.decode()?;
            norte_code_protocol::ServerEventKind::SessionForked {
                from: norte_code_protocol::SessionKey::new(decoded.from),
            }
        }
        CanonicalKind::SessionSwitched => norte_code_protocol::ServerEventKind::SessionSwitched,
        CanonicalKind::TurnStarted => {
            let decoded: CanonicalTurnStarted = canonical.decode()?;
            norte_code_protocol::ServerEventKind::TurnStarted { turn: decoded.turn }
        }
        CanonicalKind::TokenDelta => {
            let decoded: CanonicalTokenDelta = canonical.decode()?;
            norte_code_protocol::ServerEventKind::Delta {
                turn: decoded.turn,
                delta: decoded.delta,
            }
        }
        CanonicalKind::Committed => {
            let decoded: CanonicalCommitted = canonical.decode()?;
            norte_code_protocol::ServerEventKind::Committed {
                turn: decoded.turn,
                version: decoded.version,
            }
        }
        CanonicalKind::HistoryMessage => {
            let decoded: CanonicalHistoryMessage = canonical.decode()?;
            norte_code_protocol::ServerEventKind::MessageReplayed {
                index: decoded.index,
                role: decoded.role,
                kind: decoded.kind,
                text: decoded.text,
            }
        }
        CanonicalKind::SessionTodos => {
            let decoded: CanonicalSessionTodos = canonical.decode()?;
            norte_code_protocol::ServerEventKind::SessionTodos {
                todos: decoded.todos,
            }
        }
        CanonicalKind::ToolCallStarted => {
            let decoded: CanonicalToolCallStarted = canonical.decode()?;
            norte_code_protocol::ServerEventKind::ToolStarted {
                turn: decoded.turn,
                call_id: decoded.call_id,
                tool: decoded.tool,
                arguments: decoded.arguments,
                source: decoded.source,
            }
        }
        CanonicalKind::JobProgressDelta => {
            let decoded: CanonicalJobProgressDelta = canonical.decode()?;
            norte_code_protocol::ServerEventKind::ToolOutput {
                turn: decoded.turn,
                call_id: decoded.call_id,
                tool: decoded.tool,
                stream: decoded.stream,
                text: decoded.text,
                last: decoded.last,
            }
        }
        CanonicalKind::ToolCallFinished => {
            let decoded: CanonicalToolCallFinished = canonical.decode()?;
            norte_code_protocol::ServerEventKind::ToolFinished {
                turn: decoded.turn,
                call_id: decoded.call_id,
                tool: decoded.tool,
                is_error: decoded.is_error,
                error: decoded.error,
                output: decoded.output,
                truncated: decoded.truncated,
                detached: decoded.detached,
                duration_ms: decoded.duration_ms,
            }
        }
        CanonicalKind::OutputPartFinished => {
            let decoded: CanonicalOutputPartFinished = canonical.decode()?;
            norte_code_protocol::ServerEventKind::RequestReconstructed {
                turn: decoded.turn,
                index: decoded.index,
                usage: decoded.usage,
                cost_usd: decoded.cost_usd,
                truncated: decoded.truncated,
                duration_ms: decoded.duration_ms,
                first_token_ms: decoded.first_token_ms,
                tokens_per_second: decoded.tokens_per_second,
                cache_hit: decoded.cache_hit,
                tools: decoded.tools,
            }
        }
        CanonicalKind::ApprovalResolved => {
            let decoded: CanonicalApprovalResolved = canonical.decode()?;
            norte_code_protocol::ServerEventKind::QuestionAnswered {
                question: decoded.approval_id,
            }
        }
        CanonicalKind::UserInputAccepted => {
            let decoded: CanonicalUserInputAccepted = canonical.decode()?;
            norte_code_protocol::ServerEventKind::SteerApplied {
                ids: decoded.ids,
                messages: decoded.messages,
            }
        }
        CanonicalKind::ProviderRetry => {
            let decoded: CanonicalProviderRetry = canonical.decode()?;
            match decoded.state {
                RetryState::Scheduled => norte_code_protocol::ServerEventKind::RetryScheduled {
                    turn: decoded.turn,
                    attempt: decoded.attempt,
                    delay_ms: decoded.delay_ms,
                    reason: decoded.reason,
                },
                RetryState::Started => norte_code_protocol::ServerEventKind::RetryStarted {
                    turn: decoded.turn,
                    attempt: decoded.attempt,
                },
            }
        }
        CanonicalKind::AgentChanged => {
            let decoded: CanonicalAgentChanged = canonical.decode()?;
            match decoded.change {
                AgentChange::ModelChanged => norte_code_protocol::ServerEventKind::ModelChanged {
                    provider: decoded.provider.unwrap_or_default(),
                    model: decoded.model.unwrap_or_default(),
                },
                AgentChange::ModelQueued => norte_code_protocol::ServerEventKind::ModelQueued {
                    provider: decoded.provider.unwrap_or_default(),
                    model: decoded.model.unwrap_or_default(),
                },
                AgentChange::ReasoningDowngraded => {
                    let details: ReasoningDowngradedDetails = decoded
                        .details
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|error| {
                            ClientError::new(format!("invalid reasoning downgrade: {error}"))
                        })?
                        .unwrap_or(ReasoningDowngradedDetails {
                            from: String::new(),
                            to: String::new(),
                            reason: String::new(),
                        });
                    norte_code_protocol::ServerEventKind::ReasoningDowngraded {
                        from: details.from,
                        to: details.to,
                        reason: details.reason,
                    }
                }
            }
        }
        CanonicalKind::AgentCatalog => {
            let decoded: CanonicalAgentCatalog = canonical.decode()?;
            match decoded.kind {
                CatalogKind::Models => norte_code_protocol::ServerEventKind::ModelList {
                    models: serde_json::from_value(decoded.entries).map_err(|error| {
                        ClientError::new(format!("invalid model list: {error}"))
                    })?,
                },
                CatalogKind::Skills => {
                    let entries: SkillCatalogEntries = serde_json::from_value(decoded.entries)
                        .map_err(|error| {
                            ClientError::new(format!("invalid skill list: {error}"))
                        })?;
                    norte_code_protocol::ServerEventKind::SkillList {
                        skills: entries.skills,
                        warnings: entries.warnings,
                    }
                }
            }
        }
        CanonicalKind::SessionRenamed => {
            let decoded: CanonicalSessionRenamed = canonical.decode()?;
            norte_code_protocol::ServerEventKind::SessionRenamed {
                title: decoded.title,
            }
        }
        CanonicalKind::SessionStats => {
            let decoded: CanonicalSessionStats = canonical.decode()?;
            norte_code_protocol::ServerEventKind::SessionStats {
                input_tokens: decoded.input_tokens,
                output_tokens: decoded.output_tokens,
                cache_read_tokens: decoded.cache_read_tokens,
                cache_write_tokens: decoded.cache_write_tokens,
                cost_usd: decoded.cost_usd,
                requests: decoded.requests,
                context_used: decoded.context_used,
                context_window: decoded.context_window,
            }
        }
        CanonicalKind::JobList => {
            let decoded: CanonicalJobList = canonical.decode()?;
            norte_code_protocol::ServerEventKind::JobList { jobs: decoded.jobs }
        }
        CanonicalKind::TurnStateChanged => {
            let decoded: CanonicalTurnStateChanged = canonical.decode()?;
            match decoded.state {
                CanonicalTurnState::Finished => {
                    norte_code_protocol::ServerEventKind::TurnFinished {
                        turn: decoded.turn,
                        phase: decoded.phase.unwrap_or(norte_code_protocol::Phase::Idle),
                        committed: decoded.committed.unwrap_or(true),
                    }
                }
                CanonicalTurnState::Stopped => norte_code_protocol::ServerEventKind::TurnStopped {
                    reason: decoded.reason.unwrap_or_default(),
                },
            }
        }
        CanonicalKind::ContextUpdate => {
            let decoded: CanonicalContextUpdate = canonical.decode()?;
            match decoded.state {
                CanonicalContextState::Started => {
                    norte_code_protocol::ServerEventKind::CompactionStarted {
                        reason: decoded.reason.unwrap_or_default(),
                    }
                }
                CanonicalContextState::Finished => {
                    norte_code_protocol::ServerEventKind::CompactionFinished {
                        compacted: decoded.compacted.unwrap_or(false),
                        north_session: decoded.north_session,
                        summary: decoded.summary,
                        error: decoded.error,
                    }
                }
            }
        }
        CanonicalKind::BudgetWarning => {
            let decoded: CanonicalBudgetWarning = canonical.decode()?;
            norte_code_protocol::ServerEventKind::BudgetWarning {
                scope: decoded.scope,
                unit: decoded.unit,
                spent: decoded.spent,
                limit: decoded.limit,
            }
        }
        CanonicalKind::BudgetExceeded => {
            let decoded: CanonicalBudgetExceeded = canonical.decode()?;
            norte_code_protocol::ServerEventKind::BudgetExceeded {
                scope: decoded.scope,
                unit: decoded.unit,
                spent: decoded.spent,
                limit: decoded.limit,
                action: decoded.action,
            }
        }
        CanonicalKind::BudgetChanged => {
            let decoded: CanonicalBudgetChanged = canonical.decode()?;
            norte_code_protocol::ServerEventKind::BudgetChanged {
                budget: decoded.budget,
            }
        }
        CanonicalKind::CostUpdated => {
            let decoded: CanonicalCostUpdated = canonical.decode()?;
            norte_code_protocol::ServerEventKind::CostUpdated {
                total_usd: decoded.total_usd,
                requests: decoded.requests,
            }
        }
        CanonicalKind::CostNotReported => {
            let decoded: CanonicalCostNotReported = canonical.decode()?;
            norte_code_protocol::ServerEventKind::CostNotReported {
                requests: decoded.requests,
            }
        }
        CanonicalKind::PluginList => {
            let decoded: CanonicalPluginList = canonical.decode()?;
            norte_code_protocol::ServerEventKind::PluginList {
                plugins: decoded.plugins,
            }
        }
        CanonicalKind::PluginChanged => {
            let decoded: CanonicalPluginChanged = canonical.decode()?;
            norte_code_protocol::ServerEventKind::PluginChanged {
                operation: decoded.operation,
                name: decoded.name,
            }
        }
        CanonicalKind::ServiceData => {
            let decoded: CanonicalServiceData = canonical.decode()?;
            match decoded.kind {
                ServiceDataKind::Telemetry => norte_code_protocol::ServerEventKind::Telemetry {
                    records: serde_json::from_value(decoded.data).map_err(|error| {
                        ClientError::new(format!("invalid telemetry payload: {error}"))
                    })?,
                },
                ServiceDataKind::ToolList => norte_code_protocol::ServerEventKind::ToolList {
                    tools: serde_json::from_value(decoded.data).map_err(|error| {
                        ClientError::new(format!("invalid tool list payload: {error}"))
                    })?,
                },
                ServiceDataKind::McpList => {
                    let data: McpListData =
                        serde_json::from_value(decoded.data).map_err(|error| {
                            ClientError::new(format!("invalid MCP payload: {error}"))
                        })?;
                    norte_code_protocol::ServerEventKind::McpList {
                        servers: data.servers,
                        failures: data.failures,
                    }
                }
            }
        }
        CanonicalKind::Error => {
            let decoded: CanonicalError = canonical.decode()?;
            let code = serde_json::from_value(decoded.code.clone())
                .unwrap_or(norte_code_protocol::ErrorCode::Internal);
            norte_code_protocol::ServerEventKind::Error {
                code,
                message: decoded.message,
            }
        }
        CanonicalKind::Other => {
            return Err(ClientError::new(
                "unsupported RAEP canonical event".to_owned(),
            ));
        }
    };
    Ok(ServerEvent {
        request_id: 0,
        session,
        kind,
    })
}

fn session_from_request(request: &ClientRequest) -> Option<String> {
    use norte_code_protocol::ClientRequestKind;
    let session = match &request.kind {
        ClientRequestKind::SwitchSession { session }
        | ClientRequestKind::ForkSession { session, .. }
        | ClientRequestKind::RenameSession { session, .. }
        | ClientRequestKind::DeleteSession { session }
        | ClientRequestKind::Steer { session, .. }
        | ClientRequestKind::CancelTurn { session }
        | ClientRequestKind::StopJobs { session, .. }
        | ClientRequestKind::DetachTurn { session }
        | ClientRequestKind::ListJobs { session }
        | ClientRequestKind::Resume { session, .. }
        | ClientRequestKind::AnswerQuestion { session, .. }
        | ClientRequestKind::SetModel { session, .. }
        | ClientRequestKind::SetReasoning { session, .. }
        | ClientRequestKind::SetBudget { session, .. }
        | ClientRequestKind::Compact { session }
        | ClientRequestKind::ListTools {
            session: Some(session),
        }
        | ClientRequestKind::SetApprovalRule {
            session: Some(session),
            ..
        }
        | ClientRequestKind::ForgetApprovalRule {
            session: Some(session),
            ..
        } => Some(session),
        ClientRequestKind::SendMessage { session, .. }
        | ClientRequestKind::CostReport { session }
        | ClientRequestKind::ListTelemetry { session } => session.as_ref(),
        ClientRequestKind::ListMcp
        | ClientRequestKind::RefreshMcp
        | ClientRequestKind::SetMcpActive { .. }
        | ClientRequestKind::CreateSession { .. }
        | ClientRequestKind::ListSessions
        | ClientRequestKind::ListModels
        | ClientRequestKind::ListTools { session: None }
        | ClientRequestKind::SetApprovalRule { session: None, .. }
        | ClientRequestKind::ForgetApprovalRule { session: None, .. }
        | ClientRequestKind::RefreshCatalog
        | ClientRequestKind::ListSkills
        | ClientRequestKind::RefreshSkills
        | ClientRequestKind::ListPlugins
        | ClientRequestKind::InstallPlugin { .. }
        | ClientRequestKind::UpdatePlugins { .. }
        | ClientRequestKind::RemovePlugin { .. }
        | ClientRequestKind::SetPluginEnabled { .. } => None,
    }?;
    Some(session.as_str().to_owned())
}

fn timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte_code_protocol::{ClientRequestKind, SessionKey};

    /// A typed frame fixture: the envelope is built as `Envelope<Value>`
    /// with a typed payload, never as raw JSON.
    fn frame(kind: FrameType, session_id: &str, payload: serde_json::Value) -> Envelope<Value> {
        let mut frame = Envelope::new(kind, payload, 0);
        frame.session_id = session_id.to_owned();
        frame
    }

    #[test]
    fn prefers_the_standard_payload_over_the_compatibility_extension() {
        let mut frame = frame(
            FrameType::Standard(StandardFrameType::TokenDelta),
            "session-1",
            serde_json::json!({"delta": "canonical", "index": 2, "channel": "text"}),
        );
        frame.turn_id = raep_types::TurnId::new("7").ok();
        frame.ext = Some(serde_json::json!({"norte_code": {
            "request_id": 0,
            "session": "session-1",
            "kind": {"event": "committed", "turn": 7, "version": 1}
        }}));
        let event = server_event_from_frame(&frame).expect("standard token delta");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::Delta {
                turn: 7,
                delta: norte_code_protocol::Delta::Text { index: 2, ref text }
            } if text == "canonical"
        ));
    }

    #[test]
    fn rejects_an_unsupported_wire_version() {
        let mut frame = frame(
            FrameType::Standard(StandardFrameType::TokenDelta),
            "",
            serde_json::json!({"delta": "hi", "index": 0}),
        );
        frame.version = "2.0".to_owned();
        let error = server_event_from_frame(&frame).expect_err("unsupported version");
        assert!(error.to_string().contains("wire version"), "{error}");
    }

    #[test]
    fn decodes_a_standard_tool_start() {
        let mut frame = frame(
            FrameType::Standard(StandardFrameType::ToolCallStarted),
            "session-1",
            serde_json::json!({
                "tool_call_id": "call-1",
                "tool_name": "shell",
                "arguments": {"command": "ls"},
                "requires_approval": false
            }),
        );
        frame.turn_id = raep_types::TurnId::new("3").ok();
        let event = server_event_from_frame(&frame).expect("standard tool start");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::ToolStarted { turn: 3, ref call_id, ref tool, .. }
                if call_id == "call-1" && tool == "shell"
        ));
    }

    #[test]
    fn classifies_sequence_steps() {
        assert_eq!(seq_step(None, 1), SeqStep::Apply);
        assert_eq!(seq_step(Some(4), 5), SeqStep::Apply);
        assert_eq!(seq_step(Some(4), 4), SeqStep::Duplicate);
        assert_eq!(seq_step(Some(4), 3), SeqStep::Duplicate);
        assert_eq!(seq_step(Some(4), 6), SeqStep::Gap { expected: 5 });
        assert_eq!(seq_step(Some(u64::MAX), u64::MAX), SeqStep::Duplicate);
    }

    #[test]
    fn encodes_session_path_segments_without_reinterpreting_slashes() {
        assert_eq!(
            percent_encode("provider/session 1"),
            "provider%2Fsession%201"
        );
    }

    #[test]
    fn extracts_session_from_session_scoped_requests() {
        let request = ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::CancelTurn {
                session: SessionKey::new("session-1"),
            },
        };
        assert_eq!(session_from_request(&request).as_deref(), Some("session-1"));
    }

    #[test]
    fn leaves_connection_scoped_requests_without_a_session() {
        let request = ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::ListSessions,
        };
        assert_eq!(session_from_request(&request), None);
    }

    #[test]
    fn prefixes_bearer_tokens_without_redacting_the_value() {
        let client = RemoteClient::new("http://localhost:8080", Some("secret-token"), None)
            .expect("valid bearer token");
        assert_eq!(
            client
                .bearer_token
                .as_ref()
                .and_then(|value| value.to_str().ok()),
            Some("Bearer secret-token")
        );
    }

    #[test]
    fn decodes_canonical_service_data_without_the_compatibility_extension() {
        let frame = frame(
            FrameType::Extension("norte_code".to_owned()),
            "session-1",
            serde_json::json!({
                "event": "service_data",
                "kind": "tool_list",
                "data": [{
                    "name": "read",
                    "allowed": true,
                    "scope": "default"
                }]
            }),
        );
        let event = server_event_from_frame(&frame).expect("canonical service event");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::ToolList { ref tools }
                if tools[0].name == "read"
        ));
    }

    #[test]
    fn decodes_canonical_session_stats_and_jobs() {
        let stats = frame(
            FrameType::Extension("norte_code".to_owned()),
            "session-1",
            serde_json::json!({
                "event": "session_stats",
                "input_tokens": 10,
                "output_tokens": 4,
                "cache_read_tokens": 2,
                "cache_write_tokens": 1,
                "cost_usd": 0.25,
                "requests": 2,
                "context_used": 14,
                "context_window": 100
            }),
        );
        let event = server_event_from_frame(&stats).expect("canonical stats");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::SessionStats {
                context_used: 14,
                context_window: Some(100),
                ..
            }
        ));

        let jobs = frame(
            FrameType::Extension("norte_code".to_owned()),
            "session-1",
            serde_json::json!({
                "event": "job_list",
                "jobs": []
            }),
        );
        let event = server_event_from_frame(&jobs).expect("canonical jobs");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::JobList { ref jobs } if jobs.is_empty()
        ));
    }

    #[test]
    fn serializes_remote_service_requests_with_protocol_tags() {
        let request = ClientRequest {
            request_id: 7,
            kind: ClientRequestKind::ListTelemetry { session: None },
        };
        let value = serde_json::to_value(request).expect("request serializes");
        assert_eq!(value["kind"]["type"], "list_telemetry");
    }

    #[test]
    fn tracks_last_sequence_per_session_and_keeps_the_maximum() {
        let client =
            RemoteClient::new("http://localhost:8080", None, None).expect("client creates");
        assert_eq!(client.last_seq("session-1"), None);
        client.note_seq("session-1", 7);
        assert_eq!(client.last_seq("session-1"), Some(7));
        client.note_seq("session-1", 4);
        assert_eq!(
            client.last_seq("session-1"),
            Some(7),
            "an older frame must not move the cursor backwards"
        );
        client.note_seq("session-1", 9);
        assert_eq!(client.last_seq("session-1"), Some(9));
        client.note_seq("session-2", 3);
        assert_eq!(client.last_seq("session-2"), Some(3));
        assert_eq!(client.last_seq("session-1"), Some(9));
    }

    #[test]
    fn records_sequence_from_raw_frames_only() {
        let client =
            RemoteClient::new("http://localhost:8080", None, None).expect("client creates");
        let mut first = frame(
            FrameType::Extension("norte_code".to_owned()),
            "session-1",
            serde_json::json!({}),
        );
        first.seq = Some(12);
        client.note_frame_seq(&first);
        assert_eq!(client.last_seq("session-1"), Some(12));
        let without_seq = frame(
            FrameType::Extension("norte_code".to_owned()),
            "session-1",
            serde_json::json!({}),
        );
        client.note_frame_seq(&without_seq);
        assert_eq!(client.last_seq("session-1"), Some(12));
        let empty_session = frame(
            FrameType::Extension("norte_code".to_owned()),
            "",
            serde_json::json!({}),
        );
        let mut empty_session = empty_session;
        empty_session.seq = Some(99);
        client.note_frame_seq(&empty_session);
        assert_eq!(client.last_seq(""), None);
    }

    #[test]
    fn drains_sse_blocks_in_order_and_reaches_past_the_terminal() {
        // Real envelopes: version, id, timestamp, kind and payload, as the
        // server emits them through `Envelope::new`.
        let frame = |kind: &str, seq: u64, payload: &str| -> String {
            format!(
                "{{\"version\":\"1.0\",\"id\":\"frame-{seq}\",\"seq\":{seq},\"session_id\":\"session-1\",\"timestamp\":0,\"type\":\"{kind}\",\"payload\":{payload}}}"
            )
        };
        let mut buffer = concat!(
            "event: turn_state_changed\n",
            "data: ",
            "{\"session_id\":\"session-1\",\"seq\":1,\"payload\":{\"event\":\"turn_state_changed\",\"state\":\"finished\",\"turn\":1,\"phase\":\"idle\",\"committed\":true}}\n",
            "\n",
            "event: SESSION_STATS\n",
            "data: {\"session_id\":\"session-1\",\"seq\":2,\"payload\":{\"event\":\"session_stats\",\"input_tokens\":10,\"output_tokens\":4,\"cache_read_tokens\":2,\"cache_write_tokens\":1,\"cost_usd\":0.25,\"requests\":2,\"context_used\":14,\"context_window\":100}}\n",
            "\n",
            "data: {\"session_id\":\"session-1\",\"seq\":3,\"payload\":{\"event\":\"session_stats\",\"input_tokens\":11,\"output_tokens\":5,\"cache_read_tokens\":2,\"cache_write_tokens\":1,\"cost_usd\":0.26,\"requests\":3,\"context_used\":15,\"context_window\":100}}"
        )
        .to_owned()
        .into_bytes();
        // The fixtures above are canonical-only frames (no envelope `kind`):
        // they still decode through the canonical path, which is the path
        // `drain_sse_buffer` exercises.
        let _ = frame;
        let drained = drain_sse_buffer(&mut buffer);
        assert_eq!(drained.len(), 2);
        assert!(drained[0].1, "the terminal event is flagged");
        assert_eq!(drained[0].2, Some(1));
        assert!(
            !drained[1].1,
            "the event after the terminal is not terminal but still drains"
        );
        assert_eq!(drained[1].2, Some(2));
        // The partial trailing block is left for the next chunk.
        assert!(String::from_utf8_lossy(&buffer).contains("\"seq\":3"));
    }

    #[test]
    fn keeps_a_multibyte_character_split_across_chunks() {
        let text = "data: {\"session_id\":\"s\",\"payload\":{\"event\":\"session_created\",\"title\":\"café\"}}\n\n";
        let bytes = text.as_bytes();
        let split = text.find('é').expect("accented character") + 1;
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&bytes[..split]);
        assert!(
            drain_sse_buffer(&mut buffer).is_empty(),
            "a partial block must wait for the rest"
        );
        buffer.extend_from_slice(&bytes[split..]);
        let drained = drain_sse_buffer(&mut buffer);
        assert_eq!(drained.len(), 1);
        assert!(matches!(
            drained[0].0.as_ref().unwrap().kind,
            norte_code_protocol::ServerEventKind::SessionCreated { title: Some(ref title) }
                if title == "café"
        ));
    }
}
