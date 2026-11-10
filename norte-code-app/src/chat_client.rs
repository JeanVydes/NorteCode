//! The chat transport: one SSE stream per session, kept open while the
//! conversation page is open, plus the commands the composer sends.
//!
//! The RAEP server allows one attachment at a time, so the page opens the
//! stream once when a session is selected (the stream *is* the attachment)
//! and everything else, history, prompts, rewind, fork, rides the same
//! attachment instead of attaching and detaching around each call.

use crate::api::{ApiClient, ApiError};
use crate::domain::{ConversationItem, HistoryKind, HistoryRole, MessageDelivery, SessionTodo};
use crate::frames;
use crate::wire::UserInputPart;
use norte_code_wire::{JobStatus, JobSummary};

#[derive(serde::Serialize)]
struct RewindRequest<'a> {
    to_turn_id: &'a str,
    inclusive: bool,
    cancel_pending: bool,
    workspace: RewindWorkspace,
}

/// What the rewind does to the workspace, as the wire names it.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum RewindWorkspace {
    Keep,
}

#[derive(serde::Serialize)]
struct ForkRequest<'a> {
    from_turn_id: Option<&'a str>,
    title: Option<&'a str>,
    include: ForkInclude,
}

/// What a fork copies besides the messages.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
struct ForkInclude {
    uploads: bool,
    outputs: bool,
    workspace: bool,
}

