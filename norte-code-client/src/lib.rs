//! Transport-neutral client contract for Norte Code.
//!
//! The client protocol is deliberately expressed in terms of
//! `norte-code-protocol` requests and events. An in-process adapter and a
//! remote RAEP adapter can therefore be used by the same CLI without moving
//! the agent loop into the client.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use futures_core::Stream;
use futures_util::stream::{self, StreamExt};
use norte_code_protocol::{ClientRequest, ServerEvent};
use raep_types::{Envelope, FrameType, StandardFrameType};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio_stream::iter;

/// A stream of events answering one client request.
pub type EventStream = Pin<Box<dyn Stream<Item = ServerEvent> + Send>>;

/// An error raised before a request can produce an event stream.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{message}")]
pub struct ClientError {
    message: String,
}

impl ClientError {
    /// Creates a client error with a user-facing message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the error message without formatting it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A transport-independent Norte Code client.
///
/// Implementations may submit requests in-process, over RAEP WebSocket, or
/// through another transport. The stream remains the same protocol-level
/// event stream in every case.
#[async_trait]
pub trait AgentClient: Send + Sync {
    /// Submits one request and returns its event stream.
    async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError>;
}

/// HTTP client for the `NorteCode` RAEP command endpoint.
///
/// The transport returns canonical RAEP envelopes, while the compatibility
/// extension carries the original `NorteCode` event until all consumers migrate.
pub struct RemoteClient {
    http: reqwest::Client,
    base_url: String,
    bearer_token: Option<HeaderValue>,
    agent_id: String,
    default_session: Option<String>,
    attached_sessions: Mutex<HashSet<String>>,
    session_seq: Arc<Mutex<HashMap<String, u64>>>,
    session_info: Mutex<HashMap<String, norte_code_protocol::SessionInfo>>,
    session_reasoning: Mutex<HashMap<String, norte_code_protocol::Reasoning>>,
    session_stats: Mutex<HashMap<String, RemoteSessionStats>>,
    session_jobs: Mutex<HashMap<String, Vec<norte_code_protocol::JobInfo>>>,
}

/// Usage and context totals cached from a remote session.
#[derive(Clone, Debug)]
pub struct RemoteSessionStats {
    /// Total input tokens reported by the host.
    pub input_tokens: u64,
    /// Total output tokens reported by the host.
    pub output_tokens: u64,
    /// Total provider cache-read tokens.
    pub cache_read_tokens: u64,
    /// Total provider cache-write tokens.
    pub cache_write_tokens: u64,
    /// Total provider-reported cost.
    pub cost_usd: f64,
    /// Number of reported requests.
    pub requests: u64,
    /// Context tokens in the latest request.
    pub context_used: u64,
    /// Declared context window, when available.
    pub context_window: Option<u64>,
}

impl RemoteClient {
    /// Creates a client using a server base URL such as
    /// `http://127.0.0.1:8080`.
    ///
    /// # Errors
    ///
    /// Returns an error when the bearer token is not a valid header value.
    pub fn new(
        base_url: impl Into<String>,
        bearer_token: Option<&str>,
        default_session: Option<String>,
    ) -> Result<Self, ClientError> {
        let original_token = bearer_token;
        let bearer_token = original_token
            .map(|token| {
                let mut value = b"Bearer ".to_vec();
                value.extend_from_slice(token.as_bytes());
                HeaderValue::from_bytes(&value)
                    .map_err(|error| ClientError::new(format!("invalid bearer token: {error}")))
            })
            .transpose()?;
        Ok(Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            bearer_token,
            agent_id: "norte-code".to_owned(),
            default_session,
            attached_sessions: Mutex::new(HashSet::new()),
            session_seq: Arc::new(Mutex::new(HashMap::new())),
            session_info: Mutex::new(HashMap::new()),
            session_reasoning: Mutex::new(HashMap::new()),
            session_stats: Mutex::new(HashMap::new()),
            session_jobs: Mutex::new(HashMap::new()),
        })
    }

