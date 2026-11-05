//! Runtime-independent RAEP host adapter.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use norte_code_agent::Service;
use norte_code_protocol::{
    ClientRequest, ClientRequestKind, ServerEvent, ServerEventKind, SessionKey,
};
use raep_types::{
    AttachMode, Envelope, FrameId, FrameType, SessionAttach, StandardFrameType, UserInput,
    WIRE_VERSION,
};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::{Mutex, broadcast};

/// Host configuration independent of the HTTP listener.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Agent identifier accepted during attachment.
    pub agent_id: String,
    /// Advertised server version.
    pub server_version: String,
    /// Maximum number of emitted frames retained per session.
    pub replay_capacity: usize,
    /// Maximum frame size advertised to clients.
    pub max_frame_bytes: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            agent_id: "norte-code".to_owned(),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            replay_capacity: 10_000,
            max_frame_bytes: 1024 * 1024,
        }
    }
}

/// Errors raised while translating or executing a RAEP frame.
#[derive(Debug, Error)]
pub enum HostError {
    /// The frame is invalid or cannot be used in the current connection state.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    /// The Norte Code service rejected a request before streaming it.
    #[error("service request failed: {0}")]
    Service(String),
    /// Replay is unavailable for the requested sequence.
    #[error("replay unavailable: {0}")]
    Replay(String),
    /// A frame could not be serialized.
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Default)]
struct HostState {
    attached: HashSet<String>,
    sequences: HashMap<String, u64>,
    replay: HashMap<String, VecDeque<Envelope<Value>>>,
    idempotency: HashMap<FrameId, Vec<Envelope<Value>>>,
}

/// A Norte Code service exposed through the RAEP wire.
pub struct NorteCodeRaepHost {
    service: Arc<Service>,
    config: HostConfig,
    state: Mutex<HostState>,
    events: broadcast::Sender<Envelope<Value>>,
}

impl NorteCodeRaepHost {
    /// Creates a host around the server-owned Norte Code service.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured agent identifier is empty or the
    /// replay capacity is zero.
    pub fn new(service: Arc<Service>, config: HostConfig) -> Result<Self, HostError> {
        if config.agent_id.is_empty() {
            return Err(HostError::InvalidFrame(
                "agent_id must not be empty".to_owned(),
            ));
        }
        if config.replay_capacity == 0 {
            return Err(HostError::InvalidFrame(
                "replay_capacity must be positive".to_owned(),
            ));
        }
        let (events, _) = broadcast::channel(config.replay_capacity.max(16));
        Ok(Self {
            service,
            config,
            state: Mutex::new(HostState::default()),
            events,
        })
    }

