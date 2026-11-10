//! Wire-supporting types for the Norte Code protocol.
//!
//! Re-exported from the crate root; nothing here is Norte-specific.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A stable identifier of a logical Norte Code session.
///
/// It is Norte Code's own id, not the engine's: a compaction replaces the
/// underlying Norte session while the logical id stays the same.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionKey(String);

impl SessionKey {
    /// Wraps `id`.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One piece of media travelling with a message.
///
/// Local clients may provide `path`; remote clients should provide `blob` after
/// uploading through the host. The latter avoids shared-filesystem assumptions
/// and lets the service use its content-addressed store directly.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    /// The path to a local file, for in-process clients.
    #[serde(default)]
    pub path: String,
    /// A content-addressed blob already stored by the host, as its lowercase
    /// hex digest.
    ///
    /// It is the string the wire carries and not the server's parsed id: this
    /// crate is the shapes, and putting a parser here would be a parser every
    /// client has to agree with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// The media type, as `type/subtype`.
    pub mime: String,
    /// What the client called it, for a person reading the transcript.
    pub name: String,
    /// Media duration in milliseconds, for audio or video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Client-provided transcript for audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
}

/// A session as the client sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInfo {
    /// The logical id.
    pub session: SessionKey,
    /// The label.
    pub title: Option<String>,
    /// The provider that serves the model in use.
    ///
    /// Its own field and not half of a `provider/model` string: a model id may
    /// carry slashes of its own, so the two are never written as one to be
    /// taken apart again.
    pub provider: Option<String>,
    /// The model id in use.
    pub model: Option<String>,
    /// When it was created, as RFC 3339.
    pub created_at: Option<String>,
    /// Where it currently is.
    pub phase: Phase,
    /// The session-wide reasoning override, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    /// The client key of the parent, when this session is a subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionKey>,
}

/// Metadata for one plugin managed by the remote host.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginInfo {
    /// Plugin name or installed library stem.
    pub name: String,
    /// Whether the plugin is enabled for the next host start.
    pub enabled: bool,
    /// Whether the host currently loaded it successfully.
    pub loaded: bool,
}

/// One MCP server visible to the service.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpInfo {
    /// Configuration name.
    pub name: String,
    /// Command label.
    pub command: String,
    /// Published tool names.
    pub tools: Vec<String>,
    /// Tools filtered by configuration.
    pub filtered: Vec<String>,
    /// Whether its tools are enabled.
    pub active: bool,
}

/// An MCP server that could not be started.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpFailure {
    /// Configuration name.
    pub name: String,
    /// Startup failure.
    pub reason: String,
}

/// One task in a session's list, as a client renders it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TodoInfo {
    /// What the task says, as the model wrote it.
    pub content: String,
    /// Where it stands.
    pub status: TodoStatus,
}

/// The state of one task.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Not started.
    Pending,
    /// Being worked on.
    InProgress,
    /// Done.
    Completed,
}

/// Which output stream a tool's progress chunk came from.
///
/// The wire's own name for the two streams Norte carries, plus a fallback for
/// a stream a future Norte build may add: a stream this build does not know
/// is reported rather than dropped or panicked on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    /// Standard output.
    #[default]
    Stdout,
    /// Standard error.
    Stderr,
    /// A stream this build does not know by name.
    Other,
}

/// Who spoke, as a client renders a replayed message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// The human.
    User,
    /// The model.
    Assistant,
    /// A tool result.
    Tool,
    /// A system message.
    System,
    /// A developer message.
    Developer,
    /// A role this build does not know by name.
    Unknown,
}

impl fmt::Display for MessageRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::System => "system",
            Self::Developer => "developer",
            Self::Unknown => "unknown",
        })
    }
}

/// Why a compaction started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    /// Requested by a person.
    Manual,
    /// Triggered by the context threshold.
    #[default]
    Threshold,
}

impl fmt::Display for CompactionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Manual => "manual",
            Self::Threshold => "threshold",
        })
    }
}

/// One kind of content a model accepts or produces.
///
/// It is the wire's own name for the three the ecosystem shares. The catalogue
/// has its own enum and this is the one the wire carries: the protocol crate is
/// free of Norte types and of the catalogue's, so the mapping between them
/// happens in one place rather than a string crossing the wire and being parsed
/// again at the other end.
///
/// `File` is the wire's name for a document, which is what a PDF is; what a
/// turn may produce is [`OutputModality`](crate::wire::OutputModality), whose
/// document variant is spelled `document`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// Plain text.
    Text,
    /// An image.
    Image,
    /// Recorded audio.
    Audio,
    /// Recorded video.
    Video,
    /// A document, which is what a PDF is.
    File,
}