    /// Selects the agent identifier used for RAEP attachments.
    #[must_use]
    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = agent_id.into();
        self
    }

    /// Fetches the public RAEP discovery information.
    ///
    /// # Errors
    ///
    /// Returns an error when the service cannot be reached or returns a
    /// non-success response.
    pub async fn info(&self) -> Result<serde_json::Value, ClientError> {
        let mut request = self.http.get(format!("{}/raep/v1/info", self.base_url));
        if let Some(token) = &self.bearer_token {
            request = request.header(AUTHORIZATION, token.clone());
        }

        let response = request
            .send()
            .await
            .map_err(|error| ClientError::new(format!("could not reach service: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::new(format!(
                "service discovery returned HTTP {status}"
            )));
        }
        response.json().await.map_err(|error| {
            ClientError::new(format!("invalid service discovery response: {error}"))
        })
    }

    /// Returns the last session metadata received from the service.
    #[must_use]
    pub fn session_info(&self, session: &str) -> Option<norte_code_protocol::SessionInfo> {
        self.session_info
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
    }

    /// Returns the last usage and context totals received for a session.
    #[must_use]
    pub fn session_stats(&self, session: &str) -> Option<RemoteSessionStats> {
        self.session_stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
    }

    /// Returns the last background-job list received for a session.
    #[must_use]
    pub fn session_jobs(&self, session: &str) -> Vec<norte_code_protocol::JobInfo> {
        self.session_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the last session-scoped reasoning setting sent to the host.
    #[must_use]
    pub fn session_reasoning(&self, session: &str) -> Option<norte_code_protocol::Reasoning> {
        self.session_reasoning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
    }

    /// Returns the last RAEP sequence observed for a session.
    #[must_use]
    pub fn last_seq(&self, session: &str) -> Option<u64> {
        self.session_seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .copied()
    }

    /// Records a frame's sequence for its session, keeping the maximum seen.
    fn note_seq(&self, session: &str, seq: u64) {
        let mut seqs = self
            .session_seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        seqs.entry(session.to_owned())
            .and_modify(|current| *current = (*current).max(seq))
            .or_insert(seq);
    }

    /// Records the sequence of one raw RAEP frame, if it carries one.
    fn note_frame_seq(&self, frame: &Value) {
        let Some(seq) = frame.get("seq").and_then(Value::as_u64) else {
            return;
        };
        let session = frame
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        if let Some(session) = session {
            self.note_seq(session, seq);
        }
    }

    fn session_for(&self, request: &ClientRequest) -> String {
        session_from_request(request)
            .or_else(|| self.default_session.clone())
            .unwrap_or_else(|| "default".to_owned())
    }

    fn command_url(&self, session_id: &str) -> String {
        format!(
            "{}/raep/v1/sessions/{}/commands",
            self.base_url,
            percent_encode(session_id)
        )
    }

    async fn submit_http(&self, request: ClientRequest) -> Result<EventStream, ClientError> {
        if let norte_code_protocol::ClientRequestKind::SetReasoning {
            session,
            scope: norte_code_protocol::ReasoningScope::Session,
            reasoning,
        } = &request.kind
        {
            self.session_reasoning
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(session.as_str().to_owned(), reasoning.clone());
        }
        if let Some(session) = session_from_request(&request) {
            self.ensure_attached(session.as_str()).await?;
            if matches!(
                &request.kind,
                norte_code_protocol::ClientRequestKind::SendMessage { .. }
            ) {
                return self.submit_streaming(session.as_str(), request).await;
            }
        }
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(self.command_url(&self.session_for(&request)))
            .headers(headers)
            .json(&Envelope::new(
                FrameType::Extension("norte_code.command".to_owned()),
                serde_json::to_value(request)
                    .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
                timestamp_millis(),
            ))
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP request failed: {error}")))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(response_error(&body)));
        }
        let frames = body
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| ClientError::new("RAEP response omitted events"))?;
        let events = frames
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        for frame in frames {
            self.note_frame_seq(frame);
        }
        self.remember_sessions(&events);
        Ok(Box::pin(iter(events)))
    }

    fn events_url(&self, session_id: &str) -> String {
        format!(
            "{}/raep/v1/sessions/{}/events",
            self.base_url,
            percent_encode(session_id)
        )
    }

    #[allow(clippy::too_many_lines)]
    async fn submit_streaming(
        &self,
        session_id: &str,
        request: ClientRequest,
    ) -> Result<EventStream, ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        if let Some(seq) = self.last_seq(session_id) {
            headers.insert(
                HeaderName::from_static("last-event-id"),
                HeaderValue::from_str(&seq.to_string())
                    .map_err(|error| ClientError::new(format!("invalid cursor: {error}")))?,
            );
        }
        let response = self
            .http
            .get(self.events_url(session_id))
            .headers(headers.clone())
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP event stream failed: {error}")))?;
        if !response.status().is_success() {
            return Err(ClientError::new(format!(
                "RAEP event stream returned HTTP {}",
                response.status()
            )));
        }

        let command_url = self.command_url(session_id);
        let mut command_headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            command_headers.insert(AUTHORIZATION, token.clone());
        }
        let command = self
            .http
            .post(command_url)
            .headers(command_headers)
            .json(&Envelope::new(
                FrameType::Extension("norte_code.command".to_owned()),
                serde_json::to_value(request)
                    .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
                timestamp_millis(),
            ))
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP command failed: {error}")))?;
        if !command.status().is_success() {
            let status = command.status();
            let body: Value = command.json().await.unwrap_or(Value::Null);
            return Err(ClientError::new(format!(
                "RAEP command returned HTTP {status}: {}",
                response_error(&body)
            )));
        }

        let session = norte_code_protocol::SessionKey::new(session_id);
        let session_seq = Arc::clone(&self.session_seq);
        let ack_http = self.http.clone();
        let ack_url = self.command_url(session_id);
        let ack_token = self.bearer_token.clone();
        let bytes = response.bytes_stream();
        let stream = stream::unfold(
            (
                bytes,
                Vec::new(),
                VecDeque::<(ServerEvent, bool)>::new(),
                session,
                false,
                session_seq,
            ),
            move |(mut bytes, mut buffer, mut pending, session, mut finished, session_seq)| {
                let ack_http = ack_http.clone();
                let ack_url = ack_url.clone();
                let ack_token = ack_token.clone();
                async move {
                    loop {
                        if let Some((event, terminal)) = pending.pop_front() {
                            return Some((
                                event,
                                (
                                    bytes,
                                    buffer,
                                    pending,
                                    session,
                                    finished || terminal,
                                    session_seq,
                                ),
                            ));
                        }
                        if finished {
                            return None;
                        }
                        let drained = drain_sse_buffer(&mut buffer);
                        let mut last_applied = None;
                        for (event, terminal, seq) in drained {
                            if let Some(seq) = seq {
                                let mut seqs = session_seq
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                match seq_step(seqs.get(session.as_str()).copied(), seq) {
                                    SeqStep::Duplicate => continue,
                                    SeqStep::Gap { expected } => {
                                        drop(seqs);
                                        finished = true;
                                        pending.push_back((
                                            ServerEvent {
                                                request_id: 0,
                                                session: Some(session.clone()),
                                                kind: norte_code_protocol::ServerEventKind::Error {
                                                    code: norte_code_protocol::ErrorCode::Internal,
                                                    message: format!(
                                                        "RAEP sequence gap: expected {expected}, received {seq}; reconnecting from {}",
                                                        expected.saturating_sub(1)
                                                    ),
                                                },
                                            },
                                            true,
                                        ));
                                        break;
                                    }
                                    SeqStep::Apply => {}
                                }
                                seqs.insert(session.as_str().to_owned(), seq);
                                drop(seqs);
                                last_applied = Some(seq);
                            }
                            pending.push_back((event, terminal));
                        }
                        if let Some(seq) = last_applied {
                            let mut headers = HeaderMap::new();
                            if let Some(token) = &ack_token {
                                headers.insert(AUTHORIZATION, token.clone());
                            }
                            let http = ack_http.clone();
                            let url = ack_url.clone();
                            tokio::spawn(async move {
                                let _ = http
                                    .post(url)
                                    .headers(headers)
                                    .json(&Envelope::new(
                                        FrameType::Standard(StandardFrameType::Ack),
                                        serde_json::json!({"last_seq": seq}),
                                        timestamp_millis(),
                                    ))
                                    .send()
                                    .await;
                            });
                        }
                        if finished {
                            continue;
                        }
                        match bytes.next().await {
                            Some(Ok(chunk)) => {
                                buffer.extend_from_slice(&chunk);
                            }
                            Some(Err(_)) | None => {
                                if pending.is_empty() {
                                    return None;
                                }
                            }
                        }
                    }
                }
            },
        );
        Ok(Box::pin(stream))
    }

    fn remember_sessions(&self, events: &[ServerEvent]) {
        let mut cache = self
            .session_info
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stats = self
            .session_stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut jobs = self
            .session_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for event in events {
            match &event.kind {
                norte_code_protocol::ServerEventKind::SessionList { sessions } => {
                    for session in sessions {
                        cache.insert(session.session.as_str().to_owned(), session.clone());
                    }
                }
                norte_code_protocol::ServerEventKind::SessionCreated { title } => {
                    if let Some(key) = &event.session {
                        cache.insert(
                            key.as_str().to_owned(),
                            norte_code_protocol::SessionInfo {
                                session: key.clone(),
                                title: title.clone(),
                                provider: None,
                                model: None,
                                created_at: None,
                                phase: norte_code_protocol::Phase::Idle,
                            },
                        );
                    }
                }
                norte_code_protocol::ServerEventKind::SessionRenamed { title } => {
                    if let Some(key) = &event.session
                        && let Some(info) = cache.get_mut(key.as_str())
                    {
                        info.title = Some(title.clone());
                    }
                }
                norte_code_protocol::ServerEventKind::ModelChanged { provider, model }
                | norte_code_protocol::ServerEventKind::ModelQueued { provider, model } => {
                    if let Some(key) = &event.session
                        && let Some(info) = cache.get_mut(key.as_str())
                    {
                        info.provider = Some(provider.clone());
                        info.model = Some(model.clone());
                    }
                }
                norte_code_protocol::ServerEventKind::SessionStats { .. } => {
                    if let Some(key) = &event.session
                        && let norte_code_protocol::ServerEventKind::SessionStats {
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_write_tokens,
                            cost_usd,
                            requests,
                            context_used,
                            context_window,
                        } = &event.kind
                    {
                        stats.insert(
                            key.as_str().to_owned(),
                            RemoteSessionStats {
                                input_tokens: *input_tokens,
                                output_tokens: *output_tokens,
                                cache_read_tokens: *cache_read_tokens,
                                cache_write_tokens: *cache_write_tokens,
                                cost_usd: *cost_usd,
                                requests: *requests,
                                context_used: *context_used,
                                context_window: *context_window,
                            },
                        );
                    }
                }
                norte_code_protocol::ServerEventKind::JobList { jobs: listed } => {
                    if let Some(key) = &event.session {
                        jobs.insert(key.as_str().to_owned(), listed.clone());
                    }
                }
                _ => {}
            }
        }
    }

    async fn ensure_attached(&self, session_id: &str) -> Result<(), ClientError> {
        if self
            .attached_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(session_id)
        {
            return Ok(());
        }
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(self.command_url(session_id))
            .headers(headers)
            .json(&Envelope::new(
                FrameType::Standard(StandardFrameType::SessionAttach),
                serde_json::json!({
                    "agent_id": &self.agent_id,
                    "create_if_missing": false,
                    "resume_from_seq": self.last_seq(session_id),
                }),
                timestamp_millis(),
            ))
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP attach failed: {error}")))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP attach response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(response_error(&body)));
        }
        if let Some(frames) = body.get("events").and_then(Value::as_array) {
            for frame in frames {
                self.note_frame_seq(frame);
            }
        }
        self.attached_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.to_owned());
        Ok(())
    }
}