    /// Returns the server-owned agent service.
    #[must_use]
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }

    /// Returns the configured agent identifier.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.config.agent_id
    }

    /// Returns a receiver for live frames emitted by any attached connection.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope<Value>> {
        self.events.subscribe()
    }

    /// Returns the RAEP handshake frame.
    ///
    /// The handshake payload uses only JSON values constructed from validated
    /// host configuration, so serialization cannot fail.
    #[must_use]
    pub fn hello(&self) -> Envelope<Value> {
        Envelope::new(
            standard(StandardFrameType::Hello),
            json!({
                "server_version": self.config.server_version,
                "wire_versions": [WIRE_VERSION],
                "capabilities": [
                    "transport.ws",
                    "transport.sse",
                    "agent.norte_code",
                    "events.replay"
                ],
                "limits": {
                    "max_frame_bytes": self.config.max_frame_bytes,
                    "replay_buffer_events": self.config.replay_capacity,
                },
                "heartbeat_s": 20,
                "draining": false,
                "deprecations": []
            }),
            now_millis(),
        )
    }

    /// Returns discovery information for an HTTP info route.
    #[must_use]
    pub fn info(&self) -> Value {
        json!({
            "server_version": self.config.server_version,
            "wire_versions": [WIRE_VERSION],
            "capabilities": [
                "transport.ws",
                "transport.sse",
                "agent.norte_code",
                "events.replay"
            ],
            "limits": {
                "max_frame_bytes": self.config.max_frame_bytes,
                "replay_buffer_events": self.config.replay_capacity
            }
        })
    }

    /// Handles one decoded RAEP frame.
    ///
    /// Service events are published as soon as they arrive, while the
    /// returned vector provides the same frames to a request/response
    /// transport. This keeps transports independent from the agent loop.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame is malformed, unsupported, unauthenticated
    /// for the current attachment, or rejected by the Norte Code service.
    ///
    pub async fn handle(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        if frame.version != WIRE_VERSION {
            return Err(HostError::InvalidFrame(
                "unsupported RAEP wire version".to_owned(),
            ));
        }
        {
            let state = self.state.lock().await;
            if let Some(previous) = state.idempotency.get(&frame.id) {
                return Ok(previous.clone());
            }
        }
        let frame_id = frame.id.clone();
        let result = match frame.kind {
            FrameType::Standard(StandardFrameType::SessionAttach) => self.attach(frame).await,
            FrameType::Standard(StandardFrameType::UserInput) => self.user_input(frame).await,
            FrameType::Standard(StandardFrameType::CancelTurn) => self.cancel(frame).await,
            FrameType::Standard(StandardFrameType::SessionDetach) => self.detach(frame).await,
            FrameType::Standard(StandardFrameType::SessionClose) => self.close(frame).await,
            FrameType::Standard(StandardFrameType::Ack) => self.ack(frame).await,
            FrameType::Extension(ref name) if name == "norte_code.command" => {
                self.extension_command(frame).await
            }
            _ => Err(HostError::InvalidFrame("unsupported command".to_owned())),
        }?;
        self.state
            .lock()
            .await
            .idempotency
            .insert(frame_id, result.clone());
        Ok(result)
    }

    /// Returns replay frames strictly after `last_seq`.
    ///
    /// # Errors
    ///
    /// Returns an error when the session has no retained replay window or the
    /// requested sequence predates the retained window.
    pub async fn replay_after(
        &self,
        session_id: &str,
        last_seq: u64,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let state = self.state.lock().await;
        let frames = state
            .replay
            .get(session_id)
            .ok_or_else(|| HostError::Replay("session replay window is missing".to_owned()))?;
        if let Some(first) = frames.front().and_then(|frame| frame.seq)
            && last_seq.saturating_add(1) < first
        {
            return Err(HostError::Replay(
                "requested sequence is older than the retained window".to_owned(),
            ));
        }
        Ok(frames
            .iter()
            .filter(|frame| frame.seq.is_some_and(|seq| seq > last_seq))
            .cloned()
            .collect())
    }

    async fn attach(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        let attach: SessionAttach = serde_json::from_value(frame.payload)?;
        if attach.agent_id != self.config.agent_id {
            return Err(HostError::InvalidFrame(
                "agent_id does not match this host".to_owned(),
            ));
        }
        let (session_id, created) = if frame.session_id.is_empty() {
            let key = self
                .service
                .start_session(None)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?;
            (key.to_string(), true)
        } else {
            (frame.session_id.clone(), false)
        };
        let exists = created
            || self
                .service
                .session_title(&SessionKey::new(session_id.clone()))
                .is_some()
            || self
                .service
                .session_spec(&SessionKey::new(session_id.clone()))
                .is_some();
        if !exists && attach.create_if_missing == Some(false) {
            return Err(HostError::InvalidFrame("session not found".to_owned()));
        }
        {
            let mut state = self.state.lock().await;
            state.attached.insert(session_id.clone());
            state.sequences.entry(session_id.clone()).or_insert(0);
            state.replay.entry(session_id.clone()).or_default();
        }
        let mode = attach.mode.unwrap_or(AttachMode::Controller);
        let attached = self
            .emit(
                &session_id,
                StandardFrameType::SessionAttached,
                json!({
                    "created": created,
                    "mode": mode,
                    "negotiated_capabilities": [
                        "agent.norte_code",
                        "events.replay"
                    ]
                }),
            )
            .await?;
        let state = self
            .emit(
                &session_id,
                StandardFrameType::SessionState,
                json!({"status": "ACTIVE"}),
            )
            .await?;
        let mut output = vec![attached, state];
        if let Some(after) = attach.resume_from_seq {
            output.extend(self.replay_after(&session_id, after).await?);
        }
        if exists && attach.resume_from_seq.is_none() {
            let replay = self
                .submit_service(
                    &session_id,
                    ClientRequestKind::Resume {
                        session: SessionKey::new(session_id.clone()),
                    },
                )
                .await?;
            output.extend(replay);
        }
        Ok(output)
    }

    async fn user_input(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let input: UserInput = serde_json::from_value(frame.payload)?;
        input
            .validate()
            .map_err(|error| HostError::InvalidFrame(error.to_string()))?;
        let text = match (input.prompt, input.content) {
            (Some(text), None) => text,
            (None, Some(parts)) => {
                let mut text = String::new();
                for part in parts {
                    if part.kind != raep_types::Modality::Text {
                        return Err(HostError::InvalidFrame(
                            "non-text RAEP content requires an artifact adapter".to_owned(),
                        ));
                    }
                    text.push_str(part.text.as_deref().ok_or_else(|| {
                        HostError::InvalidFrame("text content part is missing text".to_owned())
                    })?);
                }
                text
            }
            _ => unreachable!("UserInput::validate enforces exactly one form"),
        };
        let mut output = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::SendMessage {
                    session: Some(SessionKey::new(frame.session_id.clone())),
                    text,
                    attachments: Vec::new(),
                    budget: None,
                    reasoning: None,
                    replay: false,
                },
            )
            .await?;
        if output.is_empty() {
            output.push(
                self.emit(
                    &frame.session_id,
                    StandardFrameType::UserInputAccepted,
                    json!({"accepted": true}),
                )
                .await?,
            );
        }
        Ok(output)
    }

    async fn cancel(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request = ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::CancelTurn {
                session: SessionKey::new(frame.session_id.clone()),
            },
        };
        let events = self
            .service
            .submit(request)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.collect_service_events(&frame.session_id, events).await
    }

    async fn detach(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        let mut state = self.state.lock().await;
        state.attached.remove(&frame.session_id);
        Ok(Vec::new())
    }

    async fn close(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let events = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::DeleteSession {
                    session: SessionKey::new(frame.session_id.clone()),
                },
            )
            .await?;
        self.state.lock().await.attached.remove(&frame.session_id);
        Ok(events)
    }

    async fn ack(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let last_seq = frame
            .payload
            .get("last_seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| HostError::InvalidFrame("ACK.last_seq is required".to_owned()))?;
        let mut state = self.state.lock().await;
        if let Some(frames) = state.replay.get_mut(&frame.session_id) {
            frames.retain(|stored| stored.seq.is_none_or(|seq| seq > last_seq));
        }
        Ok(Vec::new())
    }

    async fn extension_command(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let request: ClientRequest = serde_json::from_value(frame.payload)?;
        let session_id = session_for_request(&request.kind)
            .map(std::string::ToString::to_string)
            .or_else(|| (!frame.session_id.is_empty()).then_some(frame.session_id.clone()))
            .unwrap_or_default();
        if !session_id.is_empty() {
            self.require_attached(&session_id).await?;
        }
        self.collect_service_events(
            &session_id,
            self.service
                .submit(request)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
        )
        .await
    }

    async fn submit_service(
        &self,
        session_id: &str,
        kind: ClientRequestKind,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind,
            })
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.collect_service_events(session_id, stream).await
    }

    async fn collect_service_events(
        &self,
        session_id: &str,
        mut stream: norte_code_agent::EventStream,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        use futures_util::StreamExt;
        let mut output = Vec::new();
        while let Some(event) = stream.next().await {
            output.push(self.emit_event(session_id, event).await?);
        }
        Ok(output)
    }

    async fn emit_event(
        &self,
        session_id: &str,
        event: ServerEvent,
    ) -> Result<Envelope<Value>, HostError> {
        let kind = frame_type_for_event(&event.kind);
        let session_id = event
            .session
            .as_ref()
            .map_or_else(|| session_id.to_owned(), |key| key.as_str().to_owned());
        let payload = serde_json::to_value(event)?;
        self.emit(&session_id, kind, payload).await
    }

    async fn emit(
        &self,
        session_id: &str,
        kind: StandardFrameType,
        payload: Value,
    ) -> Result<Envelope<Value>, HostError> {
        let mut frame = Envelope::new(standard(kind), payload, now_millis());
        frame.session_id = session_id.to_owned();
        let mut state = self.state.lock().await;
        let sequence = state.sequences.entry(session_id.to_owned()).or_insert(0);
        *sequence = sequence.saturating_add(1);
        frame.seq = Some(*sequence);
        let replay = state.replay.entry(session_id.to_owned()).or_default();
        replay.push_back(frame.clone());
        while replay.len() > self.config.replay_capacity {
            replay.pop_front();
        }
        drop(state);
        let _ = self.events.send(frame.clone());
        Ok(frame)
    }

    async fn require_attached(&self, session_id: &str) -> Result<(), HostError> {
        if self.state.lock().await.attached.contains(session_id) {
            Ok(())
        } else {
            Err(HostError::InvalidFrame(
                "session is not attached".to_owned(),
            ))
        }
    }
}