#[derive(serde::Serialize)]
struct CompactRequest {
    strategy: CompactStrategy,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum CompactStrategy {
    Summarize,
}

#[derive(serde::Serialize)]
struct PinRequest<'a> {
    turn_ids: &'a [String],
    pinned: bool,
}
use raep_types::{Envelope, FrameType, StandardFrameType, TurnId};
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum JobMutation {
    Started {
        job_id: String,
        command: String,
    },
    Output {
        job_id: String,
        delta: String,
        index: u64,
    },
    Finished {
        job_id: String,
        status: JobStatus,
        exit_code: Option<i32>,
        summary: String,
        output_snapshot: String,
        output_truncated: bool,
        output_bytes_total: u64,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenChannel {
    Text,
    Reasoning,
}

/// Decodes the frames the conversation renders and reports what changed.
///
/// Frames the transcript does not draw (completions, budget notices, retries)
/// are ignored here on purpose: the conversation is history plus live deltas,
/// not a log of every protocol event.
#[allow(clippy::too_many_lines)]
pub fn decode_frame(frame: &Envelope<Value>) -> DecodedFrame {
    let turn_id = frame.turn_id.as_ref().map(ToString::to_string);
    let payload = &frame.payload;
    match &frame.kind {
        FrameType::Standard(StandardFrameType::TokenDelta) => {
            let Ok(delta) = serde_json::from_value::<norte_code_wire::TokenDelta>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            if delta.delta.is_empty() {
                return DecodedFrame::Ignored;
            }
            DecodedFrame::Token {
                turn_id,
                channel: match delta.channel {
                    norte_code_wire::TokenChannel::Text => TokenChannel::Text,
                    norte_code_wire::TokenChannel::Reasoning => TokenChannel::Reasoning,
                    // A tool call's arguments are not answer text. The row is
                    // opened by `TOOL_CALL_STARTED`, which carries them whole;
                    // drawing the fragments here is what painted raw JSON into
                    // the conversation before the call rendered.
                    norte_code_wire::TokenChannel::ToolArguments => return DecodedFrame::Ignored,
                },
                index: delta.index,
                delta: delta.delta,
                timestamp: frame.timestamp,
            }
        }
        FrameType::Standard(StandardFrameType::ToolCallStarted) => {
            let Ok(started) =
                serde_json::from_value::<norte_code_wire::ToolStarted>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::ToolStarted {
                turn_id,
                call_id: started.tool_call_id,
                tool: started.tool_name,
                arguments: started.arguments,
                timestamp: frame.timestamp,
            }
        }
        FrameType::Standard(StandardFrameType::ToolOutputDelta) => {
            let Ok(output) = serde_json::from_value::<norte_code_wire::ToolOutput>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::ToolOutput {
                call_id: output.tool_call_id,
                delta: output.delta,
            }
        }
        FrameType::Standard(StandardFrameType::ToolCallFinished) => {
            let Ok(finished) =
                serde_json::from_value::<norte_code_wire::ToolFinished>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::ToolFinished {
                call_id: finished.tool_call_id,
                failed: finished.status == norte_code_wire::ToolStatus::Error,
                cancelled: finished.status == norte_code_wire::ToolStatus::Cancelled,
                duration_ms: finished.duration_ms,
                result_summary: finished.result_summary,
            }
        }
        FrameType::Standard(StandardFrameType::TurnStateChanged) => {
            let Ok(state) = serde_json::from_value::<frames::TurnState>(payload.clone()) else {
                return DecodedFrame::Ignored;
            };
            match state.state {
                frames::TurnStateName::CallingProvider
                | frames::TurnStateName::ExecutingTool
                | frames::TurnStateName::AwaitingApproval
                | frames::TurnStateName::Queued => DecodedFrame::TurnPhase(state.state),
                frames::TurnStateName::Completed => DecodedFrame::TurnEnded {
                    turn_id,
                    outcome: TurnOutcome::Completed,
                },
                frames::TurnStateName::Cancelled => DecodedFrame::TurnEnded {
                    turn_id,
                    outcome: TurnOutcome::Cancelled,
                },
                frames::TurnStateName::Failed | frames::TurnStateName::Stopped => {
                    DecodedFrame::TurnEnded {
                        turn_id,
                        outcome: TurnOutcome::Failed(
                            state
                                .error
                                .map(|error| error.message)
                                .filter(|message| !message.trim().is_empty()),
                        ),
                    }
                }
            }
        }
        FrameType::Standard(StandardFrameType::HistoryPage) => {
            let Ok(page) = serde_json::from_value::<frames::HistoryPage>(payload.clone()) else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::History(
                page.items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| history_item_to_conversation(item, index))
                    .collect(),
            )
        }
        FrameType::Standard(StandardFrameType::JobStarted) => {
            let Ok(started) =
                serde_json::from_value::<norte_code_wire::JobStarted>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::Job(JobMutation::Started {
                job_id: started.job_id,
                command: started.command,
            })
        }
        FrameType::Standard(StandardFrameType::JobProgressDelta) => {
            let Ok(progress) =
                serde_json::from_value::<norte_code_wire::JobProgress>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::Job(JobMutation::Output {
                job_id: frame
                    .job_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                delta: progress.delta,
                index: progress.index,
            })
        }
        FrameType::Standard(StandardFrameType::JobFinished) => {
            let Ok(finished) =
                serde_json::from_value::<norte_code_wire::JobFinished>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::Job(JobMutation::Finished {
                job_id: finished
                    .job_id
                    .or_else(|| frame.job_id.as_ref().map(ToString::to_string))
                    .unwrap_or_default(),
                status: job_status(finished.status),
                exit_code: finished.exit_code,
                summary: finished.summary,
                output_snapshot: finished.output_snapshot,
                output_truncated: finished.output_truncated,
                output_bytes_total: finished.output_bytes_total,
                duration_ms: finished.duration_ms,
            })
        }
        FrameType::Standard(StandardFrameType::UserInputAccepted) => {
            let Ok(accepted) =
                serde_json::from_value::<norte_code_wire::UserInputAccepted>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::Accepted {
                turn_id: accepted.turn_id,
                queued: accepted.position.is_some(),
            }
        }
        FrameType::Standard(StandardFrameType::SessionForked) => {
            let Ok(forked) =
                serde_json::from_value::<norte_code_wire::SessionForked>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::Forked {
                from: forked.from_session_id,
                from_turn_id: forked.from_turn_id,
                new_session_id: (!frame.session_id.is_empty()).then(|| frame.session_id.clone()),
            }
        }
        FrameType::Standard(StandardFrameType::HistoryRewound) => {
            let Ok(rewound) =
                serde_json::from_value::<norte_code_wire::HistoryRewound>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::Rewound {
                turns: rewound.rewound_turn_ids,
                side_effects: rewound
                    .side_effects
                    .into_iter()
                    .map(|effect| (effect.tool_name, effect.side_effects))
                    .collect(),
            }
        }
        FrameType::Extension(name) if name.ends_with("request_completed") => {
            let Ok(completed) =
                serde_json::from_value::<norte_code_wire::RequestCompleted>(payload.clone())
            else {
                return DecodedFrame::Ignored;
            };
            DecodedFrame::Usage {
                turn_id,
                usage: crate::domain::TurnUsage {
                    input_tokens: completed.usage.input_tokens,
                    output_tokens: completed.usage.output_tokens,
                    cache_read_tokens: completed.usage.cache_read_tokens,
                    cache_write_tokens: completed.usage.cache_write_tokens,
                    cost_usd: completed.cost_usd,
                    duration_ms: completed.duration_ms,
                    first_token_ms: completed.first_token_ms,
                    tokens_per_second: completed.tokens_per_second,
                    cache_hit: completed.cache_hit,
                    provider: completed.provider,
                    model: completed.model,
                },
            }
        }
        FrameType::Extension(name) if name.ends_with("output_part_finished") => {
            match serde_json::from_value::<norte_code_wire::OutputPart>(payload.clone()) {
                Ok(part) => DecodedFrame::OutputPart {
                    id: part.part_id,
                    modality: part.modality,
                    bytes: part.bytes,
                    blob: part.blob,
                    error: part.error,
                },
                Err(_) => DecodedFrame::Ignored,
            }
        }
        FrameType::Extension(name) if name.ends_with("question_asked") => {
            match serde_json::from_value::<norte_code_wire::QuestionAsked>(payload.clone()) {
                Ok(asked) => DecodedFrame::QuestionAsked {
                    turn: asked.turn_id,
                    id: asked.question,
                    prompt: asked.prompt,
                    options: asked.options,
                    multiple: asked.multiple,
                },
                Err(_) => DecodedFrame::Ignored,
            }
        }
        FrameType::Extension(name) if name.ends_with("question_answered") => {
            match serde_json::from_value::<norte_code_wire::QuestionAnswered>(payload.clone()) {
                Ok(answered) => DecodedFrame::QuestionAnswered {
                    id: answered.question,
                },
                Err(_) => DecodedFrame::Ignored,
            }
        }
        FrameType::Extension(name) if name.ends_with("session_todos") => {
            match serde_json::from_value::<norte_code_wire::SessionTodos>(payload.clone()) {
                Ok(todos) => DecodedFrame::Todos(
                    todos
                        .todos
                        .into_iter()
                        .map(|todo| crate::domain::SessionTodo {
                            content: todo.content,
                            status: match todo.status {
                                norte_code_wire::TodoStatus::Pending => {
                                    crate::domain::TodoStatus::Pending
                                }
                                norte_code_wire::TodoStatus::InProgress => {
                                    crate::domain::TodoStatus::InProgress
                                }
                                norte_code_wire::TodoStatus::Completed => {
                                    crate::domain::TodoStatus::Completed
                                }
                            },
                        })
                        .collect(),
                ),
                Err(_) => DecodedFrame::Ignored,
            }
        }
        FrameType::Extension(name) if name.ends_with("compaction_finished") => {
            match serde_json::from_value::<norte_code_wire::CompactionFinished>(payload.clone()) {
                Ok(finished) => DecodedFrame::Compacted {
                    // The host reports the summary, not a flag: a summary with
                    // no error is a compaction that ran.
                    compacted: finished
                        .compacted
                        .unwrap_or_else(|| finished.error.is_none() && finished.summary.is_some()),
                    summary: finished.summary,
                    error: finished.error,
                },
                Err(_) => DecodedFrame::Ignored,
            }
        }
        FrameType::Extension(name) if name.ends_with("compaction_started") => {
            DecodedFrame::Compacting
        }
        FrameType::Extension(name) if name.ends_with("child_session_started") => {
            match serde_json::from_value::<norte_code_wire::ChildSessionStarted>(payload.clone()) {
                Ok(started) => DecodedFrame::ChildStarted {
                    session_id: started.child_session_id,
                    agent_id: started.agent_id,
                    task_summary: started.task_summary,
                },
                Err(_) => DecodedFrame::Ignored,
            }
        }
        FrameType::Extension(name) if name.ends_with("child_session_updated") => {
            match serde_json::from_value::<norte_code_wire::ChildSessionUpdated>(payload.clone()) {
                Ok(updated) => DecodedFrame::ChildUpdated {
                    session_id: updated.child_session_id,
                    state: updated.state,
                },
                Err(_) => DecodedFrame::Ignored,
            }
        }
        FrameType::Extension(name) if name.ends_with("agent_changed") => {
            match serde_json::from_value::<norte_code_wire::AgentChanged>(payload.clone()) {
                Ok(changed) => DecodedFrame::ModelChanged {
                    // The canonical payload names the pair as the agent that
                    // takes over: provider in `id`, model in `version`.
                    provider: (!changed.to_agent.id.is_empty()).then_some(changed.to_agent.id),
                    model: (!changed.to_agent.version.is_empty())
                        .then_some(changed.to_agent.version),
                },
                Err(_) => DecodedFrame::Ignored,
            }
        }
        _ => DecodedFrame::Ignored,
    }
}

fn job_status(status: norte_code_wire::JobStatus) -> JobStatus {
    match status {
        norte_code_wire::JobStatus::Running => JobStatus::Running,
        norte_code_wire::JobStatus::Completed => JobStatus::Completed,
        norte_code_wire::JobStatus::Failed => JobStatus::Failed,
        norte_code_wire::JobStatus::Killed => JobStatus::Killed,
        norte_code_wire::JobStatus::TimedOut => JobStatus::TimedOut,
    }
}

pub fn history_item_to_conversation(item: &frames::HistoryItem, index: usize) -> ConversationItem {
    let role = match item.role {
        frames::HistoryRole::User => HistoryRole::User,
        frames::HistoryRole::Assistant => HistoryRole::Assistant,
        frames::HistoryRole::Tool => HistoryRole::Tool,
        frames::HistoryRole::SystemEvent => HistoryRole::SystemEvent,
    };
    let kind = match item.kind {
        frames::HistoryKind::User => HistoryKind::User,
        frames::HistoryKind::Assistant => HistoryKind::Assistant,
        frames::HistoryKind::Reasoning => HistoryKind::Reasoning,
        frames::HistoryKind::ToolCall | frames::HistoryKind::Tool => HistoryKind::ToolCall,
        frames::HistoryKind::ToolResult => HistoryKind::ToolResult,
        frames::HistoryKind::Refusal => HistoryKind::Refusal,
        frames::HistoryKind::SystemEvent => HistoryKind::SystemEvent,
    };
    ConversationItem {
        id: format!(
            "history:{}:{}:{index}",
            item.turn_id.as_deref().unwrap_or("untracked"),
            item.timestamp
        ),
        turn_id: item.turn_id.clone(),
        role,
        kind,
        content: item.content.clone(),
        created_at: item.timestamp,
        delivery: MessageDelivery::Sent,
        tool: None,
        usage: None,
    }
}

/// What a running turn is doing, in the wire's own words.
///
/// It used to be a second enum with the same four values, translated by a
/// `match` at the one place that decoded it, two names for one state, which
/// is how a new phase ends up handled in one and silently dropped in the other.
pub use crate::frames::TurnStateName as TurnPhase;

/// How a turn ended, as the wire reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed(Option<String>),
}