#[async_trait]
impl AgentClient for RemoteClient {
    async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError> {
        self.submit_http(request).await
    }
}

/// Whether an arriving sequence can be applied, is a duplicate, or skipped a
/// frame the client never saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeqStep {
    Apply,
    Duplicate,
    Gap { expected: u64 },
}

fn seq_step(previous: Option<u64>, seq: u64) -> SeqStep {
    match previous {
        Some(previous) if seq <= previous => SeqStep::Duplicate,
        Some(previous) if seq > previous.saturating_add(1) => SeqStep::Gap {
            expected: previous.saturating_add(1),
        },
        _ => SeqStep::Apply,
    }
}

/// Drains complete SSE blocks from `buffer`, in wire order.
///
/// Each complete block becomes one event; a trailing partial block stays in
/// the buffer for the next chunk. Blocks that are not `data:` frames or
/// that do not decode are skipped, so a keepalive or a foreign event never
/// wedges the stream. Terminal events (`TurnFinished`, `TurnStopped`) are
/// flagged but do not stop the drain: events the server sent after the
/// terminal in the same flush must still reach the caller.
fn drain_sse_buffer(buffer: &mut Vec<u8>) -> Vec<(ServerEvent, bool, Option<u64>)> {
    let mut events = Vec::new();
    while let Some(end) = buffer.windows(2).position(|window| window == b"\n\n") {
        let block = buffer[..end].to_vec();
        buffer.drain(..end + 2);
        let Ok(block) = std::str::from_utf8(&block) else {
            continue;
        };
        let Some(data) = block.lines().find_map(|line| line.strip_prefix("data: ")) else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let seq = frame.get("seq").and_then(Value::as_u64);
        let Ok(event) = server_event_from_frame(&frame) else {
            continue;
        };
        let terminal = matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::TurnFinished { .. }
                | norte_code_protocol::ServerEventKind::TurnStopped { .. }
        );
        events.push((event, terminal, seq));
    }
    events
}