fn frame_type_for_event(kind: &ServerEventKind) -> StandardFrameType {
    match kind {
        ServerEventKind::Delta { .. } => StandardFrameType::TokenDelta,
        ServerEventKind::ToolStarted { .. } => StandardFrameType::ToolCallStarted,
        ServerEventKind::ToolOutput { .. } => StandardFrameType::JobProgressDelta,
        ServerEventKind::ToolFinished { .. } => StandardFrameType::ToolCallFinished,
        ServerEventKind::QuestionAsked { .. } => StandardFrameType::ToolApprovalResponse,
        ServerEventKind::BudgetWarning { .. } => StandardFrameType::BudgetWarning,
        ServerEventKind::CompactionStarted { .. } => StandardFrameType::ContextUpdated,
        ServerEventKind::CompactionFinished { .. } => StandardFrameType::HistoryCompacted,
        ServerEventKind::SessionCreated { .. }
        | ServerEventKind::SessionList { .. }
        | ServerEventKind::SessionDeleted
        | ServerEventKind::SessionRenamed { .. }
        | ServerEventKind::SessionForked { .. }
        | ServerEventKind::SessionSwitched
        | ServerEventKind::SessionStats { .. }
        | ServerEventKind::Committed { .. }
        | ServerEventKind::SessionTodos { .. } => StandardFrameType::SessionUpdated,
        ServerEventKind::TurnStarted { .. }
        | ServerEventKind::TurnFinished { .. }
        | ServerEventKind::TurnStopped { .. } => StandardFrameType::TurnStateChanged,
        ServerEventKind::Error { .. } => StandardFrameType::Error,
        ServerEventKind::JobList { .. } | ServerEventKind::JobsStopped { .. } => {
            StandardFrameType::JobProgressDelta
        }
        ServerEventKind::SteerApplied { .. } => StandardFrameType::UserInputAccepted,
        ServerEventKind::ModelChanged { .. }
        | ServerEventKind::ModelQueued { .. }
        | ServerEventKind::ReasoningDowngraded { .. }
        | ServerEventKind::ModelList { .. }
        | ServerEventKind::CatalogUpdated { .. } => StandardFrameType::AgentChanged,
        ServerEventKind::SkillList { .. } | ServerEventKind::SkillsUpdated { .. } => {
            StandardFrameType::AgentChanged
        }
        ServerEventKind::QuestionAnswered { .. } => StandardFrameType::ApprovalResolved,
        ServerEventKind::RetryScheduled { .. } | ServerEventKind::RetryStarted { .. } => {
            StandardFrameType::ProviderRetry
        }
        ServerEventKind::RequestReconstructed { .. }
        | ServerEventKind::CostUpdated { .. }
        | ServerEventKind::CostNotReported { .. } => StandardFrameType::OutputPartFinished,
        ServerEventKind::MessageReplayed { .. } => StandardFrameType::HistoryGet,
        ServerEventKind::BudgetExceeded { .. } | ServerEventKind::BudgetChanged { .. } => {
            StandardFrameType::BudgetWarning
        }
    }
}

