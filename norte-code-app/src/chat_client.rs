//! The chat transport: one SSE stream per session, kept open while the
//! conversation page is open, plus the commands the composer sends.
//!
//! The RAEP server allows one attachment at a time, so the page opens the
//! stream once when a session is selected (the stream *is* the attachment)
//! and everything else, history, prompts, rewind, fork, rides the same
//! attachment instead of attaching and detaching around each call.

use crate::api::{ApiClient, ApiError};
use crate::domain::{ConversationItem, HistoryKind, HistoryRole, MessageDelivery, SessionTodo};
use crate::wire::UserInputPart;
use norte_code_client as client;
use norte_code_wire::{JobStatus, JobSummary, ServerEventKind};

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

/// Decodes one protocol event into what the conversation renders.
///
/// The frame itself was already decoded by `norte-code-client`: this only
/// decides what the page draws, so there is exactly one place that knows the
/// RAEP payloads and exactly one that knows the UI.
#[allow(clippy::too_many_lines)]
pub fn decode_event(decoded: client::DecodedEvent) -> DecodedFrame {
    let turn_id = decoded.turn_id.clone();
    let timestamp = decoded.timestamp;
    let frame_session = decoded
        .event
        .session
        .as_ref()
        .map(|session| session.as_str().to_owned());
    match decoded.event.kind {
        ServerEventKind::Delta { delta, .. } => match delta {
            norte_code_wire::Delta::Text { index, text }
            | norte_code_wire::Delta::Refusal { index, text } => {
                if text.is_empty() {
                    return DecodedFrame::Ignored;
                }
                DecodedFrame::Token {
                    turn_id,
                    channel: TokenChannel::Text,
                    index: u64::from(index),
                    delta: text,
                    timestamp,
                }
            }
            norte_code_wire::Delta::Reasoning { index, text } => {
                if text.is_empty() {
                    return DecodedFrame::Ignored;
                }
                DecodedFrame::Token {
                    turn_id,
                    channel: TokenChannel::Reasoning,
                    index: u64::from(index),
                    delta: text,
                    timestamp,
                }
            }
            // A tool call's arguments are not answer text. The row is opened
            // by `ToolStarted`, which carries them whole; drawing the
            // fragments here is what painted raw JSON into the conversation.
            norte_code_wire::Delta::ToolArguments { .. } | norte_code_wire::Delta::Media { .. } => {
                DecodedFrame::Ignored
            }
        },
        ServerEventKind::ToolStarted {
            call_id,
            tool,
            arguments,
            ..
        } => DecodedFrame::ToolStarted {
            turn_id,
            call_id,
            tool,
            arguments: arguments.unwrap_or(Value::Null),
            timestamp,
        },
        ServerEventKind::ToolOutput { call_id, text, .. } => DecodedFrame::ToolOutput {
            call_id,
            delta: text,
        },
        ServerEventKind::ToolFinished {
            call_id,
            is_error,
            detached,
            error,
            output,
            duration_ms,
            ..
        } => DecodedFrame::ToolFinished {
            call_id,
            failed: is_error,
            cancelled: detached,
            duration_ms: Some(duration_ms),
            result_summary: error.or(output),
        },
        ServerEventKind::TurnPhase { phase, .. } => DecodedFrame::TurnPhase(phase),
        ServerEventKind::TurnFinished { .. } => DecodedFrame::TurnEnded {
            turn_id,
            outcome: TurnOutcome::Completed,
        },
        ServerEventKind::TurnStopped { reason } => DecodedFrame::TurnEnded {
            turn_id,
            outcome: if reason == "cancelled" {
                TurnOutcome::Cancelled
            } else {
                TurnOutcome::Failed(Some(reason))
            },
        },
        ServerEventKind::InputAccepted {
            turn_id: accepted,
            queued,
        } => DecodedFrame::Accepted {
            turn_id: accepted,
            queued,
        },
        ServerEventKind::HistoryPage { items, .. } => DecodedFrame::History(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| history_item_to_conversation(item, index))
                .collect(),
        ),
        ServerEventKind::SessionForked { from_turn_id, .. } => DecodedFrame::Forked {
            from: frame_session.clone(),
            from_turn_id,
            new_session_id: frame_session,
        },
        ServerEventKind::HistoryRewound {
            rewound_turn_ids,
            side_effects,
            ..
        } => DecodedFrame::Rewound {
            turns: rewound_turn_ids,
            side_effects: side_effects
                .into_iter()
                .map(|effect| (effect.tool_name, effect.side_effects))
                .collect(),
        },
        ServerEventKind::SessionTodos { todos } => DecodedFrame::Todos(
            todos
                .into_iter()
                .map(|todo| SessionTodo {
                    content: todo.content,
                    status: match todo.status {
                        norte_code_wire::TodoStatus::Pending => crate::domain::TodoStatus::Pending,
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
        ServerEventKind::JobStarted {
            job_id, command, ..
        } => DecodedFrame::Job(JobMutation::Started { job_id, command }),
        ServerEventKind::JobProgressDelta {
            job_id,
            delta,
            index,
            ..
        } => DecodedFrame::Job(JobMutation::Output {
            job_id,
            delta,
            index,
        }),
        ServerEventKind::JobFinished { job, .. } => DecodedFrame::Job(JobMutation::Finished {
            status: job_status_from_info(&job),
            job_id: job.id,
            exit_code: job.exit_code,
            summary: job.label,
            output_snapshot: job.output,
            output_truncated: job.dropped,
            output_bytes_total: job.output_bytes_total,
            duration_ms: job.elapsed_ms,
        }),
        ServerEventKind::CompactionStarted { .. } => DecodedFrame::Compacting,
        ServerEventKind::CompactionFinished {
            compacted,
            summary,
            error,
            ..
        } => DecodedFrame::Compacted {
            compacted,
            summary,
            error,
        },
        ServerEventKind::ChildSessionStarted {
            child_session_id,
            agent_id,
            task_summary,
            ..
        } => DecodedFrame::ChildStarted {
            session_id: child_session_id,
            agent_id,
            task_summary,
        },
        ServerEventKind::ChildSessionUpdated {
            child_session_id,
            state,
            ..
        } => DecodedFrame::ChildUpdated {
            session_id: child_session_id,
            state,
        },
        ServerEventKind::QuestionAsked {
            turn,
            question,
            prompt,
            options,
            multiple,
        } => DecodedFrame::QuestionAsked {
            turn,
            id: question,
            prompt,
            options,
            multiple,
        },
        ServerEventKind::QuestionAnswered { question } => {
            DecodedFrame::QuestionAnswered { id: question }
        }
        ServerEventKind::ModelChanged { provider, model }
        | ServerEventKind::ModelQueued { provider, model } => DecodedFrame::ModelChanged {
            provider: (!provider.is_empty()).then_some(provider),
            model: (!model.is_empty()).then_some(model),
        },
        ServerEventKind::OutputPartFinished {
            part_id,
            modality,
            bytes,
            blob,
            error,
            ..
        } => DecodedFrame::OutputPart {
            id: part_id,
            modality: serde_json::to_value(modality)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default(),
            bytes,
            blob,
            error,
        },
        ServerEventKind::RequestReconstructed {
            usage,
            cost_usd,
            duration_ms,
            first_token_ms,
            tokens_per_second,
            cache_hit,
            provider,
            model,
            ..
        } => DecodedFrame::Usage {
            turn_id,
            usage: crate::domain::TurnUsage {
                input_tokens: usage.input_tokens.unwrap_or(0),
                output_tokens: usage.output_tokens.unwrap_or(0),
                cache_read_tokens: usage.cache_read_tokens.unwrap_or(0),
                cache_write_tokens: usage.cache_write_tokens.unwrap_or(0),
                cost_usd: cost_usd.or(usage.cost_usd),
                duration_ms,
                first_token_ms,
                tokens_per_second,
                cache_hit,
                provider,
                model,
            },
        },
        _ => DecodedFrame::Ignored,
    }
}

/// The job status a finished job is shown with.
fn job_status_from_info(job: &norte_code_wire::JobInfo) -> JobStatus {
    match job.state {
        norte_code_wire::JobState::Running => JobStatus::Running,
        norte_code_wire::JobState::Finished => JobStatus::Completed,
        norte_code_wire::JobState::Cancelled if job.timed_out => JobStatus::TimedOut,
        norte_code_wire::JobState::Cancelled => JobStatus::Failed,
    }
}

pub fn history_item_to_conversation(
    item: &norte_code_wire::HistoryItem,
    index: usize,
) -> ConversationItem {
    let role = match item.role {
        norte_code_wire::HistorySpeaker::User => HistoryRole::User,
        norte_code_wire::HistorySpeaker::Assistant => HistoryRole::Assistant,
        norte_code_wire::HistorySpeaker::Tool => HistoryRole::Tool,
        norte_code_wire::HistorySpeaker::SystemEvent => HistoryRole::SystemEvent,
    };
    let kind = match item.kind.as_str() {
        "user" => HistoryKind::User,
        "assistant" => HistoryKind::Assistant,
        "reasoning" => HistoryKind::Reasoning,
        "tool_call" | "tool" => HistoryKind::ToolCall,
        "tool_result" => HistoryKind::ToolResult,
        "refusal" => HistoryKind::Refusal,
        _ => HistoryKind::SystemEvent,
    };
    ConversationItem {
        id: format!("history:{}:{}:{index}", item.id, item.created_at),
        turn_id: item.turn_id.clone(),
        role,
        kind,
        content: item.content.clone(),
        created_at: item.created_at,
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
pub use norte_code_wire::TurnPhaseName as TurnPhase;

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
            let accepted =
                response
                    .events
                    .iter()
                    .find_map(|event| match client::decode_frame(event) {
                        Ok(decoded) => match decoded.event.kind {
                            ServerEventKind::InputAccepted { turn_id, .. } => {
                                TurnId::new(turn_id).ok()
                            }
                            _ => None,
                        },
                        Err(_) => None,
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
        Ok(response
            .events
            .iter()
            .filter_map(|event| client::decode_frame(event).ok())
            .find_map(|decoded| match decoded.event.kind {
                ServerEventKind::HistoryRewound {
                    rewound_turn_ids, ..
                } => Some(rewound_turn_ids),
                _ => None,
            })
            .unwrap_or_default())
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
        response
            .events
            .iter()
            .filter_map(|event| client::decode_frame(event).ok())
            .find(|decoded| matches!(decoded.event.kind, ServerEventKind::SessionForked { .. }))
            .and_then(|decoded| {
                decoded
                    .event
                    .session
                    .map(|session| session.as_str().to_owned())
            })
            .ok_or_else(|| {
                ApiError::InvalidResponse(
                    "The server did not return the forked session.".to_owned(),
                )
            })
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
) -> Result<Option<client::DecodedEvent>, ApiError> {
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
