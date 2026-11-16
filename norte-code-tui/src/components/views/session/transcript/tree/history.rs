//! The items of the history, and what each one adds to the tree.

use norte_code_wire::{HistorySpeaker, MessageRole};

use super::{Chip, Residual, TranscriptNodeKind, TreeBuilder, TurnKey};

impl TreeBuilder {
    pub(super) fn push_user(&mut self, text: &str, chips: &[Chip], queued: bool) {
        self.flush();
        self.close_turn();
        self.residual.push(Residual::UserMessage {
            text: text.to_owned(),
            chips: chips.to_vec(),
            queued,
        });
    }

    pub(super) fn push_history(&mut self, item: &norte_code_wire::HistoryItem) {
        match item.kind.as_str() {
            "tool_result" => {
                self.flush();
                self.push_tool_result(item.content.clone());
            }
            "tool_call" => {
                self.flush();
                self.push_replayed_call(&item.content);
            }
            "reasoning" => {
                self.flush_assistant();
                if let Some(turn_id) = item.turn_id.as_deref() {
                    self.open_turn(TurnKey::from_history(turn_id));
                }
                self.turn_or_open().reasoning.push_str(&item.content);
            }
            _ => {
                self.flush();
                let role = match item.role {
                    HistorySpeaker::User => MessageRole::User,
                    HistorySpeaker::Assistant => MessageRole::Assistant,
                    HistorySpeaker::Tool => MessageRole::Tool,
                    HistorySpeaker::SystemEvent => MessageRole::System,
                };
                if let Some(turn_id) = item.turn_id.as_deref() {
                    self.open_turn(TurnKey::from_history(turn_id));
                }
                self.push_node(TranscriptNodeKind::Message {
                    role,
                    text: item.content.clone(),
                    chips: Vec::new(),
                    timestamp: Some(item.created_at),
                    queued: false,
                });
            }
        }
    }
}