fn session_for_request(kind: &ClientRequestKind) -> Option<&SessionKey> {
    match kind {
        ClientRequestKind::SwitchSession { session }
        | ClientRequestKind::RenameSession { session, .. }
        | ClientRequestKind::DeleteSession { session }
        | ClientRequestKind::Steer { session, .. }
        | ClientRequestKind::CancelTurn { session }
        | ClientRequestKind::StopJobs { session, .. }
        | ClientRequestKind::DetachTurn { session }
        | ClientRequestKind::ListJobs { session }
        | ClientRequestKind::Resume { session }
        | ClientRequestKind::AnswerQuestion { session, .. }
        | ClientRequestKind::SetModel { session, .. }
        | ClientRequestKind::SetReasoning { session, .. }
        | ClientRequestKind::SetBudget { session, .. }
        | ClientRequestKind::Compact { session }
        | ClientRequestKind::ForkSession { session, .. } => Some(session),
        ClientRequestKind::SendMessage { session, .. }
        | ClientRequestKind::CostReport { session } => session.as_ref(),
        ClientRequestKind::CreateSession { .. }
        | ClientRequestKind::ListSessions
        | ClientRequestKind::ListModels
        | ClientRequestKind::RefreshCatalog
        | ClientRequestKind::ListSkills
        | ClientRequestKind::RefreshSkills => None,
    }
}

fn standard(kind: StandardFrameType) -> FrameType {
    FrameType::Standard(kind)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
