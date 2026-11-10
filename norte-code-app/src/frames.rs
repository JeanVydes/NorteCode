//! The decode shapes for Norte Code's own frames.
//!
//! These are not the shared wire: they are the views this client folds into a
//! conversation, and `raep-types`' history and turn-state payloads describe the
//! same frames with different fields. They live together here so a frame's
//! shape is one definition, and the payloads that *are* shared come from
//! `norte-code-wire` directly.

use serde::Deserialize;

/// A `TURN_STATE_CHANGED` payload.
#[derive(Debug, Deserialize)]
pub struct TurnState {
    pub state: TurnStateName,
    #[serde(default)]
    pub error: Option<ErrorBody>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TurnStateName {
    Queued,
    CallingProvider,
    ExecutingTool,
    AwaitingApproval,
    Completed,
    Cancelled,
    Failed,
    Stopped,
}

#[derive(Debug, Deserialize)]
pub struct ErrorBody {
    #[serde(default)]
    pub message: String,
}

/// A `HISTORY_PAGE` payload.
#[derive(Debug, Deserialize)]
pub struct HistoryPage {
    #[serde(default)]
    pub items: Vec<HistoryItem>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryItem {
    #[serde(default)]
    pub turn_id: Option<String>,
    pub role: HistoryRole,
    pub kind: HistoryKind,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRole {
    User,
    Assistant,
    Tool,
    SystemEvent,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    User,
    Assistant,
    Reasoning,
    ToolCall,
    ToolResult,
    Refusal,
    Tool,
    SystemEvent,
}
