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
/// It is a path, not the bytes: the client and the service share a filesystem,
/// and a pasted photo is a file either way. The service reads it where its blob
/// store is and files it there, so the bytes never travel on the wire.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    /// The path to the file, as the client named it.
    ///
    /// A path and not bytes: the client and the service share a filesystem as
    /// well as a blob store, and a pasted photo is a file either way. Reading it
    /// where the store is keeps the bytes off the wire and out of the editor's
    /// thread, and the service is what files it.
    pub path: String,
    /// The media type, as `type/subtype`.
    pub mime: String,
    /// What the client called it, for a person reading the transcript.
    pub name: String,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    /// Standard output.
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
pub enum Phase {
    /// Nothing in flight.
    Idle,
    /// A turn is being produced.
    Streaming,
    /// A tool call is authorised and about to run.
    AwaitingTool,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetUnit {
    /// US dollars, provider-reported.
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
