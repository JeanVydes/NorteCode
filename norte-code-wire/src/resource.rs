//! Resource shapes the RAEP host serves over HTTP.
//!
//! These are the JSON bodies of `/info`, `/quota`, `/sessions/:id/telemetry`,
//! `/sessions/:id/jobs`, plan listings and uploads. They are not protocol
//! events: they are the host's own responses, defined once here so the browser
//! client, the terminal client and the host agree on a field without copying it.
//!
//! Every field a client might reasonably omit is `#[serde(default)]` and every
//! collection defaults empty: a host that adds a field must not break a client
//! that predates it.

use serde::{Deserialize, Serialize};

pub use crate::{JobState, PlanStatus};

/// The job status as the RAEP `JOB_LIST` protocol names it.
///
/// This is the host's wire status (five outcomes), distinct from [`JobState`],
/// which is the agent's conceptual lifecycle (running, finished or cancelled)
/// carried on session events. The host maps its internal outcome onto these
/// names; a client filters with them.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobStatus {
    /// Still doing work.
    #[default]
    Running,
    /// Ended on its own, successfully.
    Completed,
    /// Ended on its own, unsuccessfully.
    Failed,
    /// Ended because someone stopped it.
    Killed,
    /// Ended because its time bound ran out.
    TimedOut,
}

impl JobStatus {
    /// A short label a person can read in a list.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Killed => "Stopped",
            Self::TimedOut => "Timed out",
        }
    }

    /// Whether the job is still doing work.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// One background job as the host lists it for a session.
///
/// The host's `JOB_LIST_RESULT` flattens agent-specific fields; this keeps the
/// fields the clients actually read and ignores the rest, so a new field never
/// breaks deserialization.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct JobSummary {
    /// The id the model was given, and the one a client addresses it by.
    #[serde(default)]
    pub job_id: String,
    /// The script or title, as one line.
    #[serde(default)]
    pub command: String,
    /// Where the job stands.
    #[serde(default)]
    pub status: JobStatus,
    /// The exit code, once it ended on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The most recent output, when the host sends any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_snapshot: Option<String>,
    /// How many bytes the job has written in total.
    #[serde(default)]
    pub output_bytes_total: u64,
    /// Whether the output was cut at the host's bound.
    #[serde(default)]
    pub output_truncated: bool,
    /// How long it ran, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// One plan as the host lists it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanSummary {
    /// Stable plan identifier, also the Markdown filename.
    #[serde(default)]
    pub id: String,
    /// The plan text.
    #[serde(default)]
    pub content: String,
    /// Where the plan stands.
    #[serde(default)]
    pub status: PlanStatus,
    /// Its title from the front matter, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Revisions after the initial proposal.
    #[serde(default)]
    pub revision: u32,
    /// The model that proposed it, when the front matter names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl PlanStatus {
    /// A short label a person can read in a list.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pending => "awaiting approval",
            Self::Canceled => "canceled",
        }
    }
}

/// Where a session stands, as the management API reports it.
///
/// The host writes `ACTIVE` or `SUSPENDED`; `CLOSED` exists for a session that
/// was ended, and `Unknown` keeps a client working when the host grows a state
/// it predates instead of failing the whole list.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SessionStatus {
    /// Open and able to take work.
    Active,
    /// Stored but not running: resumable, not attached.
    Suspended,
    /// Ended and kept for reading.
    Closed,
    /// A state this client does not know.
    #[default]
    #[serde(other)]
    Unknown,
}

impl SessionStatus {
    /// A short label a person can read in a list.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "Available",
            Self::Suspended => "Suspended",
            Self::Closed => "Closed",
            Self::Unknown => "Unknown",
        }
    }

    /// Whether the session is stored but not running.
    #[must_use]
    pub const fn is_suspended(self) -> bool {
        matches!(self, Self::Suspended)
    }
}

/// One child session, as a parent's child list and the event stream describe it.
///
/// The list endpoint sends the id, the title and the state; the stream also
/// learns the agent and the task it was given, so those are optional. A client
/// that only has the list still decodes.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChildSessionSummary {
    /// The child's session id.
    #[serde(default)]
    pub session_id: String,
    /// The title assigned to the child, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Where the child stands.
    #[serde(default)]
    pub state: crate::ChildState,
    /// The agent the child runs, when the stream named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The task the parent gave it, when the stream carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Milliseconds since the client first saw it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
}

