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
        let usage = self.take_usage_for(&turn.key);
        turn.children.extend(usage);
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

    /// The footer of the turn that just closed: what its requests reported,
    /// accumulated into the one footer the turn shows, plus the errors that
    /// arrived while it was open. Only live turns carry a number; anything
    /// else stays queued for whoever closes a matching turn.
    fn take_usage_for(&mut self, key: &super::TurnKey) -> Vec<TranscriptNode> {
        let Some(number) = (match key {
            super::TurnKey::Live(number) => Some(*number),
            super::TurnKey::Named(_) => None,
        }) else {
            return Vec::new();
        };
        let mut footers = Vec::new();
        let mut errors = Vec::new();
        let mut kept = Vec::with_capacity(self.residual.len());
        for entry in std::mem::take(&mut self.residual) {
            match entry {
                Residual::Usage {
                    turn: Some(turn),
                    usage,
                } if turn == number => footers.push(usage),
                Residual::Error { message } => errors.push(message),
                other => kept.push(other),
            }
        }
        self.residual = kept;
        let mut taken = Vec::new();
        if let Some(usage) = merged_usage(footers) {
            taken.push(TranscriptNode::new(TranscriptNodeKind::Usage { usage }));
        }
        for message in errors {
            taken.push(TranscriptNode::new(TranscriptNodeKind::Error {
                code: None,
                message,
            }));
        }
        taken
    }

    /// The tree, with everything no turn claimed laid down in the order it
    /// arrived: user messages, errors and footers follow one another in
    /// stream order, which a reload cannot discover for them.
    pub(super) fn finish(mut self) -> Vec<TranscriptNode> {
        self.flush();
        self.close_turn();
        let mut roots = std::mem::take(&mut self.roots);
        let mut pending_footers: Vec<(Option<u64>, crate::ui_model::UsageSummary)> = Vec::new();
        let flush_footers =
            |roots: &mut Vec<TranscriptNode>,
             pending: &mut Vec<(Option<u64>, crate::ui_model::UsageSummary)>| {
                for (_, usage) in merged_pending(std::mem::take(pending)) {
                    roots.push(TranscriptNode::new(TranscriptNodeKind::Usage { usage }));
                }
            };
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
        let mut pending_users: Vec<TranscriptNode> = Vec::new();
        for entry in std::mem::take(&mut self.residual) {
            match entry {
                Residual::UserMessage {
                    text,
                    chips,
                    queued,
                } => {
                    flush_footers(&mut roots, &mut pending_footers);
                    pending_users.push(TranscriptNode::new(TranscriptNodeKind::Message {
                        role: MessageRole::User,
                        text,
                        chips,
                        timestamp: None,
                        queued,
                    }));
                }
                Residual::Error { message } => {
                    flush_footers(&mut roots, &mut pending_footers);
                    flush_users(&mut roots, &mut pending_users);
                    roots.push(TranscriptNode::new(TranscriptNodeKind::Error {
                        code: None,
                        message,
                    }));
                }
                Residual::Usage { turn, usage } => pending_footers.push((turn, usage)),
            }
        }
        flush_users(&mut roots, &mut pending_users);
        flush_footers(&mut roots, &mut pending_footers);
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

/// The footers of one turn, accumulated into the single footer it shows.
fn merged_usage(
    footers: Vec<crate::ui_model::UsageSummary>,
) -> Option<crate::ui_model::UsageSummary> {
    let mut merged: Option<crate::ui_model::UsageSummary> = None;
    for footer in footers {
        match &mut merged {
            None => merged = Some(footer),
            Some(accumulated) => accumulated.absorb(footer),
        }
    }
    merged
}

/// The footers no turn claimed, accumulated per turn: the footers of a turn
/// travel together in the log, so the ones that share a turn become one.
fn merged_pending(
    footers: Vec<(Option<u64>, crate::ui_model::UsageSummary)>,
) -> Vec<(Option<u64>, crate::ui_model::UsageSummary)> {
    let mut merged: Vec<(Option<u64>, crate::ui_model::UsageSummary)> = Vec::new();
    for (turn, usage) in footers {
        match merged.last_mut() {
            Some((last_turn, accumulated)) if *last_turn == turn => accumulated.absorb(usage),
            _ => merged.push((turn, usage)),
        }
    }
    merged
}