#[derive(Debug, Clone)]
pub enum DecodedFrame {
    Ignored,
    /// The server accepted the input and named the turn it will run as.
    Accepted {
        turn_id: String,
        /// Whether it waits behind the turn already running.
        queued: bool,
    },
    Token {
        turn_id: Option<String>,
        channel: TokenChannel,
        index: u64,
        delta: String,
        timestamp: i64,
    },
    ToolStarted {
        turn_id: Option<String>,
        call_id: String,
        tool: String,
        arguments: Value,
        timestamp: i64,
    },
    ToolOutput {
        call_id: String,
        delta: String,
    },
    ToolFinished {
        call_id: String,
        failed: bool,
        cancelled: bool,
        duration_ms: Option<u64>,
        result_summary: Option<String>,
    },
    TurnPhase(TurnPhase),
    TurnEnded {
        turn_id: Option<String>,
        outcome: TurnOutcome,
    },
    History(Vec<ConversationItem>),
    Todos(Vec<SessionTodo>),
    /// One model request finished: its cost and speed, rendered under the
    /// answer it produced.
    Usage {
        turn_id: Option<String>,
        usage: crate::domain::TurnUsage,
    },
    Job(JobMutation),
    /// A fork landed: the new session is `new_session_id`.
    Forked {
        /// The session the fork came from and the turn it stopped at.
        from: Option<String>,
        from_turn_id: Option<String>,
        /// The new session, named by the frame's envelope.
        new_session_id: Option<String>,
    },
    /// A rewind landed, with the turns it removed.
    Rewound {
        turns: Vec<String>,
        side_effects: Vec<(String, String)>,
    },
    /// Compaction started for this session.
    Compacting,
    /// Compaction finished.
    Compacted {
        compacted: bool,
        summary: Option<String>,
        error: Option<String>,
    },
    /// A child session was spawned.
    ChildStarted {
        session_id: String,
        agent_id: String,
        task_summary: String,
    },
    /// A child session changed state.
    ChildUpdated {
        session_id: String,
        state: norte_code_wire::ChildState,
    },
    /// The model produced a non-text part (an image, a recording, a file).
    OutputPart {
        id: String,
        modality: String,
        bytes: u64,
        blob: Option<String>,
        error: Option<String>,
    },
    /// The model asked a question and its turn is parked until it is answered.
    QuestionAsked {
        turn: u64,
        id: String,
        prompt: String,
        options: Vec<String>,
        multiple: bool,
    },
    /// A question was answered, by this client or another, so it is no
    /// longer waiting.
    QuestionAnswered {
        id: String,
    },
    /// The session's model changed.
    ModelChanged {
        provider: Option<String>,
        model: Option<String>,
    },
}

