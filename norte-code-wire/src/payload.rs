//! The canonical payloads the host writes and every client reads.
//!
//! The host used to keep these as private `Serialize` structs and each client
//! kept its own `Deserialize` copy of the same fields. That is how a renamed
//! field became an empty list on one client and a working screen on another:
//! nothing forced the two to agree. They are defined once here, both ways, and
//! every side imports them.
//!
//! Fields are owned and `#[serde(default)]` where the host may omit them, so a
//! client built before a field existed still decodes a frame that carries it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ChildState, OutputStream, TodoInfo};

/// Which channel a token delta belongs to.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenChannel {
    /// Answer text.
    #[default]
    Text,
    /// The model's reasoning.
    Reasoning,
    /// Fragments of a tool call's JSON arguments. They are not answer text:
    /// a client that draws them as prose shows the person raw JSON.
    #[serde(alias = "tool_arguments")]
    ToolArguments,
}

/// A `TOKEN_DELTA` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TokenDelta {
    /// The text that arrived.
    pub delta: String,
    /// Its position within the turn.
    #[serde(default)]
    pub index: u64,
    /// The channel it belongs to; absent means text.
    #[serde(default)]
    pub channel: TokenChannel,
}

/// Who executed a tool call.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolExecutor {
    /// The server ran it.
    #[default]
    Server,
}

/// How a finished tool call stood.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolStatus {
    /// It returned a result.
    #[default]
    Ok,
    /// It failed.
    Error,
    /// It was handed to a job and stopped here.
    Cancelled,
}

impl ToolStatus {
    /// How a finished call stood, from the two flags the engine reports.
    ///
    /// The two facts arrive separately — `detached` says the call was handed
    /// to a job, `is_error` says it failed — and the wire has one field for
    /// both. Deciding it here means every producer agrees, instead of each
    /// call site re-deciding which flag wins.
    #[must_use]
    pub const fn from_outcome(detached: bool, is_error: bool) -> Self {
        if detached {
            Self::Cancelled
        } else if is_error {
            Self::Error
        } else {
            Self::Ok
        }
    }
}

/// A `TOOL_CALL_STARTED` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ToolStarted {
    /// The id this call is addressed by.
    pub tool_call_id: String,
    /// The tool's name.
    pub tool_name: String,
    /// Its arguments, as the model wrote them.
    #[serde(default)]
    pub arguments: Value,
    /// Where the call came from, when the host says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Whether it waits for an approval before running.
    #[serde(default)]
    pub requires_approval: bool,
    /// Who runs it.
    #[serde(default)]
    pub executor: ToolExecutor,
    /// Which attempt this is, for a retried call.
    #[serde(default)]
    pub attempt: u32,
}

/// A `TOOL_CALL_FINISHED` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ToolFinished {
    /// The call that finished.
    pub tool_call_id: String,
    /// The tool's name.
    #[serde(default)]
    pub tool_name: String,
    /// How it stood.
    #[serde(default)]
    pub status: ToolStatus,
    /// How long it took, in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// A one-line result, or the error.
    #[serde(default)]
    pub result_summary: Option<String>,
    /// Whether the result was cut at the host's bound.
    #[serde(default)]
    pub result_truncated: bool,
}

/// A `TOOL_OUTPUT_DELTA` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ToolOutput {
    /// The call whose output this is.
    pub tool_call_id: String,
    /// The tool's name.
    #[serde(default)]
    pub tool_name: String,
    /// Which stream: standard output or standard error.
    #[serde(default)]
    pub stream: OutputStream,
    /// The text that arrived.
    #[serde(default)]
    pub delta: String,
    /// Its position within the output.
    #[serde(default)]
    pub index: u64,
}

/// A detached tool call became a job.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct JobStarted {
    /// The job's id.
    pub job_id: String,
    /// The call it came from.
    #[serde(default)]
    pub tool_call_id: String,
    /// The script it runs.
    #[serde(default)]
    pub command: String,
    /// Whether it outlived the call that started it.
    #[serde(default)]
    pub detached: bool,
    /// When it started, in milliseconds since the epoch.
    #[serde(default)]
    pub started_at: u64,
    /// The bounds the runner was given, when it has any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<crate::JobLimits>,
}

/// One output chunk from a detached job.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct JobProgress {
    /// Which stream: standard output or standard error.
    #[serde(default)]
    pub stream: OutputStream,
    /// The text that arrived.
    #[serde(default)]
    pub delta: String,
    /// Its position within the output.
    #[serde(default)]
    pub index: u64,
}