/// Decodes a standard RAEP frame from its canonical payload.
///
/// Returns `Ok(None)` for a frame this adapter has no faithful standard
/// mapping for, so the caller can fall back to the `NorteCode` extension.
#[allow(clippy::too_many_lines)]
fn standard_event_from_frame(frame: &Value) -> Result<Option<ServerEvent>, ClientError> {
    let Some(frame_type) = frame.get("type").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(payload) = frame.get("payload") else {
        return Ok(None);
    };
    let session = frame
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(norte_code_protocol::SessionKey::new);
    let turn = frame
        .get("turn_id")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let event = |kind| ServerEvent {
        request_id: 0,
        session: session.clone(),
        kind,
    };
    let kind = match frame_type {
        "TOKEN_DELTA" => {
            let index = u32::try_from(number(payload, "index")?).unwrap_or(u32::MAX);
            let text = required_string(payload, "delta")?;
            let delta = if payload.get("channel").and_then(Value::as_str) == Some("reasoning") {
                norte_code_protocol::Delta::Reasoning { index, text }
            } else {
                norte_code_protocol::Delta::Text { index, text }
            };
            event(norte_code_protocol::ServerEventKind::Delta { turn, delta })
        }
        "TURN_STATE_CHANGED" => match required_string(payload, "state")?.as_str() {
            "QUEUED" => event(norte_code_protocol::ServerEventKind::TurnStarted { turn }),
            "COMPLETED" => event(norte_code_protocol::ServerEventKind::TurnFinished {
                turn,
                phase: norte_code_protocol::Phase::Idle,
                committed: true,
            }),
            "CANCELLED" => event(norte_code_protocol::ServerEventKind::TurnStopped {
                reason: payload
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("cancelled")
                    .to_owned(),
            }),
            _ => return Ok(None),
        },
        "TOOL_CALL_STARTED" => event(norte_code_protocol::ServerEventKind::ToolStarted {
            turn,
            call_id: required_string(payload, "tool_call_id")?,
            tool: required_string(payload, "tool_name")?,
            arguments: payload.get("arguments").cloned(),
            source: None,
        }),
        "TOOL_OUTPUT_DELTA" => event(norte_code_protocol::ServerEventKind::ToolOutput {
            turn,
            call_id: required_string(payload, "tool_call_id")?,
            tool: required_string(payload, "tool_name")?,
            stream: serde_json::from_value(
                payload
                    .get("stream")
                    .cloned()
                    .unwrap_or_else(|| json!("stdout")),
            )
            .unwrap_or(norte_code_protocol::types::OutputStream::Stdout),
            text: required_string(payload, "delta")?,
            last: false,
        }),
        "TOOL_CALL_FINISHED" => {
            let status = required_string(payload, "status")?;
            event(norte_code_protocol::ServerEventKind::ToolFinished {
                turn,
                call_id: required_string(payload, "tool_call_id")?,
                tool: String::new(),
                is_error: status == "ERROR",
                error: None,
                output: payload
                    .get("result_summary")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                truncated: payload
                    .get("result_truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                detached: status == "CANCELLED",
                duration_ms: payload
                    .get("duration_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            })
        }
        "JOB_LIST_RESULT" => {
            let jobs = payload
                .get("jobs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|job| {
                    let status = job
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("RUNNING");
                    let state = match status {
                        "COMPLETED" => norte_code_protocol::JobState::Finished,
                        "FAILED" | "KILLED" | "TIMED_OUT" => {
                            norte_code_protocol::JobState::Cancelled
                        }
                        _ => norte_code_protocol::JobState::Running,
                    };
                    norte_code_protocol::JobInfo {
                        id: job
                            .get("job_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        label: job
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        script: String::new(),
                        state,
                        exit_code: job
                            .get("exit_code")
                            .and_then(Value::as_i64)
                            .and_then(|code| i32::try_from(code).ok()),
                        timed_out: status == "TIMED_OUT",
                        elapsed_ms: 0,
                        output: String::new(),
                        dropped: false,
                    }
                })
                .collect();
            event(norte_code_protocol::ServerEventKind::JobList { jobs })
        }
        "ERROR" => {
            let code = payload
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("internal")
                .to_ascii_lowercase();
            let code = serde_json::from_value(code.into())
                .unwrap_or(norte_code_protocol::ErrorCode::Internal);
            event(norte_code_protocol::ServerEventKind::Error {
                code,
                message: payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
        }
        _ => return Ok(None),
    };
    Ok(Some(kind))
}

#[allow(clippy::too_many_lines)]
fn server_event_from_frame(frame: &Value) -> Result<ServerEvent, ClientError> {
    if let Some(version) = frame.get("version").and_then(Value::as_str)
        && version != raep_types::WIRE_VERSION
    {
        return Err(ClientError::new(format!(
            "unsupported RAEP wire version `{version}`; this client speaks {}",
            raep_types::WIRE_VERSION
        )));
    }
    if let Some(event) = standard_event_from_frame(frame)? {
        return Ok(event);
    }
    if let Some(event) = frame
        .get("ext")
        .and_then(|ext| ext.get("norte_code"))
        .cloned()
    {
        return serde_json::from_value(event)
            .map_err(|error| ClientError::new(format!("invalid NorteCode event: {error}")));
    }
    let payload = frame.get("payload").cloned().ok_or_else(|| {
        ClientError::new("RAEP frame omitted both compatibility and canonical payloads")
    })?;
    let session = frame
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(norte_code_protocol::SessionKey::new);
    let kind = payload
        .get("event")
        .and_then(Value::as_str)
        .ok_or_else(|| ClientError::new("RAEP canonical payload omitted event"))?;
    let kind = match kind {
        "session_created" => norte_code_protocol::ServerEventKind::SessionCreated {
            title: payload
                .get("title")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| ClientError::new(format!("invalid session title: {error}")))?,
        },
        "session_list" => norte_code_protocol::ServerEventKind::SessionList {
            sessions: serde_json::from_value(
                payload
                    .get("sessions")
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new())),
            )
            .map_err(|error| ClientError::new(format!("invalid session list: {error}")))?,
        },
        "session_deleted" => norte_code_protocol::ServerEventKind::SessionDeleted,
        "session_forked" => norte_code_protocol::ServerEventKind::SessionForked {
            from: norte_code_protocol::SessionKey::new(required_string(&payload, "from")?),
        },
        "session_switched" => norte_code_protocol::ServerEventKind::SessionSwitched,
        "turn_started" => norte_code_protocol::ServerEventKind::TurnStarted {
            turn: number(&payload, "turn")?,
        },
        "token_delta" => norte_code_protocol::ServerEventKind::Delta {
            turn: number(&payload, "turn")?,
            delta: from_field(&payload, "delta")?,
        },
        "committed" => norte_code_protocol::ServerEventKind::Committed {
            turn: number(&payload, "turn")?,
            version: number(&payload, "version")?,
        },
        "history_message" => norte_code_protocol::ServerEventKind::MessageReplayed {
            index: number(&payload, "index")?,
            role: from_field(&payload, "role")?,
            kind: from_field(&payload, "kind")?,
            text: required_string(&payload, "text")?,
        },
        "session_todos" => norte_code_protocol::ServerEventKind::SessionTodos {
            todos: from_field(&payload, "todos")?,
        },
        "tool_call_started" => norte_code_protocol::ServerEventKind::ToolStarted {
            turn: number(&payload, "turn")?,
            call_id: required_string(&payload, "call_id")?,
            tool: required_string(&payload, "tool")?,
            arguments: optional_field(&payload, "arguments")?,
            source: optional_field(&payload, "source")?,
        },
        "job_progress_delta" => norte_code_protocol::ServerEventKind::ToolOutput {
            turn: number(&payload, "turn")?,
            call_id: required_string(&payload, "call_id")?,
            tool: required_string(&payload, "tool")?,
            stream: from_field(&payload, "stream")?,
            text: required_string(&payload, "text")?,
            last: payload
                .get("last")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "tool_call_finished" => norte_code_protocol::ServerEventKind::ToolFinished {
            turn: number(&payload, "turn")?,
            call_id: required_string(&payload, "call_id")?,
            tool: required_string(&payload, "tool")?,
            is_error: required_bool(&payload, "is_error")?,
            error: optional_field(&payload, "error")?,
            output: optional_field(&payload, "output")?,
            truncated: required_bool(&payload, "truncated")?,
            detached: required_bool(&payload, "detached")?,
            duration_ms: number(&payload, "duration_ms")?,
        },
        "output_part_finished" => norte_code_protocol::ServerEventKind::RequestReconstructed {
            turn: number(&payload, "turn")?,
            index: number(&payload, "index")?,
            usage: from_field(&payload, "usage")?,
            cost_usd: optional_field(&payload, "cost_usd")?,
            truncated: required_bool(&payload, "truncated")?,
            duration_ms: number(&payload, "duration_ms")?,
            first_token_ms: optional_field(&payload, "first_token_ms")?,
            tokens_per_second: optional_field(&payload, "tokens_per_second")?,
            cache_hit: required_bool(&payload, "cache_hit")?,
            tools: from_field(&payload, "tools")?,
        },
        "approval_resolved" => norte_code_protocol::ServerEventKind::QuestionAnswered {
            question: required_string(&payload, "approval_id")?,
        },
        "user_input_accepted" => norte_code_protocol::ServerEventKind::SteerApplied {
            ids: from_field(&payload, "ids")?,
            messages: from_field(&payload, "messages")?,
        },
        "provider_retry" => {
            let state = required_string(&payload, "state")?;
            if state == "scheduled" {
                norte_code_protocol::ServerEventKind::RetryScheduled {
                    turn: number(&payload, "turn")?,
                    attempt: u32_number(&payload, "attempt")?,
                    delay_ms: number(&payload, "delay_ms")?,
                    reason: required_string(&payload, "reason")?,
                }
            } else {
                norte_code_protocol::ServerEventKind::RetryStarted {
                    turn: number(&payload, "turn")?,
                    attempt: u32_number(&payload, "attempt")?,
                }
            }
        }
        "agent_changed" => match required_string(&payload, "change")?.as_str() {
            "model_changed" => norte_code_protocol::ServerEventKind::ModelChanged {
                provider: required_string(&payload, "provider")?,
                model: required_string(&payload, "model")?,
            },
            "model_queued" => norte_code_protocol::ServerEventKind::ModelQueued {
                provider: required_string(&payload, "provider")?,
                model: required_string(&payload, "model")?,
            },
            "reasoning_downgraded" => {
                let details = payload.get("details").cloned().unwrap_or(Value::Null);
                norte_code_protocol::ServerEventKind::ReasoningDowngraded {
                    from: from_field(&details, "from")?,
                    to: from_field(&details, "to")?,
                    reason: from_field(&details, "reason")?,
                }
            }
            other => {
                return Err(ClientError::new(format!(
                    "unsupported agent change `{other}`"
                )));
            }
        },
        "agent_catalog" => {
            let kind = required_string(&payload, "kind")?;
            let entries = payload
                .get("entries")
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            match kind.as_str() {
                "models" => norte_code_protocol::ServerEventKind::ModelList {
                    models: serde_json::from_value(entries).map_err(|error| {
                        ClientError::new(format!("invalid model list: {error}"))
                    })?,
                },
                "skills" => {
                    let skills = from_field(&entries, "skills")?;
                    let warnings = from_field(&entries, "warnings")?;
                    norte_code_protocol::ServerEventKind::SkillList { skills, warnings }
                }
                _ => {
                    return Err(ClientError::new(format!(
                        "unsupported agent catalog `{kind}`"
                    )));
                }
            }
        }
        "session_renamed" => norte_code_protocol::ServerEventKind::SessionRenamed {
            title: payload
                .get("title")
                .and_then(Value::as_str)
                .ok_or_else(|| ClientError::new("session_renamed omitted title"))?
                .to_owned(),
        },
        "session_stats" => norte_code_protocol::ServerEventKind::SessionStats {
            input_tokens: number(&payload, "input_tokens")?,
            output_tokens: number(&payload, "output_tokens")?,
            cache_read_tokens: number(&payload, "cache_read_tokens")?,
            cache_write_tokens: number(&payload, "cache_write_tokens")?,
            cost_usd: payload
                .get("cost_usd")
                .and_then(Value::as_f64)
                .ok_or_else(|| ClientError::new("session_stats omitted cost_usd"))?,
            requests: number(&payload, "requests")?,
            context_used: number(&payload, "context_used")?,
            context_window: payload.get("context_window").and_then(Value::as_u64),
        },
        "job_list" => norte_code_protocol::ServerEventKind::JobList {
            jobs: serde_json::from_value(
                payload
                    .get("jobs")
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new())),
            )
            .map_err(|error| ClientError::new(format!("invalid job list: {error}")))?,
        },
        "turn_state_changed" => {
            let state = required_string(&payload, "state")?;
            match state.as_str() {
                "finished" => norte_code_protocol::ServerEventKind::TurnFinished {
                    turn: number(&payload, "turn")?,
                    phase: from_field(&payload, "phase")?,
                    committed: required_bool(&payload, "committed")?,
                },
                "stopped" => norte_code_protocol::ServerEventKind::TurnStopped {
                    reason: required_string(&payload, "reason")?,
                },
                _ => {
                    return Err(ClientError::new(format!(
                        "unsupported turn state `{state}`"
                    )));
                }
            }
        }
        "context_update" => {
            let state = required_string(&payload, "state")?;
            match state.as_str() {
                "started" => norte_code_protocol::ServerEventKind::CompactionStarted {
                    reason: from_field(&payload, "reason")?,
                },
                "finished" => norte_code_protocol::ServerEventKind::CompactionFinished {
                    compacted: required_bool(&payload, "compacted")?,
                    north_session: optional_field(&payload, "north_session")?,
                    summary: optional_field(&payload, "summary")?,
                    error: optional_field(&payload, "error")?,
                },
                _ => {
                    return Err(ClientError::new(format!(
                        "unsupported context state `{state}`"
                    )));
                }
            }
        }
        "budget_warning" => norte_code_protocol::ServerEventKind::BudgetWarning {
            scope: from_field(&payload, "scope")?,
            unit: from_field(&payload, "unit")?,
            spent: required_number(&payload, "spent")?,
            limit: required_number(&payload, "limit")?,
        },
        "budget_exceeded" => norte_code_protocol::ServerEventKind::BudgetExceeded {
            scope: from_field(&payload, "scope")?,
            unit: from_field(&payload, "unit")?,
            spent: required_number(&payload, "spent")?,
            limit: required_number(&payload, "limit")?,
            action: from_field(&payload, "action")?,
        },
        "budget_changed" => norte_code_protocol::ServerEventKind::BudgetChanged {
            budget: optional_field(&payload, "budget")?,
        },
        "cost_updated" => norte_code_protocol::ServerEventKind::CostUpdated {
            total_usd: required_number(&payload, "total_usd")?,
            requests: number(&payload, "requests")?,
        },
        "cost_not_reported" => norte_code_protocol::ServerEventKind::CostNotReported {
            requests: number(&payload, "requests")?,
        },
        "plugin_list" => norte_code_protocol::ServerEventKind::PluginList {
            plugins: from_field(&payload, "plugins")?,
        },
        "plugin_changed" => norte_code_protocol::ServerEventKind::PluginChanged {
            operation: required_string(&payload, "operation")?,
            name: required_string(&payload, "name")?,
        },
        "service_data" => {
            let data = payload.get("data").cloned().unwrap_or(Value::Null);
            match payload.get("kind").and_then(Value::as_str) {
                Some("telemetry") => norte_code_protocol::ServerEventKind::Telemetry {
                    records: serde_json::from_value(data).map_err(|error| {
                        ClientError::new(format!("invalid telemetry payload: {error}"))
                    })?,
                },
                Some("tool_list") => norte_code_protocol::ServerEventKind::ToolList {
                    tools: serde_json::from_value(data).map_err(|error| {
                        ClientError::new(format!("invalid tool list payload: {error}"))
                    })?,
                },
                Some("mcp_list") => {
                    let servers = serde_json::from_value(
                        data.get("servers")
                            .cloned()
                            .unwrap_or(Value::Array(Vec::new())),
                    )
                    .map_err(|error| ClientError::new(format!("invalid MCP payload: {error}")))?;
                    let failures = serde_json::from_value(
                        data.get("failures")
                            .cloned()
                            .unwrap_or(Value::Array(Vec::new())),
                    )
                    .map_err(|error| {
                        ClientError::new(format!("invalid MCP failure payload: {error}"))
                    })?;
                    norte_code_protocol::ServerEventKind::McpList { servers, failures }
                }

                Some(other) => {
                    return Err(ClientError::new(format!(
                        "unsupported Norte Code service payload `{other}`"
                    )));
                }
                None => return Err(ClientError::new("service payload omitted kind")),
            }
        }
        "error" => norte_code_protocol::ServerEventKind::Error {
            code: serde_json::from_value(payload.get("code").cloned().unwrap_or(Value::Null))
                .map_err(|error| ClientError::new(format!("invalid error code: {error}")))?,
            message: payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("RAEP command failed")
                .to_owned(),
        },
        other => {
            return Err(ClientError::new(format!(
                "unsupported RAEP canonical event `{other}`"
            )));
        }
    };
    Ok(ServerEvent {
        request_id: 0,
        session,
        kind,
    })
}

fn number(payload: &Value, field: &str) -> Result<u64, ClientError> {
    payload
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| ClientError::new(format!("canonical payload omitted {field}")))
}

fn u32_number(payload: &Value, field: &str) -> Result<u32, ClientError> {
    u32::try_from(number(payload, field)?)
        .map_err(|_| ClientError::new(format!("canonical field {field} exceeds u32")))
}

fn required_string(payload: &Value, field: &str) -> Result<String, ClientError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ClientError::new(format!("canonical payload omitted {field}")))
}