/// One provider as the editor shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProviderInfo {
    /// The provider id.
    pub provider_id: String,
    /// The display name.
    pub name: String,
    /// The base URL of its preferred wire.
    pub api: Option<String>,
    /// Whether it lives in `providers.json`. A plugin's provider does not, and
    /// editing one here would be lost on the next start.
    pub from_file: bool,
    /// Whether any of its models is in use by a session.
    pub model_count: usize,
}

/// One model as the editor shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogModelInfo {
    /// The provider id.
    pub provider: String,
    /// The model id.
    pub model: String,
    /// The display name.
    pub name: String,
    /// Whether it lives in `models.json`.
    pub from_file: bool,
}

/// One model the catalogue knows, flattened for a picker.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInfo {
    /// The provider id.
    pub provider: String,
    /// The model id the provider expects.
    pub model: String,
    /// The display name.
    pub name: String,
    /// A one-line description.
    pub description: Option<String>,
    /// The context window, when known and plausible.
    pub max_context: Option<u64>,
    /// The output limit, when known and plausible.
    pub max_output: Option<u64>,
    /// Whether the model calls host functions.
    pub tools: bool,
    /// Accepted input modalities.
    pub input: Vec<Modality>,
    /// Produced output modalities.
    pub output: Vec<Modality>,
    /// The reasoning controls the host may offer.
    pub reasoning: ReasoningInfo,
    /// Whether the catalogue marks the model deprecated.
    pub deprecated: bool,
}

/// Where a background job stands.
///
/// It crosses the wire as an enum and not as a word: a client that matched on a
/// string had a catch-all arm that swallowed every typo, so a state the service
/// added would have been drawn as "finished" by every client that did not know
/// it. An unknown variant in a payload is a parse error naming the variant,
/// which is the failure a reader can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Running, or finished and not yet read.
    Running,
    /// Ended on its own.
    Finished,
    /// Ended because someone asked it to.
    Cancelled,
}

impl JobState {
    /// The word a person and the model read.
    ///
    /// It is the same word the wire carries, because these are the same three
    /// states: a separate spelling for the prompt would be a second vocabulary
    /// that drifts, and the prompt is where a drift is hardest to notice.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether the job is over.
    #[must_use]
    pub fn is_done(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// Effective limits applied to a detached job.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobLimits {
    /// Runtime ceiling in seconds, when the runner is bounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    /// Maximum captured output in bytes, when the runner has an output cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<u64>,
}

/// When the result of a detached job is offered to the agent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobTurnPolicy {
    /// Start a turn for every terminal job result.
    Always,
    /// Start a turn only for failed, killed, or timed-out jobs.
    OnFailure,
    /// Record the result for the next user turn without starting one.
    #[default]
    Never,
}

/// One background job, as a client sees it.
///
/// It is the registry's summary, flattened for the wire: the output is a string
/// rather than a byte buffer, the state is the word the model reads, and the
/// timings are milliseconds. A client that only lists jobs ignores the output;
/// one that checks a job reads it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobInfo {
    /// The id the model was given.
    pub id: String,
    /// The label, clipped to one line: a shell's script, a subagent's title.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// The script, clipped to one line. Kept on the wire for one release so
    /// a client that still reads it stops getting a parse error.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub script: String,
    /// Where it stands.
    pub state: JobState,
    /// The exit code, once it ended on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Whether its time bound killed it.
    pub timed_out: bool,
    /// Milliseconds since it started.
    pub elapsed_ms: u64,
    /// The most recent output.
    pub output: String,
    /// Whether older output was dropped to stay within the buffer.
    pub dropped: bool,
    /// Total output bytes written, including bytes no longer retained.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub output_bytes_total: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// One plan of a session, as the app lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanInfo {
    /// Stable plan identifier, also the Markdown filename.
    pub id: String,
    /// The plan text.
    pub content: String,
    /// Where the plan stands.
    pub status: crate::PlanStatus,
    /// Its title from the front matter, when it has one.
    pub title: Option<String>,
    /// Revisions after the initial proposal.
    pub revision: u32,
    /// The model that proposed it, when recorded.
    pub model: Option<String>,
}

/// One installed Agent Skill, as a client sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillInfo {
    /// The validated name, equal to its directory name.
    pub name: String,
    /// What the skill does and when to use it.
    pub description: String,
    /// The skill directory.
    pub root: String,
    /// Tools the skill declares as pre-approved, when the host honors them.
    pub allowed_tools: Vec<String>,
}

/// The reasoning controls a model declares.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningInfo {
    /// Whether the model reasons at all.
    pub supported: bool,
    /// Whether reasoning may be turned off.
    pub can_disable: bool,
    /// The effort names the host may offer, in Norte's spelling.
    pub efforts: Vec<String>,
    /// Whether a token budget is accepted.
    pub supports_budget: bool,
    /// The largest accepted budget.
    pub max_budget: Option<u32>,
}

