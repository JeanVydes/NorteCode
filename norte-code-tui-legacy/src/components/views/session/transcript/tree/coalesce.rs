//! The open turn and its pending buffers: how the coalesced text
//! becomes part of the tree when a call, a replay or the end of the log
//! needs them.

use norte_code_wire::MessageRole;

use super::{
    Residual, ToolSlot, TranscriptNode, TranscriptNodeKind, TreeBuilder, TurnKey, TurnState,
};

impl TreeBuilder {
    /// The pending text of the model, as a node.
    pub(super) fn flush_assistant(&mut self) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        if turn.assistant.is_empty() {
            return;
        }
        let text = std::mem::take(&mut turn.assistant);
        turn.children
            .push(TranscriptNode::new(TranscriptNodeKind::Message {
                role: MessageRole::Assistant,
                text,
                chips: Vec::new(),
                timestamp: None,
                queued: false,
            }));
    }

    /// The pending reasoning, as a node.
    pub(super) fn flush_reasoning(&mut self) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        if turn.reasoning.is_empty() {
            return;
        }
        let text = std::mem::take(&mut turn.reasoning);
        turn.children
            .push(TranscriptNode::new(TranscriptNodeKind::Reasoning { text }));
    }

    /// Everything pending becomes a node.
    pub(super) fn flush(&mut self) {
        self.flush_assistant();
        self.flush_reasoning();
    }

    /// The turn in progress becomes a node of the tree.
    pub(super) fn close_turn(&mut self) {
        let Some(mut turn) = self.turn.take() else {
            return;
        };
        Self::flush_assistant_of(&mut turn);
        Self::flush_reasoning_of(&mut turn);
        turn.children.extend(self.take_errors_for());
        let mut node = TranscriptNode::new(TranscriptNodeKind::Turn { key: turn.key });
        for child in turn.children {
            node.push(child);
        }
        let root = self.roots.len();
        self.roots.push(node);
        for (call_id, child) in turn.tools {
            self.tools.insert(
                call_id,
                ToolSlot {
                    root: Some(root),
                    child,
                },
            );
        }
    }

    fn flush_assistant_of(turn: &mut TurnState) {
        if turn.assistant.is_empty() {
            return;
        }
        let text = std::mem::take(&mut turn.assistant);
        turn.children
            .push(TranscriptNode::new(TranscriptNodeKind::Message {
                role: MessageRole::Assistant,
                text,
                chips: Vec::new(),
                timestamp: None,
                queued: false,
            }));
    }

    fn flush_reasoning_of(turn: &mut TurnState) {
        if turn.reasoning.is_empty() {
            return;
        }
        let text = std::mem::take(&mut turn.reasoning);
        turn.children
            .push(TranscriptNode::new(TranscriptNodeKind::Reasoning { text }));
    }

    /// The errors that arrived while the turn was open.
    fn take_errors_for(&mut self) -> Vec<TranscriptNode> {
        let mut errors = Vec::new();
        let mut kept = Vec::with_capacity(self.residual.len());
        for entry in std::mem::take(&mut self.residual) {
            match entry {
                Residual::Error { message } => errors.push(message),
                other @ Residual::UserMessage { .. } => kept.push(other),
            }
        }
        self.residual = kept;
        errors
            .into_iter()
            .map(|message| {
                TranscriptNode::new(TranscriptNodeKind::Error {
                    code: None,
                    message,
                })
            })
            .collect()
    }

    /// The tree, with everything no turn claimed laid down in the order it
    /// arrived: user messages and errors follow one another in
    /// stream order, which a reload cannot discover for them.
    pub(super) fn finish(mut self) -> Vec<TranscriptNode> {
        self.flush();
        self.close_turn();
        let mut roots = std::mem::take(&mut self.roots);
        let mut pending_users: Vec<TranscriptNode> = Vec::new();
        let flush_users = |roots: &mut Vec<TranscriptNode>, users: &mut Vec<TranscriptNode>| {
            if users.is_empty() {
                return;
            }
            let mut turn = TranscriptNode::new(TranscriptNodeKind::Turn {
                key: TurnKey::Named("pending".to_owned()),
            });
            for child in std::mem::take(users) {
                turn.push(child);
            }
            roots.push(turn);
        };
        for entry in std::mem::take(&mut self.residual) {
            match entry {
                Residual::UserMessage {
                    text,
                    chips,
                    queued,
                } => {
                    pending_users.push(TranscriptNode::new(TranscriptNodeKind::Message {
                        role: MessageRole::User,
                        text,
                        chips,
                        timestamp: None,
                        queued,
                    }));
                }
                Residual::Error { message } => {
                    flush_users(&mut roots, &mut pending_users);
                    roots.push(TranscriptNode::new(TranscriptNodeKind::Error {
                        code: None,
                        message,
                    }));
                }
            }
        }
        flush_users(&mut roots, &mut pending_users);
        self.roots = roots;
        Self::merge_adjacent_turns(std::mem::take(&mut self.roots))
    }

    /// A turn split by what the log interleaved with it is one turn again:
    /// the history of a turn and its live events, or a status written while
    /// the turn was open, all land in one node.
    fn merge_adjacent_turns(roots: Vec<TranscriptNode>) -> Vec<TranscriptNode> {
        let mut merged: Vec<TranscriptNode> = Vec::with_capacity(roots.len());
        for node in roots {
            if node.is_turn()
                && let Some(last) = merged.last_mut()
                && let Some(key) = node.turn_key()
                && last.turn_key() == Some(key)
            {
                for child in node.take_children() {
                    last.push(child);
                }
                continue;
            }
            merged.push(node);
        }
        merged
    }
}