fn required_bool(payload: &Value, field: &str) -> Result<bool, ClientError> {
    payload
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| ClientError::new(format!("canonical payload omitted {field}")))
}

fn required_number(payload: &Value, field: &str) -> Result<f64, ClientError> {
    payload
        .get(field)
        .and_then(Value::as_f64)
        .ok_or_else(|| ClientError::new(format!("canonical payload omitted {field}")))
}

fn from_field<T: serde::de::DeserializeOwned>(
    payload: &Value,
    field: &str,
) -> Result<T, ClientError> {
    serde_json::from_value(
        payload
            .get(field)
            .cloned()
            .ok_or_else(|| ClientError::new(format!("canonical payload omitted {field}")))?,
    )
    .map_err(|error| ClientError::new(format!("invalid canonical field {field}: {error}")))
}

fn optional_field<T: serde::de::DeserializeOwned>(
    payload: &Value,
    field: &str,
) -> Result<Option<T>, ClientError> {
    payload
        .get(field)
        .cloned()
        .map(|value| {
            serde_json::from_value(value).map_err(|error| {
                ClientError::new(format!("invalid canonical field {field}: {error}"))
            })
        })
        .transpose()
}

fn response_error(body: &Value) -> String {
    body.get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("RAEP command failed")
        .to_owned()
}