/// A `QUESTION_ASKED` payload: the turn is parked until someone answers.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct QuestionAsked {
    /// The turn waiting on it.
    #[serde(default)]
    pub turn_id: u64,
    /// The question's id.
    pub question: String,
    /// What to show the person.
    #[serde(default)]
    pub prompt: String,
    /// The choices, when there are any.
    #[serde(default)]
    pub options: Vec<String>,
    /// Whether more than one choice may be picked.
    #[serde(default)]
    pub multiple: bool,
}

/// A `QUESTION_ANSWERED` payload: the question this answer closes.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct QuestionAnswered {
    /// What was closed. The extension names it `approval_id` in some frames.
    #[serde(default, alias = "approval_id")]
    pub question: String,
}

/// A `SESSION_TODOS` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SessionTodos {
    /// The session's task list, in the model's order.
    #[serde(default)]
    pub todos: Vec<TodoInfo>,
}

/// A `REASONING_DOWNGRADED` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct ReasoningDowngraded {
    /// The setting that was asked for.
    pub from: String,
    /// The setting the model will use.
    pub to: String,
    /// Why it changed.
    pub reason: String,
}

/// A `JOBS_STOPPED` payload.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct JobsStopped {
    /// How many jobs were stopped.
    pub stopped: usize,
}

/// Why a compaction ran.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionTrigger {
    /// The context grew past its bound.
    #[default]
    Auto,
}

/// A finished compaction, as the canonical `context_update` payload spells it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CompactionFinished {
    /// Why it ran.
    #[serde(default)]
    pub trigger: CompactionTrigger,
    /// What replaced the history, when it compacted.
    #[serde(default)]
    pub summary: Option<String>,
    /// Why it did not happen, when it did not.
    #[serde(default)]
    pub error: Option<String>,
    /// Whether it happened.
    #[serde(default, alias = "compacted")]
    pub compacted: Option<bool>,
}

/// A `SESSION_STATS` payload.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SessionStats {
    /// Input tokens used by the session.
    #[serde(default)]
    pub input_tokens: u64,
    /// Output tokens produced by the session.
    #[serde(default)]
    pub output_tokens: u64,
    /// Provider-reported cost, in US dollars.
    #[serde(default)]
    pub cost_usd: f64,
    /// How many model requests it made.
    #[serde(default)]
    pub requests: u64,
}

/// A `CHILD_SESSION_STARTED` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct ChildSessionStarted {
    /// The child's session id.
    pub child_session_id: String,
    /// The agent it runs.
    #[serde(default)]
    pub agent_id: String,
    /// The call that started it.
    #[serde(default)]
    pub tool_call_id: String,
    /// How long it is meant to live.
    #[serde(default)]
    pub lifetime: String,
    /// What the parent asked it to do.
    #[serde(default)]
    pub task_summary: String,
}

/// A `CHILD_SESSION_UPDATED` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ChildSessionUpdated {
    /// The child whose state changed.
    pub child_session_id: String,
    /// Where it stands.
    #[serde(default)]
    pub state: ChildState,
    /// Why it changed.
    #[serde(default)]
    pub reason: String,
    /// Its result, when it ended.
    #[serde(default)]
    pub result: Option<Value>,
    /// Whether the result matched the output schema, when one was asked for.
    #[serde(default)]
    pub output_schema_valid: Option<bool>,
}

/// A `DELEGATOR_STATE_CHANGED` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct DelegatorStateChanged {
    /// The child whose parent changed.
    pub child_session_id: String,
    /// Whether the parent is suspended.
    #[serde(default)]
    pub suspended: bool,
    /// Who suspended it, when the host says.
    #[serde(default)]
    pub by: Option<String>,
    /// How many messages are waiting for it.
    #[serde(default)]
    pub queued_messages: Option<usize>,
}

/// A `CHILD_AWAITING_INPUT` payload: a child needs its parent's answer.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct ChildAwaitingInput {
    /// The child waiting.
    pub child_session_id: String,
    /// The question's id.
    #[serde(default)]
    pub question: String,
    /// What to show the person.
    #[serde(default)]
    pub prompt: String,
}

/// A `SESSION_FORKED` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionForked {
    /// The session it was forked from.
    #[serde(default, alias = "from")]
    pub from_session_id: Option<String>,
    /// The turn it was forked at.
    #[serde(default)]
    pub from_turn_id: Option<String>,
    /// The new session's title, when one was given.
    #[serde(default)]
    pub title: Option<String>,
}

