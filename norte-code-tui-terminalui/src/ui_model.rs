//! The client-visible state of one attached session.
//!
//! The view reads this; only the client layer mutates it. Everything here
//! is plain data, free of any notion of terminal or layout.

use crate::theme::ThemeKind;
use norte_code_wire::SessionSummary;

/// Where the active turn is.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TurnState {
    /// Nothing is being produced.
    Idle,
    /// A turn is being dispatched to the server.
    Connecting,
    /// The server is thinking.
    Thinking,
    /// A turn is streaming in.
    Streaming,
    /// A question is waiting for the user's answer.
    AwaitingAnswer,
}

/// Where the connection to the raep server stands.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionAttachmentState {
    /// Never attached, or attachment was torn down.
    Disconnected,
    /// Attaching now; no messages will flow until this resolves.
    Connecting,
    /// Attached and streaming.
    Attached,
    /// The attach attempt failed; a reason is shown instead of the chat.
    Failed {
        /// What went wrong, in words a person can act on.
        reason: String,
    },
}

/// The effort level the reasoning selector offers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReasoningLevel {
    /// Let the model decide.
    Auto,
    /// Low effort.
    Low,
    /// Medium effort.
    Medium,
    /// High effort.
    High,
}

/// What to do with a message sent while a turn is running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SendMode {
    /// Steer the running turn.
    Steer,
    /// Wait behind the running turn.
    Queue,
    /// Cancel the running turn, then run the message.
    Interrupt,
    /// Refuse to send while a turn is running.
    Reject,
}

/// The preferences the client keeps across sessions.
#[derive(Clone)]
pub(crate) struct Preferences {
    /// Which built-in theme to paint.
    pub(crate) theme: ThemeKind,
    /// Reasoning effort, sent with every turn.
    pub(crate) reasoning: ReasoningLevel,
    /// What to do with a message sent while a turn is running.
    pub(crate) send_mode: SendMode,
}

/// The one big piece of state the interface reads from.
pub(crate) struct UiModel {
    pub(crate) preferences: Preferences,
    pub(crate) attachment: SessionAttachmentState,
    pub(crate) turn: TurnState,
    pub(crate) sessions: Vec<SessionSummary>,
    /// The session the interface is attached to; set on attach.
    pub(crate) active: Option<norte_code_wire::SessionKey>,
}

impl UiModel {
    /// A fresh model for a client that never attached yet.
    pub(crate) fn detached() -> Self {
        Self {
            preferences: Preferences {
                theme: ThemeKind::Dark,
                reasoning: ReasoningLevel::Auto,
                send_mode: SendMode::Steer,
            },
            attachment: SessionAttachmentState::Disconnected,
            turn: TurnState::Idle,
            sessions: Vec::new(),
            active: None,
        }
    }
}