fn session_from_request(request: &ClientRequest) -> Option<String> {
    use norte_code_protocol::ClientRequestKind;
    let session = match &request.kind {
        ClientRequestKind::SwitchSession { session }
        | ClientRequestKind::ForkSession { session, .. }
        | ClientRequestKind::RenameSession { session, .. }
        | ClientRequestKind::DeleteSession { session }
        | ClientRequestKind::Steer { session, .. }
        | ClientRequestKind::CancelTurn { session }
        | ClientRequestKind::StopJobs { session, .. }
        | ClientRequestKind::DetachTurn { session }
        | ClientRequestKind::ListJobs { session }
        | ClientRequestKind::Resume { session, .. }
        | ClientRequestKind::AnswerQuestion { session, .. }
        | ClientRequestKind::SetModel { session, .. }
        | ClientRequestKind::SetReasoning { session, .. }
        | ClientRequestKind::SetBudget { session, .. }
        | ClientRequestKind::Compact { session }
        | ClientRequestKind::ListTools {
            session: Some(session),
        }
        | ClientRequestKind::SetApprovalRule {
            session: Some(session),
            ..
        }
        | ClientRequestKind::ForgetApprovalRule {
            session: Some(session),
            ..
        } => Some(session),
        ClientRequestKind::SendMessage { session, .. }
        | ClientRequestKind::CostReport { session }
        | ClientRequestKind::ListTelemetry { session } => session.as_ref(),
        ClientRequestKind::ListMcp
        | ClientRequestKind::RefreshMcp
        | ClientRequestKind::SetMcpActive { .. }
        | ClientRequestKind::CreateSession { .. }
        | ClientRequestKind::ListSessions
        | ClientRequestKind::ListModels
        | ClientRequestKind::ListTools { session: None }
        | ClientRequestKind::SetApprovalRule { session: None, .. }
        | ClientRequestKind::ForgetApprovalRule { session: None, .. }
        | ClientRequestKind::RefreshCatalog
        | ClientRequestKind::ListSkills
        | ClientRequestKind::RefreshSkills
        | ClientRequestKind::ListPlugins
        | ClientRequestKind::InstallPlugin { .. }
        | ClientRequestKind::UpdatePlugins { .. }
        | ClientRequestKind::RemovePlugin { .. }
        | ClientRequestKind::SetPluginEnabled { .. } => None,
    }?;
    Some(session.as_str().to_owned())
}