/// A `HISTORY_REWOUND` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct HistoryRewound {
    /// The turns that were rewound away.
    #[serde(default)]
    pub rewound_turn_ids: Vec<String>,
    /// What the tools did there, so a person sees what was discarded.
    #[serde(default)]
    pub side_effects: Vec<SideEffectReport>,
}

/// What one tool did in a rewound turn.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct SideEffectReport {
    /// The tool's name.
    #[serde(default)]
    pub tool_name: String,
    /// What it did, in one line.
    #[serde(default)]
    pub side_effects: String,
}

/// An `OUTPUT_PART_FINISHED` payload: a non-text part the model produced.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct OutputPart {
    /// The part's id.
    pub part_id: String,
    /// What it is, as a media type or a modality name.
    #[serde(default)]
    pub modality: String,
    /// How many bytes it holds.
    #[serde(default)]
    pub bytes: u64,
    /// The stored blob's digest, when the host kept it.
    #[serde(default)]
    pub blob: Option<String>,
    /// Why it failed, when it did.
    #[serde(default)]
    pub error: Option<String>,
}

/// Usage one model request reported.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[allow(clippy::struct_field_names)]
pub struct RequestUsage {
    /// Tokens sent to the provider.
    #[serde(default)]
    pub input_tokens: u64,
    /// Tokens it produced.
    #[serde(default)]
    pub output_tokens: u64,
    /// Tokens served from its cache.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Tokens written to its cache.
    #[serde(default)]
    pub cache_write_tokens: u64,
}

/// The `request_completed` payload: one model request, accounted for.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RequestCompleted {
    /// What the provider reported.
    #[serde(default)]
    pub usage: RequestUsage,
    /// Provider-reported cost, when it reports one.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// How long the request took.
    #[serde(default)]
    pub duration_ms: u64,
    /// How long the first token took.
    #[serde(default)]
    pub first_token_ms: Option<u64>,
    /// Output tokens per second.
    #[serde(default)]
    pub tokens_per_second: Option<f64>,
    /// Whether the provider served it from cache.
    #[serde(default)]
    pub cache_hit: bool,
    /// The provider that served it.
    #[serde(default)]
    pub provider: Option<String>,
    /// The model it ran on.
    #[serde(default)]
    pub model: Option<String>,
}

/// A `USER_INPUT_ACCEPTED` payload (RFC 12.4): the turn, who sent it, and
/// what it carried.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct UserInputAccepted {
    /// The turn this input started.
    pub turn_id: String,
    /// Who sent it.
    #[serde(default)]
    pub origin: Origin,
    /// What it carried.
    #[serde(default)]
    pub content: Vec<AcceptedPart>,
    /// Its queue position, when it waits behind a running turn.
    #[serde(default)]
    pub position: Option<u64>,
    /// The turn it re-runs, when it is a re-run.
    #[serde(default)]
    pub replaces_turn_id: Option<String>,
}

/// Who originated an accepted input.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Origin {
    /// The kind, as `user`, `delegator` or `system`.
    #[serde(default, rename = "kind")]
    pub kind: OriginKind,
    /// The sender's id, when the transport knows one.
    #[serde(default)]
    pub user_id: Option<String>,
    /// The sender's display name, when the transport knows one.
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Which transport-side origin an input came from.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OriginKind {
    /// A person at a client.
    #[default]
    User,
    /// A parent agent.
    Delegator,
    /// The server itself.
    System,
}

/// One part of an accepted input.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AcceptedPart {
    /// What kind of part it is.
    #[serde(default, rename = "type")]
    pub kind: AcceptedContentKind,
    /// Its text.
    #[serde(default)]
    pub text: String,
}

/// What kind of part an accepted input carries.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AcceptedContentKind {
    /// Plain text.
    #[default]
    Text,
}

/// One agent, as a change payload names it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AgentChanged {
    /// The agent being left.
    #[serde(default)]
    pub from_agent: crate::AgentRef,
    /// The agent taking over.
    #[serde(default)]
    pub to_agent: crate::AgentRef,
}

/// What a budget notice applies to, as RAEP spells it.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeScope {
    /// One message.
    #[default]
    Message,
    /// One session.
    Session,
    /// The day, across sessions. RAEP writes it as one word.
    #[serde(rename = "globaldaily")]
    GlobalDaily,
}

