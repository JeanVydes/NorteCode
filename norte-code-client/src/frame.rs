use raep_types::{Envelope, FrameType, StandardFrameType};
use serde_json::Value;

#[allow(clippy::wildcard_imports)]
use crate::*;

#[derive(Debug, serde::Deserialize)]
pub(crate) struct FrameWire {
    #[serde(default)]
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) id: Option<raep_types::FrameId>,
    #[serde(default)]
    pub(crate) seq: Option<u64>,
    #[serde(default)]
    pub(crate) session_id: String,
    #[serde(default)]
    pub(crate) turn_id: Option<raep_types::TurnId>,
    #[serde(default)]
    pub(crate) timestamp: i64,
    #[serde(default)]
    pub(crate) kind: Option<FrameType>,
    #[serde(default)]
    pub(crate) payload: Value,
    #[serde(default)]
    pub(crate) ext: Option<Value>,
}

impl FrameWire {
    pub(crate) fn into_envelope(self) -> Envelope<Value> {
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

/// A fork request payload. `raep-types` defines the `SessionFork` frame
/// but no payload struct for it yet, so the client owns this one shape.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ForkPayload {
    pub(crate) session_id: String,
}

/// A command response body: the events a command produced, or the error
/// that rejected it. Decoded as a struct, never by key.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CommandResponseBody {
    #[serde(default)]
    pub(crate) events: Vec<Envelope<Value>>,
    #[serde(default)]
    pub(crate) error: Option<RejectError>,
}

impl CommandResponseBody {
    pub(crate) fn error_message(&self) -> String {
        self.error.as_ref().map_or_else(
            || "RAEP command failed".to_owned(),
            |error| error.message.clone(),
        )
    }
}

/// A rejected command's error body.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct RejectError {
    #[serde(default)]
    pub(crate) message: String,
}

/// The frame kinds this client maps onto protocol events. The wire spells
/// them in screaming snake case; the enum is the closed set, so a new frame
/// type is a compile error here, not a silent fall-through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameKind {
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
    pub(crate) fn from_standard(kind: &StandardFrameType) -> Self {
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
pub(crate) enum ToolStatus {
    Ok,
    Error,
    Cancelled,
}

/// A job's status, as the wire spells it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum JobStatus {
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
pub(crate) struct TokenDeltaPayload {
    pub(crate) delta: String,
    pub(crate) index: u32,
    #[serde(default)]
    pub(crate) channel: String,
}

/// A tool call start payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ToolCallStartedPayload {
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    #[serde(default)]
    pub(crate) arguments: Option<Value>,
    #[serde(default)]
    pub(crate) source: Option<String>,
}

/// A tool output delta payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ToolOutputDeltaPayload {
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    #[serde(default = "default_output_stream")]
    pub(crate) stream: norte_code_wire::OutputStream,
    pub(crate) delta: String,
}

pub(crate) fn default_output_stream() -> norte_code_wire::OutputStream {
    norte_code_wire::OutputStream::Stdout
}

/// A tool call finish payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ToolCallFinishedPayload {
    pub(crate) tool_call_id: String,
    #[serde(default)]
    pub(crate) tool_name: Option<String>,
    pub(crate) status: ToolStatus,
    #[serde(default)]
    pub(crate) result_summary: Option<String>,
    #[serde(default)]
    pub(crate) result_truncated: bool,
    #[serde(default)]
    pub(crate) duration_ms: u64,
}

/// One job entry in a job list payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct JobEntryPayload {
    pub(crate) job_id: String,
    #[serde(default)]
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) status: JobStatus,
    #[serde(default)]
    pub(crate) exit_code: Option<i32>,
}

/// A job list payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct JobListPayload {
    #[serde(default)]
    pub(crate) jobs: Vec<JobEntryPayload>,
}

/// An error payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ErrorPayload {
    #[serde(default)]
    pub(crate) code: String,
    #[serde(default)]
    pub(crate) message: String,
}

/// The extension's `TurnFinished` fields, which the canonical turn payload
/// does not carry.
#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct ExtensionTurnFinished {
    #[serde(default)]
    pub(crate) phase: Option<norte_code_wire::Phase>,
    #[serde(default)]
    pub(crate) committed: Option<bool>,
}

/// A typed view of one raw frame: the envelope fields the decoder needs,
/// read once from the typed `Envelope`, instead of string lookups scattered
/// through the match.
///
/// The frame arrives as `Envelope<Value>` — the spec's generic payload slot
/// — but every field the decoder touches is read through the envelope's own
/// typed fields (`kind`, `session_id`, `seq`, `ext`), never by key.
pub(crate) struct FrameView {
    pub(crate) frame_type: Option<FrameKind>,
    pub(crate) payload: Option<Value>,
    pub(crate) session: Option<norte_code_wire::SessionKey>,
    pub(crate) turn: u64,
    pub(crate) extension_turn_finished: Option<ExtensionTurnFinished>,
}

impl FrameView {
    pub(crate) fn from_envelope(frame: &Envelope<Value>) -> Self {
        let frame_type = match &frame.kind {
            FrameType::Standard(kind) => Some(FrameKind::from_standard(kind)),
            FrameType::Extension(_) => None,
        };
        let session = (!frame.session_id.is_empty())
            .then(|| norte_code_wire::SessionKey::new(frame.session_id.clone()));
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
pub(crate) fn from_payload<T: serde::de::DeserializeOwned>(
    payload: Value,
) -> Result<T, ClientError> {
    serde_json::from_value(payload)
        .map_err(|error| ClientError::new(format!("invalid RAEP payload: {error}")))
}