/// The agent half of a session or agent summary: an id and its version.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRef {
    /// The agent's id.
    #[serde(default)]
    pub id: String,
    /// The agent's version, empty when the host does not report one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
}

/// Who named the session.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTitleSource {
    /// A person named it, with `/rename` or a create request.
    #[default]
    User,
}

/// One session as `/sessions` lists it.
///
/// `None` on a timestamp or a count means the host did not report it, which is
/// different from zero: a client that sorts by activity sorts reported ones
/// first instead of pretending every old session ran at the epoch.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SessionSummary {
    /// The session id.
    #[serde(default)]
    pub session_id: String,
    /// The label, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Who named it.
    #[serde(default)]
    pub title_source: SessionTitleSource,
    /// Free-form labels the host keeps with the session.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, serde_json::Value>,
    /// Whether the session is pinned in the host's list.
    #[serde(default)]
    pub pinned: bool,
    /// Whether the session is archived.
    #[serde(default)]
    pub archived: bool,
    /// Where it stands.
    #[serde(default)]
    pub status: SessionStatus,
    /// The agent that runs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentRef>,
    /// The model this session runs on, when the host reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<AgentRef>,
    /// The provider that serves `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The session-wide reasoning setting, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<crate::Reasoning>,
    /// The parent's key, when this session is a subagent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// When it was created, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    /// When it last changed, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    /// When it last did any work, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    /// How many turns it has taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    /// Whether a turn is waiting to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_pending: Option<bool>,
}

#[cfg(test)]
#[path = "resource_tests.rs"]
mod tests;

/// How one model request ended.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    /// The provider answered.
    #[default]
    Ok,
    /// The request failed.
    Error,
    /// The caller stopped it.
    Cancelled,
}

/// One model request, as the host measured it.
///
/// It is the transport shape of a stored request row: the times are RFC 3339
/// text and every measurement the host could not take is absent, which is
/// different from zero.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TelemetryRecord {
    /// A stable request id.
    #[serde(default)]
    pub id: String,
    /// The logical session key.
    #[serde(default)]
    pub session: String,
    /// The engine session the request belongs to.
    #[serde(default)]
    pub north_session: String,
    /// The turn inside that session.
    #[serde(default)]
    pub turn: u64,
    /// The provider id.
    #[serde(default)]
    pub provider: String,
    /// The model id.
    #[serde(default)]
    pub model: String,
    /// The base URL the request went to, when the provider declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// When the request started, in RFC 3339.
    #[serde(default)]
    pub started_at: String,
    /// How long it took, including streaming.
    #[serde(default)]
    pub duration_ms: u64,
    /// Time to the first answer delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    /// Time to the last answer delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_token_ms: Option<u64>,
    /// Prompt tokens the provider reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Answer tokens the provider reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Reasoning tokens the provider reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Prompt tokens served from the provider cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Prompt tokens written to the provider cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// A local estimate used only when usage is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_input_tokens: Option<u64>,
    /// Whether the provider served any prompt token from cache.
    #[serde(default)]
    pub cache_hit: bool,
    /// Tools the model called in this request, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<String>,
    /// The reasoning setting the request carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Whether the command sandbox was enabled.
    #[serde(default)]
    pub sandbox: bool,
    /// Whether that sandbox allowed network access.
    #[serde(default)]
    pub sandbox_network: bool,
    /// The generation parameters the request carried.
    #[serde(default)]
    pub params: serde_json::Value,
    /// The vendor extensions the request carried.
    #[serde(default)]
    pub extensions: serde_json::Value,
    /// Anything else the host measured.
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// The cost the provider reported, in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Why generation stopped, as the wire named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// How the request ended.
    #[serde(default)]
    pub status: RequestStatus,
    /// The provider or transport error, when it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The kind of provider failure, when it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    /// How long the provider asked the caller to wait before retrying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// How much of a model's context a session has used.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContextUsage {
    /// The tokens the last request carried.
    pub used: u64,
    /// The model's context window, when the catalogue knows it.
    pub window: Option<u64>,
}

