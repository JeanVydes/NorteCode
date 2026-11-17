//! The nodes the transcript is drawn from.
//!
//! The log is flat; the tree is what the reader sees: a turn, and inside it
//! the reasoning, the messages, the tool calls and the errors that belong
//! to that turn.

use norte_code_wire::{MessageRole, OutputStream, ToolStatus};
use serde_json::Value;

use crate::chips::Chip;

/// The key that groups the nodes of one turn.
///
/// The host names a turn `turn_<number>` in the history and numbers the same
/// turn in the events it streams, so the two names of a turn are one key.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum TurnKey {
    /// A turn the live stream numbered.
    Live(u64),
    /// A turn the history named with an id of its own.
    Named(String),
}

impl Default for TurnKey {
    fn default() -> Self {
        Self::Named("live".to_owned())
    }
}

impl TurnKey {
    /// The key a history item names.
    pub(crate) fn from_history(turn_id: &str) -> Self {
        turn_id
            .strip_prefix("turn_")
            .and_then(|number| number.parse::<u64>().ok())
            .map_or_else(|| Self::Named(turn_id.to_owned()), Self::Live)
    }
}

/// One node of the transcript tree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TranscriptNode {
    kind: TranscriptNodeKind,
    children: Vec<TranscriptNode>,
}

impl TranscriptNode {
    /// A node with no children.
    pub(crate) fn new(kind: TranscriptNodeKind) -> Self {
        Self {
            kind,
            children: Vec::new(),
        }
    }

    /// What the node is.
    pub(crate) fn kind(&self) -> &TranscriptNodeKind {
        &self.kind
    }

    /// What the node is, to change it.
    pub(crate) fn kind_mut(&mut self) -> &mut TranscriptNodeKind {
        &mut self.kind
    }

    /// The nodes under this one.
    pub(crate) fn children(&self) -> &[TranscriptNode] {
        &self.children
    }

    /// The nodes under this one, taking ownership.
    pub(crate) fn take_children(mut self) -> Vec<TranscriptNode> {
        std::mem::take(&mut self.children)
    }

    /// A node under this one.
    pub(crate) fn push(&mut self, child: TranscriptNode) {
        self.children.push(child);
    }

    /// Whether the node is a turn.
    pub(crate) fn is_turn(&self) -> bool {
        matches!(self.kind, TranscriptNodeKind::Turn { .. })
    }

    /// The key of the turn, when the node is one.
    pub(crate) fn turn_key(&self) -> Option<&TurnKey> {
        match &self.kind {
            TranscriptNodeKind::Turn { key } => Some(key),
            _ => None,
        }
    }
}

/// What a node is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TranscriptNodeKind {
    /// A turn: everything the model did in one round trip.
    Turn { key: TurnKey },
    /// The reasoning the model wrote.
    Reasoning { text: String },
    /// A message: what the reader said, or what the model answered.
    Message {
        role: MessageRole,
        text: String,
        chips: Vec<Chip>,
        /// When it was said, when the reader wants the times.
        timestamp: Option<i64>,
        /// The message was queued behind the running turn and has not been
        /// dispatched to it yet.
        queued: bool,
    },
    /// A tool call, with everything it produced.
    Tool(ToolNode),
    /// An error: from the host, with its code, or from the reader's own
    /// view of a failed request.
    Error {
        code: Option<norte_code_wire::ErrorCode>,
        message: String,
    },
    /// A notice: a model changed, a job started, a job ended.
    Notice { text: String },
    /// A retry the host scheduled, or the retry that started.
    Retry {
        attempt: u32,
        delay_ms: Option<u64>,
        reason: String,
    },
    /// The reasoning level the host lowered.
    Downgrade {
        from: String,
        to: String,
        reason: String,
    },
    /// A media part the model produced.
    Media { bytes: u64, last: bool },
}

/// One tool call and everything it produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ToolNode {
    /// The call's id: what ties its events together.
    pub(crate) call_id: String,
    /// The tool's name.
    pub(crate) name: String,
    /// The arguments the call ran with.
    pub(crate) arguments: Option<Value>,
    /// The file's contents before the call, when the host captured them.
    pub(crate) source: Option<String>,
    /// The output the call streamed while it ran.
    pub(crate) chunks: Vec<(OutputStream, String)>,
    /// How the call ended, when it ended.
    pub(crate) status: Option<ToolStatus>,
    /// How long it ran.
    pub(crate) duration_ms: Option<u64>,
    /// The result it returned.
    pub(crate) output: Option<String>,
    /// The reason it failed.
    pub(crate) error: Option<String>,
    /// Whether the call was replayed from the history rather than streamed.
    pub(crate) replayed: bool,
}

#[cfg(test)]
#[path = "node_tests.rs"]
mod tests;