/// One `USER_INPUT`: either a prompt or content parts, never both, plus the
/// routing fields the host reads.
#[derive(serde::Serialize)]
#[serde(untagged)]
enum UserInputRequest {
    Prompt {
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        replaces_turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        queue_policy: Option<String>,
    },
    Content {
        content: Vec<UserInputPart>,
        #[serde(skip_serializing_if = "Option::is_none")]
        replaces_turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        queue_policy: Option<String>,
    },
}

impl UserInputRequest {
    fn prompt(
        prompt: Option<String>,
        content: Option<Vec<UserInputPart>>,
        replaces_turn_id: Option<String>,
        queue_policy: Option<String>,
    ) -> Option<Self> {
        match (prompt, content) {
            (Some(prompt), _) if !prompt.is_empty() => Some(Self::Prompt {
                prompt,
                replaces_turn_id,
                queue_policy,
            }),
            (_, Some(content)) if !content.is_empty() => Some(Self::Content {
                content,
                replaces_turn_id,
                queue_policy,
            }),
            _ => None,
        }
    }
}

/// The commands the conversation page sends over its own client.
impl ApiClient {
    /// Sends a prompt on the session's open stream. Text-only prompts take
    /// the plain form; anything with attachments goes as content parts.
    pub async fn send_prompt(
        &self,
        session_id: &str,
        text: Option<&str>,
        content: Option<Vec<UserInputPart>>,
        replaces_turn_id: Option<&str>,
        queue_mode: &str,
    ) -> Result<TurnId, ApiError> {
        let turn_id = TurnId::generate();
        // `prompt` and `content` are exclusive: the wire accepts exactly one,
        // and a request carrying both is rejected as `INVALID_FRAME`. A prompt
        // with attachments therefore becomes the first text part of its
        // `content`, keeping the order the person wrote.
        let (prompt, content) = match (text, content) {
            (Some(text), Some(mut parts)) => {
                parts.insert(0, crate::wire::UserInputPart::text(text.to_owned()));
                (None, Some(parts))
            }
            other => other,
        };
        let request = UserInputRequest::prompt(
            prompt.map(str::to_owned),
            content,
            replaces_turn_id.map(str::to_owned),
            replaces_turn_id.is_none().then(|| queue_mode.to_owned()),
        )
        .ok_or_else(|| {
            ApiError::InvalidResponse("A message needs text or an attachment.".to_owned())
        })?;
        let payload = serde_json::to_value(request)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::UserInput),
            payload,
            crate::api::now_millis()?,
        );
        frame.session_id = session_id.to_owned();
        frame.turn_id = Some(turn_id.clone());
        let response = self.post_command(session_id, &frame).await?;
        if response.accepted {
            let accepted = response
                .events
                .iter()
                .find(|event| {
                    event.kind == FrameType::Standard(StandardFrameType::UserInputAccepted)
                })
                .and_then(|event| {
                    event.payload["turn_id"]
                        .as_str()
                        .and_then(|id| TurnId::new(id).ok())
                });
            Ok(accepted.unwrap_or(turn_id))
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "The message was rejected.".to_owned(),
                |error| error.message,
            )))
        }
    }

    /// Rewinds a session to `to_turn_id` and reports which turns were dropped.
    pub async fn rewind(
        &self,
        session_id: &str,
        to_turn_id: &str,
        inclusive: bool,
    ) -> Result<Vec<String>, ApiError> {
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionRewind),
            serde_json::to_value(RewindRequest {
                to_turn_id,
                inclusive,
                cancel_pending: false,
                workspace: RewindWorkspace::Keep,
            })
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            crate::api::now_millis()?,
        );
        frame.session_id = session_id.to_owned();
        let response = self.post_command(session_id, &frame).await?;
        let rewound = response
            .events
            .iter()
            .find(|event| event.kind == FrameType::Standard(StandardFrameType::HistoryRewound))
            .and_then(|event| event.payload["rewound_turn_ids"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        Ok(rewound)
    }

    /// Forks a session, optionally from a specific turn, and returns the new id.
    pub async fn fork(
        &self,
        session_id: &str,
        from_turn_id: Option<&str>,
        title: Option<&str>,
    ) -> Result<String, ApiError> {
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionFork),
            serde_json::to_value(ForkRequest {
                from_turn_id,
                title,
                include: ForkInclude::default(),
            })
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            crate::api::now_millis()?,
        );
        frame.session_id = session_id.to_owned();
        let response = self.post_command(session_id, &frame).await?;
        for event in &response.events {
            if event.kind == FrameType::Standard(StandardFrameType::SessionForked) {
                return Ok(event.session_id.clone());
            }
        }
        Err(ApiError::InvalidResponse(
            "The server did not return the forked session.".to_owned(),
        ))
    }

    /// Compacts a session now.
    pub async fn compact(&self, session_id: &str) -> Result<(), ApiError> {
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionCompact),
            serde_json::to_value(CompactRequest {
                strategy: CompactStrategy::Summarize,
            })
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            crate::api::now_millis()?,
        );
        frame.session_id = session_id.to_owned();
        self.post_command(session_id, &frame).await?;
        Ok(())
    }

    /// Pins or unpins turns so compaction keeps them verbatim.
    pub async fn pin_turns(
        &self,
        session_id: &str,
        turn_ids: &[String],
        pinned: bool,
    ) -> Result<Vec<String>, ApiError> {
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::ContextPin),
            serde_json::to_value(PinRequest { turn_ids, pinned })
                .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            crate::api::now_millis()?,
        );
        frame.session_id = session_id.to_owned();
        let response = self.post_command(session_id, &frame).await?;
        Ok(response
            .events
            .iter()
            .find(|event| event.kind == FrameType::Standard(StandardFrameType::ContextUpdated))
            .and_then(|event| event.payload["pinned_turn_ids"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect())
    }
}

