//! Builds the transcript tree from the log.
//!
//! The log is flat and stays flat: this is a view of it, rebuilt every frame.
//! A turn holds the reasoning, the messages, the tool calls, the usage footer
//! and the errors that belong to it.

use std::collections::HashMap;

use norte_code_wire::MessageRole;

pub(super) use super::node::{ToolNode, TranscriptNode, TranscriptNodeKind, TurnKey};
pub(super) use crate::chips::Chip;
use crate::ui_model::TranscriptItem;

mod coalesce;
mod events;
mod history;

/// Builds the tree.
pub(crate) struct TranscriptTree;

impl TranscriptTree {
    /// The tree the log describes.
    pub(crate) fn build(items: &[TranscriptItem]) -> Vec<TranscriptNode> {
        let mut builder = TreeBuilder::default();
        for item in items {
            builder.push(item);
        }
        builder.finish()
    }
}

/// The turn being built.
#[derive(Default)]
pub(super) struct TurnState {
    key: TurnKey,
    children: Vec<TranscriptNode>,
    assistant: String,
    reasoning: String,
    /// The tool calls of this turn, and where each one sits among them.
    tools: HashMap<String, usize>,
}

/// Where a tool call lives in the tree, so its later events find it.
#[derive(Clone, Copy)]
pub(super) struct ToolSlot {
    /// The turn that holds the call, once it has been closed.
    root: Option<usize>,
    /// The call's place among its turn's children.
    child: usize,
}

/// What remains of the stream while no turn claims it.
///
/// User messages, errors and usage footers all land in one queue, in the
/// order they arrived, so a render that happens before the turn exists
/// cannot reorder them or scatter them across the page.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Residual {
    UserMessage {
        text: String,
        chips: Vec<Chip>,
        queued: bool,
    },
    Error {
        message: String,
    },
    Usage {
        turn: Option<u64>,
        usage: crate::ui_model::UsageSummary,
    },
}

#[derive(Default)]
struct TreeBuilder {
    roots: Vec<TranscriptNode>,
    turn: Option<TurnState>,
    /// The stream's leftover items, in the order they arrived.
    residual: Vec<Residual>,
    /// The tool calls, and where each one lives.
    tools: HashMap<String, ToolSlot>,
    /// The call each detached job belongs to.
    jobs: HashMap<String, String>,
}

impl TreeBuilder {
    fn push(&mut self, item: &TranscriptItem) {
        match item {
            TranscriptItem::UserMessage {
                text,
                chips,
                queued,
                ..
            } => self.push_user(text, chips, *queued),
            TranscriptItem::Status(text) => {
                self.flush();
                self.push_node(TranscriptNodeKind::Notice { text: text.clone() });
            }
            TranscriptItem::History(item) => self.push_history(item),
            TranscriptItem::ServerEvent(event) => self.push_event(&event.kind),
            TranscriptItem::Error(message) => {
                // Kept for the open turn, like the usage footers, so a
                // reload cannot clump the errors before what caused them.
                self.residual.push(Residual::Error {
                    message: message.clone(),
                });
            }
            TranscriptItem::Usage(usage) => {
                // A footer is kept for the turn that is still open and
                // accumulated into it when the turn closes, so a reload that
                // reorders the log cannot strand it after later turns.
                self.residual.push(Residual::Usage {
                    turn: usage.turn,
                    usage: usage.clone(),
                });
            }
        }
    }

    /// A tool call the history replayed: its name and arguments are in the
    /// text the host wrote.
    fn push_replayed_call(&mut self, text: &str) {
        let (name, arguments) = split_tool_call(text);
        self.push_tool(ToolNode {
            name,
            arguments,
            replayed: true,
            ..ToolNode::default()
        });
    }

    /// The result of a tool call: it belongs to the call that is open.
    fn push_tool_result(&mut self, text: String) {
        if let Some(slot) = self.last_tool()
            && let TranscriptNodeKind::Tool(tool) = self.tool_at(slot).kind_mut()
            && tool.output.is_none()
        {
            tool.output = Some(text);
            return;
        }
        self.push_tool(ToolNode {
            output: Some(text),
            replayed: true,
            ..ToolNode::default()
        });
    }

    /// One chunk of output of a running call.
    fn push_chunk(
        &mut self,
        call_id: &str,
        stream: norte_code_wire::OutputStream,
        text: &str,
        turn: Option<TurnKey>,
    ) {
        if !self.tools.contains_key(call_id) {
            let Some(turn) = turn else {
                return;
            };
            self.open_turn(turn);
            self.push_tool(ToolNode {
                call_id: call_id.to_owned(),
                ..ToolNode::default()
            });
        }
        let Some(slot) = self.tools.get(call_id).copied() else {
            return;
        };
        let TranscriptNodeKind::Tool(tool) = self.tool_at(slot).kind_mut() else {
            return;
        };
        for line in text.lines() {
            tool.chunks.push((stream, line.to_owned()));
        }
    }