impl From<crate::BudgetScope> for NoticeScope {
    fn from(value: crate::BudgetScope) -> Self {
        match value {
            crate::BudgetScope::Message => Self::Message,
            crate::BudgetScope::Session => Self::Session,
            crate::BudgetScope::GlobalDaily => Self::GlobalDaily,
        }
    }
}

/// A budget warning or an exhausted budget.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct BudgetNotice {
    /// What it applies to.
    #[serde(default)]
    pub scope: NoticeScope,
    /// What it measures.
    #[serde(default)]
    pub metric: crate::BudgetUnit,
    /// How much was spent.
    pub used: f64,
    /// The limit that was set.
    pub limit: f64,
    /// What happens now, when the budget is exhausted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<crate::BudgetAction>,
}

/// A `JOB_FINISHED` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct JobFinished {
    /// The job that ended, when the frame names it in the body.
    #[serde(default)]
    pub job_id: Option<String>,
    /// How it ended.
    #[serde(default)]
    pub status: crate::JobStatus,
    /// Its exit code, when its runner reported one.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// The signal that killed it, when a signal did.
    #[serde(default)]
    pub signal: Option<String>,
    /// Its label, in one line.
    #[serde(default)]
    pub summary: String,
    /// The tail of its output.
    #[serde(default)]
    pub output_snapshot: String,
    /// Whether the output was cut.
    #[serde(default)]
    pub output_truncated: bool,
    /// How many bytes it wrote in total.
    #[serde(default)]
    pub output_bytes_total: u64,
    /// How long it ran, in milliseconds.
    #[serde(default)]
    pub duration_ms: u64,
    /// Whether its end started a new turn.
    #[serde(default)]
    pub triggered_new_turn: bool,
    /// The turn that new turn is.
    #[serde(default)]
    pub new_turn_id: Option<String>,
    /// The turn its report was injected into.
    #[serde(default)]
    pub injected_into_turn_id: Option<String>,
}

/// One job in a `JOB_LIST` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct JobEntry {
    /// The job's id.
    pub job_id: String,
    /// Its command or label.
    #[serde(default)]
    pub command: String,
    /// Where it stands.
    #[serde(default)]
    pub status: crate::JobStatus,
    /// Its exit code, when its runner reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// A `JOB_LIST` payload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct JobList {
    /// The session's jobs, in the host's order.
    #[serde(default)]
    pub jobs: Vec<JobEntry>,
}

/// What kind of agent change a canonical payload describes.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChange {
    /// The session moved to another model.
    #[default]
    ModelChanged,
    /// A model change is waiting for the next turn.
    ModelQueued,
    /// The requested reasoning setting was lowered.
    ReasoningDowngraded,
    /// The catalogue was reloaded.
    CatalogUpdated,
    /// The skills were rescanned.
    SkillsUpdated,
}

/// The receipt of one uploaded artifact.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct UploadReceipt {
    /// The opaque upload id a user input references.
    pub upload_id: String,
    /// The MIME type the client sent.
    #[serde(default)]
    pub mime_type: String,
    /// How many bytes were stored.
    #[serde(default)]
    pub bytes: u64,
    /// The digest of the stored bytes.
    #[serde(default)]
    pub sha256: String,
    /// When the upload stops being retained.
    #[serde(default)]
    pub expires_at: i64,
}

/// One part of a `USER_INPUT`'s content.
#[derive(Clone, Debug, Serialize)]
pub struct UserInputPart {
    /// What it is.
    #[serde(rename = "type")]
    pub kind: InputPartKind,
    /// The text, for a text part.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Where the bytes are, for an uploaded part.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<UploadSource>,
    /// The MIME type, when the part names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// What a content part carries.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputPartKind {
    /// Text.
    Text,
    /// An image.
    Image,
    /// A document.
    Document,
    /// An audio recording.
    Audio,
    /// A video.
    Video,
}

/// Where an uploaded part's bytes live.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum UploadSource {
    /// An upload the host already stores.
    Upload {
        /// The upload id.
        upload_id: String,
    },
}

impl UserInputPart {
    /// A text part, used when a prompt travels with attachments.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: InputPartKind::Text,
            text: Some(text.into()),
            source: None,
            mime_type: None,
        }
    }

    /// A part backed by an upload.
    #[must_use]
    pub fn upload(kind: InputPartKind, upload_id: String, mime_type: String) -> Self {
        Self {
            kind,
            text: None,
            source: Some(UploadSource::Upload { upload_id }),
            mime_type: Some(mime_type),
        }
    }
}