/// Blocks the calling task until the next frame arrives.
pub async fn next_frame(
    stream: &mut crate::sse::SessionEventStream,
) -> Result<Option<Envelope<Value>>, ApiError> {
    stream.next().await
}

/// Groups jobs the way the Jobs page renders them.
pub fn upsert_job(jobs: &mut Vec<JobSummary>, mutation: &JobMutation) {
    match mutation {
        JobMutation::Started { job_id, command } => {
            if let Some(job) = jobs.iter_mut().find(|job| job.job_id == *job_id) {
                job.status = JobStatus::Running;
                job.command.clone_from(command);
            } else {
                jobs.insert(
                    0,
                    JobSummary {
                        job_id: job_id.clone(),
                        command: command.clone(),
                        status: JobStatus::Running,
                        exit_code: None,
                        output_snapshot: Some(String::new()),
                        output_bytes_total: 0,
                        output_truncated: false,
                        duration_ms: None,
                    },
                );
            }
        }
        JobMutation::Output {
            job_id,
            delta,
            index,
        } => {
            if let Some(job) = jobs.iter_mut().find(|job| job.job_id == *job_id) {
                if *index < job.output_bytes_total {
                    return;
                }
                let output = job.output_snapshot.get_or_insert_with(String::new);
                if *index > job.output_bytes_total {
                    job.output_truncated = true;
                }
                output.push_str(delta);
                job.output_bytes_total =
                    index.saturating_add(u64::try_from(delta.len()).unwrap_or(u64::MAX));
            }
        }
        JobMutation::Finished {
            job_id,
            status,
            exit_code,
            summary,
            output_snapshot,
            output_truncated,
            output_bytes_total,
            duration_ms,
        } => {
            let finished = JobSummary {
                job_id: job_id.clone(),
                command: summary.clone(),
                status: *status,
                exit_code: *exit_code,
                output_snapshot: Some(output_snapshot.clone()),
                output_bytes_total: *output_bytes_total,
                output_truncated: *output_truncated,
                duration_ms: Some(*duration_ms),
            };
            if let Some(job) = jobs.iter_mut().find(|job| job.job_id == *job_id) {
                *job = finished;
            } else {
                jobs.insert(0, finished);
            }
        }
    }
}

#[cfg(test)]
#[path = "chat_client_tests.rs"]
mod tests;