impl ContextUsage {
    /// The share of the window in use, from 0 to 1, when the window is known.
    ///
    /// The share is thousandths of the window computed in whole numbers: a
    /// token count is 64 bits wide and a float holds 52, and the difference is
    /// a rounding nobody can see in a percentage.
    #[must_use]
    pub fn share(&self) -> Option<f64> {
        let window = self.window.filter(|window| *window > 0)?;
        let thousandths = (u128::from(self.used) * 1_000) / u128::from(window);
        let thousandths = u32::try_from(thousandths).unwrap_or(1_000);
        Some(f64::from(thousandths) / 1_000.0)
    }
}

/// One agent as `/agents` lists it.
///
/// The capabilities are the agent's own: which modalities it accepts and
/// produces, and what each one is bounded by, so a client can offer only what
/// the server will take instead of discovering it by failing an upload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentSummary {
    /// The agent's id.
    #[serde(default)]
    pub id: String,
    /// The agent's version.
    #[serde(default)]
    pub version: String,
    /// What it can do, when the host reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<AgentCapabilities>,
    /// The provider that serves its default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentRef>,
    /// Its default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<AgentRef>,
}

/// What one agent accepts and produces.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentCapabilities {
    /// The modalities it accepts as input.
    #[serde(default)]
    pub input: Vec<crate::OutputModality>,
    /// The modalities it may produce.
    #[serde(default)]
    pub output: Vec<crate::OutputModality>,
    /// The bound for each modality it accepts.
    #[serde(default)]
    pub limits: std::collections::BTreeMap<crate::OutputModality, AgentModalityLimits>,
}

/// What one modality is bounded by.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AgentModalityLimits {
    /// The media types it accepts for this modality.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mime_types: Vec<String>,
    /// The largest one part may be, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// How many parts one message may carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    /// The longest audio or video it accepts, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
}

/// One quota period, in milliseconds since the epoch.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuotaPeriod {
    /// When the period started.
    #[serde(default)]
    pub start: u64,
    /// When it ends.
    #[serde(default)]
    pub end: u64,
}

/// Which resource a quota dimension counts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaDimensionName {
    /// Stored sessions.
    #[default]
    Sessions,
    /// Events held for replay.
    ReplayBufferEvents,
}

impl QuotaDimensionName {
    /// A label a person can read in a list.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sessions => "Owned sessions",
            Self::ReplayBufferEvents => "Retained replay events",
        }
    }
}

/// One quota dimension and where it stands.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuotaDimension {
    /// What it counts.
    #[serde(default)]
    pub name: QuotaDimensionName,
    /// How much is allowed.
    #[serde(default)]
    pub limit: u64,
    /// How much is used.
    #[serde(default)]
    pub used: u64,
    /// How much is left.
    #[serde(default)]
    pub remaining: u64,
}

/// A principal's quota, as `/quota` reports it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuotaReport {
    /// The principal it belongs to.
    #[serde(default)]
    pub tenant: String,
    /// The period it is measured over.
    #[serde(default)]
    pub period: QuotaPeriod,
    /// Each dimension it limits.
    #[serde(default)]
    pub dimensions: Vec<QuotaDimension>,
}

/// Totals over a set of request records.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TelemetrySummary {
    /// How many requests there were.
    #[serde(default)]
    pub requests: u64,
    /// How many failed.
    #[serde(default)]
    pub errors: u64,
    /// How many were stopped.
    #[serde(default)]
    pub cancelled: u64,
    /// Total time spent, in milliseconds.
    #[serde(default)]
    pub duration_ms: u64,
    /// Prompt tokens, as the providers reported them.
    #[serde(default)]
    pub input_tokens: u64,
    /// Answer tokens, as the providers reported them.
    #[serde(default)]
    pub output_tokens: u64,
    /// Prompt tokens served from a provider cache.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Prompt tokens written to a provider cache.
    #[serde(default)]
    pub cache_write_tokens: u64,
}

/// A telemetry report, as `/telemetry` serves it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TelemetryReport {
    /// The session it covers, when it covers one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The rows, oldest first.
    #[serde(default)]
    pub records: Vec<TelemetryRecord>,
    /// The totals over those rows.
    #[serde(default)]
    pub summary: TelemetrySummary,
}