/// Where a session currently is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Phase {
    /// Nothing in flight.
    Idle,
    /// A turn is being produced.
    Streaming,
    /// A tool call is authorised and about to run.
    AwaitingTool,
}

impl Phase {
    /// Whether nothing is in flight.
    #[must_use]
    pub const fn is_idle(self) -> bool {
        matches!(self, Self::Idle)
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Idle => "idle",
            Self::Streaming => "streaming",
            Self::AwaitingTool => "awaiting_tool",
        })
    }
}

/// Where a delegated child session stands.
///
/// It travels on the wire and is read by clients that show a subagent list, so
/// it is a closed set here rather than a string each producer spells: a typo in
/// `"RUNNING"` used to be a child that silently read as `Stopped`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChildState {
    /// Started, with no turn in flight.
    #[default]
    Idle,
    /// A turn is running.
    Running,
    /// Parked on an approval.
    AwaitingApproval,
    /// It ended in failure.
    Failed,
    /// It stopped without failing.
    Stopped,
}

impl ChildState {
    /// Whether nothing more will happen in this child.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Failed | Self::Stopped)
    }

    /// The word a list shows.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::AwaitingApproval => "waiting",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    /// Whether it is doing something now.
    #[must_use]
    pub const fn is_working(self) -> bool {
        matches!(self, Self::Running | Self::AwaitingApproval)
    }
}

impl fmt::Display for ChildState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Idle => "IDLE",
            Self::Running => "RUNNING",
            Self::AwaitingApproval => "AWAITING_APPROVAL",
            Self::Failed => "FAILED",
            Self::Stopped => "STOPPED",
        })
    }
}

/// Why a child's state changed, on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildStateReason {
    /// Its turn finished.
    TurnFinished,
    /// A turn failed.
    TurnFailed,
    /// It was stopped on request.
    StateChange,
}

impl fmt::Display for ChildStateReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TurnFinished => "turn_finished",
            Self::TurnFailed => "turn_failed",
            Self::StateChange => "state_change",
        })
    }
}

/// A stream delta.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "delta", rename_all = "snake_case")]
pub enum Delta {
    /// Answer text.
    Text {
        /// The part index.
        index: u32,
        /// The text.
        text: String,
    },
    /// Reasoning text.
    Reasoning {
        /// The part index.
        index: u32,
        /// The text.
        text: String,
    },
    /// Refusal text.
    Refusal {
        /// The part index.
        index: u32,
        /// The text.
        text: String,
    },
    /// A fragment of a tool call's arguments.
    ToolArguments {
        /// The part index.
        index: u32,
        /// The call.
        call_id: String,
        /// The fragment.
        arguments: String,
    },
    /// Generated media, as a byte count.
    Media {
        /// The part index.
        index: u32,
        /// How many bytes.
        bytes: u64,
        /// Whether the part ends here.
        last: bool,
    },
}

/// Provider-reported token counters and cost.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    /// Input tokens.
    pub input_tokens: Option<u64>,
    /// Output tokens.
    pub output_tokens: Option<u64>,
    /// Reasoning tokens, counted inside the output.
    pub reasoning_tokens: Option<u64>,
    /// Tokens read from the provider's cache.
    pub cache_read_tokens: Option<u64>,
    /// Tokens written to the provider's cache.
    pub cache_write_tokens: Option<u64>,
    /// The cost the provider reported, in USD, when it reported one.
    pub cost_usd: Option<f64>,
    /// Provider-specific counters, kept verbatim.
    pub vendor: BTreeMap<String, Value>,
}

/// What a budget limits.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum BudgetUnit {
    /// US dollars, provider-reported.
    #[default]
    Usd,
    /// Tokens, provider-reported.
    Tokens,
    /// Model requests.
    Requests,
}

/// What to do when a budget is exhausted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetAction {
    /// Report it and keep going.
    Warn,
    /// Park for approval.
    Ask,
    /// Stop the turn.
    Stop,
}

/// A spending limit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// What it measures.
    pub unit: BudgetUnit,
    /// The limit.
    pub limit: f64,
    /// What to do at the limit.
    pub action: BudgetAction,
}

/// What a budget applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    /// One message.
    Message,
    /// One session.
    Session,
    /// The day, across sessions.
    GlobalDaily,
}

/// A reasoning setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reasoning", rename_all = "snake_case")]
pub enum Reasoning {
    /// Let the model decide.
    Automatic,
    /// Turn reasoning off, when the model allows it.
    Disabled,
    /// A named effort.
    Effort {
        /// The effort name.
        effort: String,
    },
    /// A token budget for reasoning.
    Budget {
        /// The budget.
        tokens: u32,
    },
}