fn timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte_code_protocol::{ClientRequestKind, SessionKey};

    #[test]
    fn prefers_the_standard_payload_over_the_compatibility_extension() {
        let frame = serde_json::json!({
            "version": "1.0",
            "type": "TOKEN_DELTA",
            "session_id": "session-1",
            "turn_id": "7",
            "payload": {"delta": "canonical", "index": 2, "channel": "text"},
            "ext": {"norte_code": {
                "request_id": 0,
                "session": "session-1",
                "kind": {"event": "committed", "turn": 7, "version": 1}
            }}
        });
        let event = server_event_from_frame(&frame).expect("standard token delta");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::Delta {
                turn: 7,
                delta: norte_code_protocol::Delta::Text { index: 2, ref text }
            } if text == "canonical"
        ));
    }

    #[test]
    fn rejects_an_unsupported_wire_version() {
        let frame = serde_json::json!({
            "version": "2.0",
            "type": "TOKEN_DELTA",
            "payload": {"delta": "hi", "index": 0}
        });
        let error = server_event_from_frame(&frame).expect_err("unsupported version");
        assert!(error.to_string().contains("wire version"), "{error}");
    }

    #[test]
    fn decodes_a_standard_tool_start() {
        let frame = serde_json::json!({
            "version": "1.0",
            "type": "TOOL_CALL_STARTED",
            "session_id": "session-1",
            "turn_id": "3",
            "payload": {
                "tool_call_id": "call-1",
                "tool_name": "shell",
                "arguments": {"command": "ls"},
                "requires_approval": false
            }
        });
        let event = server_event_from_frame(&frame).expect("standard tool start");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::ToolStarted { turn: 3, ref call_id, ref tool, .. }
                if call_id == "call-1" && tool == "shell"
        ));
    }

    #[test]
    fn classifies_sequence_steps() {
        assert_eq!(seq_step(None, 1), SeqStep::Apply);
        assert_eq!(seq_step(Some(4), 5), SeqStep::Apply);
        assert_eq!(seq_step(Some(4), 4), SeqStep::Duplicate);
        assert_eq!(seq_step(Some(4), 3), SeqStep::Duplicate);
        assert_eq!(seq_step(Some(4), 6), SeqStep::Gap { expected: 5 });
        assert_eq!(seq_step(Some(u64::MAX), u64::MAX), SeqStep::Duplicate);
    }

    #[test]
    fn encodes_session_path_segments_without_reinterpreting_slashes() {
        assert_eq!(
            percent_encode("provider/session 1"),
            "provider%2Fsession%201"
        );
    }

    #[test]
    fn extracts_session_from_session_scoped_requests() {
        let request = ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::CancelTurn {
                session: SessionKey::new("session-1"),
            },
        };
        assert_eq!(session_from_request(&request).as_deref(), Some("session-1"));
    }

    #[test]
    fn leaves_connection_scoped_requests_without_a_session() {
        let request = ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::ListSessions,
        };
        assert_eq!(session_from_request(&request), None);
    }

    #[test]
    fn prefixes_bearer_tokens_without_redacting_the_value() {
        let client = RemoteClient::new("http://localhost:8080", Some("secret-token"), None)
            .expect("valid bearer token");
        assert_eq!(
            client
                .bearer_token
                .as_ref()
                .and_then(|value| value.to_str().ok()),
            Some("Bearer secret-token")
        );
    }

    #[test]
    fn decodes_canonical_service_data_without_the_compatibility_extension() {
        let frame = serde_json::json!({
            "session_id": "session-1",
            "payload": {
                "event": "service_data",
                "kind": "tool_list",
                "data": [{
                    "name": "read",
                    "allowed": true,
                    "scope": "default"
                }]
            }
        });
        let event = server_event_from_frame(&frame).expect("canonical service event");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::ToolList { ref tools }
                if tools[0].name == "read"
        ));
    }

    #[test]
    fn decodes_canonical_session_stats_and_jobs() {
        let stats = serde_json::json!({
            "session_id": "session-1",
            "payload": {
                "event": "session_stats",
                "input_tokens": 10,
                "output_tokens": 4,
                "cache_read_tokens": 2,
                "cache_write_tokens": 1,
                "cost_usd": 0.25,
                "requests": 2,
                "context_used": 14,
                "context_window": 100
            }
        });
        let event = server_event_from_frame(&stats).expect("canonical stats");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::SessionStats {
                context_used: 14,
                context_window: Some(100),
                ..
            }
        ));

        let jobs = serde_json::json!({
            "session_id": "session-1",
            "payload": {
                "event": "job_list",
                "jobs": []
            }
        });
        let event = server_event_from_frame(&jobs).expect("canonical jobs");
        assert!(matches!(
            event.kind,
            norte_code_protocol::ServerEventKind::JobList { ref jobs } if jobs.is_empty()
        ));
    }

    #[test]
    fn serializes_remote_service_requests_with_protocol_tags() {
        let request = ClientRequest {
            request_id: 7,
            kind: ClientRequestKind::ListTelemetry { session: None },
        };
        let value = serde_json::to_value(request).expect("request serializes");
        assert_eq!(value["kind"]["type"], "list_telemetry");
    }

    #[test]
    fn tracks_last_sequence_per_session_and_keeps_the_maximum() {
        let client =
            RemoteClient::new("http://localhost:8080", None, None).expect("client creates");
        assert_eq!(client.last_seq("session-1"), None);
        client.note_seq("session-1", 7);
        assert_eq!(client.last_seq("session-1"), Some(7));
        client.note_seq("session-1", 4);
        assert_eq!(
            client.last_seq("session-1"),
            Some(7),
            "an older frame must not move the cursor backwards"
        );
        client.note_seq("session-1", 9);
        assert_eq!(client.last_seq("session-1"), Some(9));
        client.note_seq("session-2", 3);
        assert_eq!(client.last_seq("session-2"), Some(3));
        assert_eq!(client.last_seq("session-1"), Some(9));
    }

    #[test]
    fn records_sequence_from_raw_frames_only() {
        let client =
            RemoteClient::new("http://localhost:8080", None, None).expect("client creates");
        client.note_frame_seq(&serde_json::json!({
            "session_id": "session-1",
            "seq": 12,
        }));
        assert_eq!(client.last_seq("session-1"), Some(12));
        client.note_frame_seq(&serde_json::json!({
            "session_id": "session-1",
            "payload": {},
        }));
        assert_eq!(client.last_seq("session-1"), Some(12));
        client.note_frame_seq(&serde_json::json!({
            "session_id": "",
            "seq": 99,
        }));
        assert_eq!(client.last_seq(""), None);
    }

    #[test]
    fn drains_sse_blocks_in_order_and_reaches_past_the_terminal() {
        let mut buffer = concat!(
            "event: turn_state_changed\n",
            "data: {\"session_id\":\"session-1\",\"seq\":1,\"payload\":{\"event\":\"turn_state_changed\",\"state\":\"finished\",\"turn\":1,\"phase\":\"idle\",\"committed\":true}}\n",
            "\n",
            "event: SESSION_STATS\n",
            "data: {\"session_id\":\"session-1\",\"seq\":2,\"payload\":{\"event\":\"session_stats\",\"input_tokens\":10,\"output_tokens\":4,\"cache_read_tokens\":2,\"cache_write_tokens\":1,\"cost_usd\":0.25,\"requests\":2,\"context_used\":14,\"context_window\":100}}\n",
            "\n",
            "data: {\"session_id\":\"session-1\",\"seq\":3,\"payload\":{\"event\":\"session_stats\",\"input_tokens\":11,\"output_tokens\":5,\"cache_read_tokens\":2,\"cache_write_tokens\":1,\"cost_usd\":0.26,\"requests\":3,\"context_used\":15,\"context_window\":100}}"
        )
        .to_owned()
        .into_bytes();
        let drained = drain_sse_buffer(&mut buffer);
        assert_eq!(drained.len(), 2);
        assert!(drained[0].1, "the terminal event is flagged");
        assert_eq!(drained[0].2, Some(1));
        assert!(
            !drained[1].1,
            "the event after the terminal is not terminal but still drains"
        );
        assert_eq!(drained[1].2, Some(2));
        // The partial trailing block is left for the next chunk.
        assert!(String::from_utf8_lossy(&buffer).contains("\"seq\":3"));
    }

    #[test]
    fn keeps_a_multibyte_character_split_across_chunks() {
        let text = "data: {\"session_id\":\"s\",\"payload\":{\"event\":\"session_created\",\"title\":\"café\"}}\n\n";
        let bytes = text.as_bytes();
        let split = text.find('é').expect("accented character") + 1;
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&bytes[..split]);
        assert!(
            drain_sse_buffer(&mut buffer).is_empty(),
            "a partial block must wait for the rest"
        );
        buffer.extend_from_slice(&bytes[split..]);
        let drained = drain_sse_buffer(&mut buffer);
        assert_eq!(drained.len(), 1);
        assert!(matches!(
            drained[0].0.kind,
            norte_code_protocol::ServerEventKind::SessionCreated { title: Some(ref title) }
                if title == "café"
        ));
    }
}
