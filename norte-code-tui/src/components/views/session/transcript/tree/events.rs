//! The events of the live stream, and what each one adds to the tree.

use norte_code_wire::{Delta, ReplayedKind, ServerEventKind};

use super::{ToolNode, TranscriptNodeKind, TreeBuilder, TurnKey};

impl TreeBuilder {
    #[allow(clippy::too_many_lines)]
    pub(super) fn push_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::Delta { turn, delta } => match delta {
                Delta::Text { text, .. } | Delta::Refusal { text, .. } => {
                    self.flush_reasoning();
                    self.open_turn(TurnKey::Live(*turn));
                    self.turn_mut().assistant.push_str(text);
                }
                Delta::Reasoning { text, .. } => {
                    self.flush_assistant();
                    self.open_turn(TurnKey::Live(*turn));
                    self.turn_mut().reasoning.push_str(text);
                }
                Delta::ToolArguments { .. } => {}
                Delta::Media { bytes, last, .. } => {
                    self.flush();
                    self.push_node(TranscriptNodeKind::Media {
                        bytes: *bytes,
                        last: *last,
                    });
                }
            },
            ServerEventKind::ToolStarted {
                turn,
                call_id,
                tool,
                arguments,
                source,
                ..
            } => {
                self.flush();
                self.open_turn(TurnKey::Live(*turn));
                self.push_tool(ToolNode {
                    call_id: call_id.clone(),
                    name: tool.clone(),
                    arguments: arguments.clone(),
                    source: source.clone(),
                    ..ToolNode::default()
                });
            }
            ServerEventKind::ToolOutput {
                turn,
                call_id,
                stream,
                text,
                ..
            } => {
                self.flush();
                self.push_chunk(call_id, *stream, text, Some(TurnKey::Live(*turn)));
            }
            ServerEventKind::ToolFinished {
                turn,
                call_id,
                tool,
                is_error,
                detached,
                error,
                output,
                duration_ms,
                ..
            } => {
                self.flush();
                self.finish_tool(
                    ToolNode {
                        call_id: call_id.clone(),
                        name: tool.clone(),
                        status: Some(norte_code_wire::ToolStatus::from_outcome(
                            *detached, *is_error,
                        )),
                        duration_ms: Some(*duration_ms),
                        output: output.clone(),
                        error: error.clone(),
                        ..ToolNode::default()
                    },
                    Some(TurnKey::Live(*turn)),
                );
            }
            ServerEventKind::JobStarted {
                job_id,
                tool_call_id,
                ..
            } => {
                self.jobs.insert(job_id.clone(), tool_call_id.clone());
            }
            ServerEventKind::JobProgressDelta {
                job_id,
                stream,
                delta,
                ..
            } => {
                let Some(call_id) = self.jobs.get(job_id).cloned() else {
                    return;
                };
                self.flush();
                self.push_chunk(&call_id, *stream, delta, None);
            }
            ServerEventKind::MessageReplayed {
                role, kind, text, ..
            } => match kind {
                ReplayedKind::Reasoning => {
                    self.flush_assistant();
                    self.turn_or_open().reasoning.push_str(text);
                }
                ReplayedKind::ToolCall => {
                    self.flush();
                    self.push_replayed_call(text);
                }
                ReplayedKind::ToolResult => {
                    self.flush();
                    self.push_tool_result(text.clone());
                }
                ReplayedKind::Text | ReplayedKind::Refusal => {
                    self.flush();
                    self.push_node(TranscriptNodeKind::Message {
                        role: *role,
                        text: text.clone(),
                        chips: Vec::new(),
                        timestamp: None,
                        queued: false,
                    });
                }
            },
            ServerEventKind::RetryScheduled {
                attempt,
                delay_ms,
                reason,
                ..
            } => {
                self.flush();
                self.push_node(TranscriptNodeKind::Retry {
                    attempt: *attempt,
                    delay_ms: Some(*delay_ms),
                    reason: reason.clone(),
                });
            }
            ServerEventKind::RetryStarted { attempt, .. } => {
                self.flush();
                self.push_node(TranscriptNodeKind::Retry {
                    attempt: *attempt,
                    delay_ms: None,
                    reason: String::new(),
                });
            }
            ServerEventKind::ReasoningDowngraded {
                from, to, reason, ..
            } => {
                self.flush();
                self.push_node(TranscriptNodeKind::Downgrade {
                    from: from.clone(),
                    to: to.clone(),
                    reason: reason.clone(),
                });
            }
            ServerEventKind::Error { code, message } => {
                self.flush();
                self.push_node(TranscriptNodeKind::Error {
                    code: Some(*code),
                    message: message.clone(),
                });
            }
            _ => {}
        }
    }
}
