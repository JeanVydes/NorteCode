use raep_types::{Envelope, FrameType, StandardFrameType};
use serde_json::Value;

use crate::error::ClientError;

/// A fork request payload. `raep-types` defines the `SessionFork` frame
/// but no payload struct for it yet, so the client owns this one shape.
#[cfg(feature = "native")]
#[derive(Debug, serde::Serialize)]
pub(crate) struct ForkPayload {
    pub(crate) session_id: String,
}

/// A command response body: the events a command produced, or the error
/// that rejected it. Decoded as a struct, never by key.
#[cfg(feature = "native")]
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CommandResponseBody {
    #[serde(default)]
    pub(crate) events: Vec<Envelope<Value>>,
    #[serde(default)]
    pub(crate) error: Option<RejectError>,
}

#[cfg(feature = "native")]
impl CommandResponseBody {
    pub(crate) fn error_message(&self) -> String {
        self.error.as_ref().map_or_else(
            || "RAEP command failed".to_owned(),
            |error| error.message.clone(),
        )
    }
}

/// A rejected command's error body.
#[cfg(feature = "native")]
#[derive(Debug, serde::Deserialize)]
pub(crate) struct RejectError {
    #[serde(default)]
    pub(crate) message: String,
}

/// A `HISTORY_PAGE` payload as the host sends it.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct HistoryPagePayload {
    #[serde(default)]
    pub(crate) items: Vec<HistoryItemPayload>,
    #[serde(default)]
    pub(crate) has_more: bool,
    /// The spec's spelling of the same fact: another page exists.
    #[serde(default)]
    pub(crate) next: Option<String>,
}

/// One item of a `HISTORY_PAGE`.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct HistoryItemPayload {
    #[serde(default)]
    pub(crate) turn_id: Option<String>,
    #[serde(default)]
    pub(crate) role: String,
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) content: String,
    #[serde(default, alias = "created_at")]
    pub(crate) timestamp: i64,
}

impl HistoryItemPayload {
    pub(crate) fn into_wire(self, index: usize) -> norte_code_wire::HistoryItem {
        let role = match self.role.as_str() {
            "user" => norte_code_wire::HistorySpeaker::User,
            "assistant" => norte_code_wire::HistorySpeaker::Assistant,
            "tool" => norte_code_wire::HistorySpeaker::Tool,
            _ => norte_code_wire::HistorySpeaker::SystemEvent,
        };
        let kind = if self.kind.is_empty() {
            match role {
                norte_code_wire::HistorySpeaker::User => "user",
                norte_code_wire::HistorySpeaker::Assistant => "assistant",
                norte_code_wire::HistorySpeaker::Tool => "tool",
                norte_code_wire::HistorySpeaker::SystemEvent => "system_event",
            }
            .to_owned()
        } else {
            self.kind
        };
        norte_code_wire::HistoryItem {
            id: self.turn_id.as_ref().map_or_else(
                || format!("history:{index}"),
                |turn| format!("{turn}:{index}"),
            ),
            kind,
            content: self.content,
            created_at: self.timestamp,
            turn_id: self.turn_id,
            role,
            provenance: None,
        }
    }
}

/// A `HISTORY_REWOUND` payload as the host sends it.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct HistoryRewoundPayload {
    #[serde(default)]
    pub(crate) to_turn_id: Option<String>,
    #[serde(default)]
    pub(crate) inclusive: bool,
    #[serde(default)]
    pub(crate) rewound_turn_ids: Vec<String>,
    #[serde(default)]
    pub(crate) side_effects: Vec<HistorySideEffectPayload>,
    #[serde(default)]
    pub(crate) workspace_restored: bool,
}

/// What one tool did in a rewound turn.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct HistorySideEffectPayload {
    #[serde(default)]
    pub(crate) tool_call_id: String,
    #[serde(default)]
    pub(crate) tool_name: String,
    #[serde(default)]
    pub(crate) side_effects: Value,
}

impl HistorySideEffectPayload {
    pub(crate) fn into_wire(self) -> norte_code_wire::SideEffect {
        let side_effects = match self.side_effects {
            Value::String(text) => text,
            Value::Null => String::new(),
            other => other.to_string(),
        };
        norte_code_wire::SideEffect {
            tool_call_id: self.tool_call_id,
            tool_name: self.tool_name,
            side_effects,
        }
    }
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
    UserInputAccepted,
    HistoryPage,
    SessionForked,
    HistoryRewound,
    JobStarted,
    JobProgressDelta,
    JobFinished,
    JobListResult,
    ChildSessionStarted,
    ChildSessionUpdated,
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
            StandardFrameType::UserInputAccepted => Self::UserInputAccepted,
            StandardFrameType::HistoryPage => Self::HistoryPage,
            StandardFrameType::SessionForked => Self::SessionForked,
            StandardFrameType::HistoryRewound => Self::HistoryRewound,
            StandardFrameType::JobStarted => Self::JobStarted,
            StandardFrameType::JobProgressDelta => Self::JobProgressDelta,
            StandardFrameType::JobFinished => Self::JobFinished,
            StandardFrameType::JobListResult => Self::JobListResult,
            StandardFrameType::ChildSessionStarted => Self::ChildSessionStarted,
            StandardFrameType::ChildSessionUpdated => Self::ChildSessionUpdated,
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

/// An error payload.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ErrorPayload {
    #[serde(default)]
    pub(crate) code: String,
    #[serde(default)]
    pub(crate) message: String,
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
    pub(crate) job_id: Option<String>,
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
        Self {
            frame_type,
            payload: Some(frame.payload.clone()),
            session,
            turn,
            job_id: frame.job_id.as_ref().map(ToString::to_string),
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