    /// A call that ended: its status, its duration and its result.
    fn finish_tool(&mut self, finished: ToolNode, turn: Option<TurnKey>) {
        if !self.tools.contains_key(&finished.call_id) {
            if let Some(turn) = turn {
                self.open_turn(turn);
            }
            self.push_tool(finished);
            return;
        }
        let Some(slot) = self.tools.get(&finished.call_id).copied() else {
            return;
        };
        let TranscriptNodeKind::Tool(tool) = self.tool_at(slot).kind_mut() else {
            return;
        };
        tool.status = finished.status;
        tool.duration_ms = finished.duration_ms;
        tool.output = finished.output;
        tool.error = finished.error;
        if !finished.name.is_empty() {
            tool.name = finished.name;
        }
    }

    /// The tool call at a slot.
    fn tool_at(&mut self, slot: ToolSlot) -> &mut TranscriptNode {
        if let Some(root) = slot.root {
            return &mut self.roots[root];
        }
        let turn = self.turn.as_mut().expect("the turn is open");
        &mut turn.children[slot.child]
    }

    /// A new tool call, in the turn being built.
    fn push_tool(&mut self, tool: ToolNode) {
        let call_id = tool.call_id.clone();
        let node = TranscriptNode::new(TranscriptNodeKind::Tool(tool));
        if let Some(turn) = self.turn.as_mut() {
            let child = turn.children.len();
            turn.children.push(node);
            turn.tools.insert(call_id.clone(), child);
            self.tools.insert(call_id, ToolSlot { root: None, child });
            return;
        }
        let root = self.roots.len();
        self.roots.push(node);
        self.tools.insert(
            call_id,
            ToolSlot {
                root: Some(root),
                child: 0,
            },
        );
    }

    /// A node in the turn being built, or at the root when there is none.
    fn push_node(&mut self, kind: TranscriptNodeKind) {
        let node = TranscriptNode::new(kind);
        match self.turn.as_mut() {
            Some(turn) => turn.children.push(node),
            None => self.roots.push(node),
        }
    }

    /// The tool call that is open, if any.
    fn last_tool(&self) -> Option<ToolSlot> {
        for (root, node) in self.roots.iter().enumerate().rev() {
            if matches!(node.kind(), TranscriptNodeKind::Tool(_)) {
                return Some(ToolSlot {
                    root: Some(root),
                    child: 0,
                });
            }
            if let Some(child) = node
                .children()
                .iter()
                .enumerate()
                .rev()
                .find(|(_, child)| matches!(child.kind(), TranscriptNodeKind::Tool(_)))
                .map(|(child, _)| child)
            {
                return Some(ToolSlot {
                    root: Some(root),
                    child,
                });
            }
        }
        None
    }

    /// The turn being built.
    fn turn_mut(&mut self) -> &mut TurnState {
        self.turn
            .as_mut()
            .expect("the turn was opened before it was used")
    }

    /// The turn being built, opened without a name when there is none: what
    /// a replayed item needs, since a replay names no turn.
    fn turn_or_open(&mut self) -> &mut TurnState {
        if self.turn.is_none() {
            self.turn = Some(TurnState {
                key: TurnKey::Named("replay".to_owned()),
                ..TurnState::default()
            });
        }
        self.turn_mut()
    }

    /// The turn of a key: the turn in progress continues when it is the same
    /// one, so the history of a turn and its live events land together.
    fn open_turn(&mut self, key: TurnKey) {
        if self.turn.as_ref().is_some_and(|turn| turn.key == key) {
            return;
        }
        self.close_turn();
        let mut turn = TurnState {
            key,
            ..TurnState::default()
        };
        if let Some(index) = self
            .residual
            .iter()
            .position(|entry| matches!(entry, Residual::UserMessage { .. }))
        {
            let entry = self.residual.remove(index);
            if let Residual::UserMessage {
                text,
                chips,
                queued,
            } = entry
            {
                turn.children
                    .push(TranscriptNode::new(TranscriptNodeKind::Message {
                        role: MessageRole::User,
                        text,
                        chips,
                        timestamp: None,
                        queued,
                    }));
            }
        }
        self.turn = Some(turn);
    }
}

/// Splits a replayed call into its name and its arguments.
fn split_tool_call(text: &str) -> (String, Option<serde_json::Value>) {
    let trimmed = text.trim_start();
    let Some(end) = trimmed.find(char::is_whitespace) else {
        return (trimmed.to_owned(), None);
    };
    let name = trimmed[..end].to_owned();
    let rest = trimmed[end..].trim();
    if rest.is_empty() {
        return (name, None);
    }
    let arguments = rest
        .starts_with('{')
        .then(|| serde_json::from_str::<serde_json::Value>(rest).ok())
        .flatten();
    (name, arguments)
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