/// Whether a reasoning setting applies to a session or one message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningScope {
    /// The session default.
    Session,
    /// The next message only.
    Message,
}

/// A machine-readable error category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request was malformed.
    InvalidRequest,
    /// The named session, model or approval does not exist.
    NotFound,
    /// The session moved and the action could not be applied.
    Conflict,
    /// The model cannot do what was asked.
    Capability,
    /// A budget stopped the action.
    Budget,
    /// The provider failed.
    Provider,
    /// The service failed internally.
    Internal,
    /// The feature is not implemented yet.
    Unsupported,
}

#[cfg(test)]
mod child_state_tests {
    use super::{ChildState, ChildStateReason};

    #[test]
    fn a_child_state_spells_the_wire_name_in_every_direction() {
        for (state, wire) in [
            (ChildState::Idle, "\"IDLE\""),
            (ChildState::Running, "\"RUNNING\""),
            (ChildState::AwaitingApproval, "\"AWAITING_APPROVAL\""),
            (ChildState::Failed, "\"FAILED\""),
            (ChildState::Stopped, "\"STOPPED\""),
        ] {
            assert_eq!(serde_json::to_string(&state).expect("serializable"), wire);
            let decoded: ChildState = serde_json::from_str(wire).expect("deserializable");
            assert_eq!(decoded, state);
            assert_eq!(state.to_string(), wire.trim_matches('"'));
        }
    }

    #[test]
    fn a_state_this_build_does_not_know_fails_instead_of_guessing() {
        // The closed set is the point: an unknown word is a build that is out
        // of date, not a child that stopped.
        assert!(serde_json::from_str::<ChildState>("\"PAUSED\"").is_err());
    }

    #[test]
    fn only_failed_and_stopped_are_terminal() {
        assert!(ChildState::Failed.is_terminal());
        assert!(ChildState::Stopped.is_terminal());
        assert!(!ChildState::Idle.is_terminal());
        assert!(!ChildState::Running.is_terminal());
        assert!(!ChildState::AwaitingApproval.is_terminal());
    }

    #[test]
    fn a_reason_keeps_its_wire_spelling() {
        let reason = serde_json::to_string(&ChildStateReason::TurnFinished).expect("serializable");
        assert_eq!(reason, "\"turn_finished\"");
    }
}

/// The kind of media a message carries or a model produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// An image.
    Image,
    /// Recorded audio.
    Audio,
    /// Recorded video.
    Video,
    /// A document, such as a PDF.
    Document,
}

/// Where a media part's bytes live.
///
/// Adjacently tagged, as the engine writes it: `tag = "source"` alone cannot
/// hold a variant whose payload is a string or the bytes themselves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "source", content = "value")]
pub enum MediaSource {
    /// Bytes held in the blob store, as their lowercase hex digest.
    Blob(String),
    /// A URL the provider fetches itself.
    Url(String),
    /// A file the provider already holds.
    ProviderFile(ProviderFileRef),
    /// The bytes themselves, base64 in JSON.
    #[serde(with = "bytes_base64")]
    Inline(Vec<u8>),
}

/// A file a provider holds, so its bytes need not travel again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFileRef {
    /// The provider that holds the file.
    pub provider: String,
    /// The provider's id for the file.
    pub file_id: String,
    /// When the reference stops being usable, in RFC 3339, where it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Durable fallback for the bytes, so an expired reference can be revived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

/// Descriptive fields a media part may carry. Every one is optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaMetadata {
    /// Length in milliseconds, for audio and video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Pixel width, for images and video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Pixel height, for images and video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// A transcript the provider produced for audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// Anything else the provider sent, under its own key.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub extension: std::collections::BTreeMap<String, Value>,
}

impl MediaPart {
    /// The bytes, when they travel inline.
    #[must_use]
    pub fn inline_bytes(&self) -> &[u8] {
        match &self.source {
            MediaSource::Inline(bytes) => bytes,
            MediaSource::Blob(_) | MediaSource::Url(_) | MediaSource::ProviderFile(_) => &[],
        }
    }
}

/// One piece of media in a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPart {
    /// The kind of media.
    pub kind: MediaKind,
    /// Where the bytes live.
    pub source: MediaSource,
    /// The media type, as `type/subtype`.
    pub mime: String,
    /// The descriptive fields.
    #[serde(default)]
    pub metadata: MediaMetadata,
}

/// Base64 in JSON, the encoding the engine writes inline media with.
mod bytes_base64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(serde::de::Error::custom)
    }
}
