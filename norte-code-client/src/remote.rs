use async_trait::async_trait;
use futures_util::stream::{self, StreamExt};
use norte_code_wire::QueueMode;
use norte_code_wire::{ClientRequest, ServerEvent};
use raep_types::{
    Ack, ClientInfo, Envelope, FrameType, SessionAttach, StandardFrameType, UserInput,
};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tokio_stream::iter;

#[allow(clippy::wildcard_imports)]
use crate::*;

/// HTTP client for the `NorteCode` RAEP command endpoint.
///
/// The transport returns canonical RAEP envelopes, while the compatibility
/// extension carries the original `NorteCode` event until all consumers migrate.
pub struct RemoteClient {
    http: reqwest::Client,
    base_url: String,
    pub(crate) bearer_token: Option<HeaderValue>,
    agent_id: String,
    default_session: Option<String>,
    attached_sessions: Mutex<HashSet<String>>,
    session_seq: Arc<Mutex<HashMap<String, u64>>>,
    session_info: Mutex<HashMap<String, norte_code_wire::SessionInfo>>,
    session_reasoning: Mutex<HashMap<String, norte_code_wire::Reasoning>>,
    session_stats: Mutex<HashMap<String, RemoteSessionStats>>,
    session_jobs: Mutex<HashMap<String, Vec<norte_code_wire::JobInfo>>>,
    /// What this client can render and which optional features it negotiates
    /// on `SessionAttach`.
    capabilities: raep_types::ClientCapabilities,
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
            capabilities: Self::default_client_capabilities(),
        })
    }

    /// Selects the agent identifier used for RAEP attachments.
    #[must_use]
    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = agent_id.into();
        self
    }

    /// Converts a Norte Code [`norte_code_wire::Reasoning`] into the
    /// `reasoning` fragment of a RAEP `agent_override` object (RFC 7).
    fn reasoning_override(reasoning: &norte_code_wire::Reasoning) -> serde_json::Value {
        match reasoning {
            norte_code_wire::Reasoning::Automatic => serde_json::json!({
                "reasoning": { "mode": "auto" }
            }),
            norte_code_wire::Reasoning::Disabled => serde_json::json!({
                "reasoning": { "mode": "off" }
            }),
            norte_code_wire::Reasoning::Effort { effort } => serde_json::json!({
                "reasoning": { "mode": "on", "effort": effort }
            }),
            norte_code_wire::Reasoning::Budget { tokens } => serde_json::json!({
                "reasoning": { "mode": "on", "budget_tokens": tokens }
            }),
        }
    }

    /// Converts a Norte Code [`norte_code_wire::Budget`] into the
    /// `budgets` fragment of a RAEP `agent_override` object (RFC 7).
    fn budget_override(budget: &norte_code_wire::Budget) -> serde_json::Value {
        let on_exceed = match budget.action {
            norte_code_wire::BudgetAction::Ask => "ask",
            norte_code_wire::BudgetAction::Warn => "degrade",
            norte_code_wire::BudgetAction::Stop => "stop",
        };
        match budget.unit {
            norte_code_wire::BudgetUnit::Usd => serde_json::json!({
                "budgets": { "max_session_cost": budget.limit, "currency": "USD", "on_exceed": on_exceed }
            }),
            norte_code_wire::BudgetUnit::Tokens | norte_code_wire::BudgetUnit::Requests => {
                serde_json::json!({
                    "budgets": { "max_session_tokens": budget.limit, "on_exceed": on_exceed }
                })
            }
        }
    }

    /// Fetches the public RAEP discovery information.
    ///
    /// # Errors
    ///
    /// Returns an error when the service cannot be reached or returns a
    /// non-success response.
    /// Selects the [`raep_types::ClientCapabilities`] sent on the next
    /// `SessionAttach` instead of the default negotiated set.
    ///
    /// The default negotiates every render modality the wire knows and
    /// `output.parts`, so a client that can render the outputs gets them
    /// without naming them. A text-only caller narrows `render` to `[Text]`
    /// and drops `features`; the host then only delivers text.
    #[must_use]
    pub fn with_capabilities(mut self, capabilities: raep_types::ClientCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// The capabilities negotiated when a caller does not narrow them.
    fn default_client_capabilities() -> raep_types::ClientCapabilities {
        raep_types::ClientCapabilities {
            render: vec![
                raep_types::Modality::Text,
                raep_types::Modality::Image,
                raep_types::Modality::Document,
                raep_types::Modality::Audio,
                raep_types::Modality::Video,
            ],
            features: vec![raep_types::ClientFeature::OutputParts],
            client_tools: Vec::new(),
        }
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
    pub fn session_info(&self, session: &str) -> Option<norte_code_wire::SessionInfo> {
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
    pub fn session_jobs(&self, session: &str) -> Vec<norte_code_wire::JobInfo> {
        self.session_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the last session-scoped reasoning setting sent to the host.
    #[must_use]
    pub fn session_reasoning(&self, session: &str) -> Option<norte_code_wire::Reasoning> {
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
    pub(crate) fn note_seq(&self, session: &str, seq: u64) {
        let mut seqs = self
            .session_seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        seqs.entry(session.to_owned())
            .and_modify(|current| *current = (*current).max(seq))
            .or_insert(seq);
    }

    /// Records the sequence of one raw RAEP frame, if it carries one.
    pub(crate) fn note_frame_seq(&self, frame: &Envelope<Value>) {
        let Some(seq) = frame.seq else {
            return;
        };
        if !frame.session_id.is_empty() {
            self.note_seq(&frame.session_id, seq);
        }
    }

    pub(crate) fn session_for(&self, request: &ClientRequest) -> String {
        session_from_request(request)
            .or_else(|| self.default_session.clone())
            .unwrap_or_else(|| "default".to_owned())
    }

    pub(crate) fn command_url(&self, session_id: &str) -> String {
        format!(
            "{}/raep/v1/sessions/{}/commands",
            self.base_url,
            percent_encode(session_id)
        )
    }

    /// Posts a standard frame to the command endpoint and returns the
    /// response's frames. The payload is already a typed value; this is
    /// the one place the standard-command POST is built.
    pub(crate) async fn post_standard(
        &self,
        session: &str,
        kind: FrameType,
        payload: Value,
    ) -> Result<Vec<Envelope<Value>>, ClientError> {
        self.ensure_attached(session).await?;
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let mut frame = Envelope::new(kind, payload, timestamp_millis());
        frame.session_id = session.to_owned();
        let response = self
            .http
            .post(self.command_url(session))
            .headers(headers)
            .json(&frame)
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP command failed: {error}")))?;
        let status = response.status();
        let body: CommandResponseBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP command response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(body.error_message()));
        }
        Ok(body.events)
    }

    /// Posts a standard `SESSION_CONFIGURE` frame (RFC 8.1) with an
    /// `agent_override` fragment and streams the resulting events. The wire
    /// shape is the spec's — `agent_id`, `agent_override`, `options`,
    /// `apply` — not a serialised command enum; the host maps its known
    /// fields onto its service. Operations without an `agent_override`
    /// equivalent (resume, compact, jobs, questions) do not go through
    /// here: they use the extension command or their standard frame.
    pub(crate) async fn submit_configure(
        &self,
        session: &str,
        agent_override: serde_json::Value,
    ) -> Result<EventStream, ClientError> {
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::SessionConfigure),
                serde_json::to_value(serde_json::json!({
                    "agent_override": agent_override,
                    "apply": "now",
                }))
                .map_err(|error| ClientError::new(format!("serialize command: {error}")))?,
            )
            .await?;
        let events = frames
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    pub(crate) async fn submit_http(
        &self,
        request: ClientRequest,
    ) -> Result<EventStream, ClientError> {
        if let norte_code_wire::ClientRequestKind::SetReasoning {
            session,
            scope: norte_code_wire::ReasoningScope::Session,
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
                norte_code_wire::ClientRequestKind::SendMessage { .. }
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
        let response_body: CommandResponseBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(response_body.error_message()));
        }
        let events = response_body
            .events
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        for frame in &response_body.events {
            self.note_frame_seq(frame);
        }
        self.remember_sessions(&events);
        Ok(Box::pin(iter(events)))
    }

    pub(crate) fn events_url(&self, session_id: &str) -> String {
        format!(
            "{}/raep/v1/sessions/{}/events",
            self.base_url,
            percent_encode(session_id)
        )
    }

    /// RAEP `QueuePolicy` for a [`QueueMode`]: the two enums spell the
    /// same three values, so the mapping is exhaustive and cannot fall
    /// through to a default.
    pub(crate) fn raep_queue_policy(mode: QueueMode) -> raep_types::QueuePolicy {
        match mode {
            QueueMode::Reject => raep_types::QueuePolicy::Reject,
            QueueMode::Queue => raep_types::QueuePolicy::Queue,
            QueueMode::Interrupt => raep_types::QueuePolicy::Interrupt,
        }
    }

    /// Submits one `UserInput` frame and maps its immediate reply onto the
    /// `NorteCode` event stream.
    pub(crate) async fn post_user_input(
        &self,
        session: &str,
        text: &str,
        queue_policy: raep_types::QueuePolicy,
    ) -> Result<EventStream, ClientError> {
        let input = UserInput {
            prompt: Some(text.to_owned()),
            queue_policy: Some(queue_policy),
            // Caller metadata (RFC 12.4): opaque to the model, stored with
            // the turn. The CLI sends only which client it is.
            metadata: Some(serde_json::json!({ "client_id": "cli" })),
            ..UserInput::default()
        };
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::UserInput),
                serde_json::to_value(input)
                    .map_err(|error| ClientError::new(format!("serialize prompt: {error}")))?,
            )
            .await?;
        let events = frames
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn submit_streaming(
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
        let command_body = Envelope::new(
            FrameType::Extension("norte_code.command".to_owned()),
            serde_json::to_value(request)
                .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
            timestamp_millis(),
        );
        let command_result: Arc<Mutex<Option<Result<(), ClientError>>>> =
            Arc::new(Mutex::new(None));
        let command_result_task = Arc::clone(&command_result);
        let http_for_command = self.http.clone();
        let url_for_command = command_url.clone();
        let headers_for_command = command_headers.clone();
        tokio::spawn(async move {
            let outcome = async {
                let response = http_for_command
                    .post(url_for_command)
                    .headers(headers_for_command)
                    .json(&command_body)
                    .send()
                    .await
                    .map_err(|error| ClientError::new(format!("RAEP command failed: {error}")))?;
                if !response.status().is_success() {
                    let status = response.status();
                    let body: CommandResponseBody =
                        response.json().await.unwrap_or(CommandResponseBody {
                            events: Vec::new(),
                            error: None,
                        });
                    return Err(ClientError::new(format!(
                        "RAEP command returned HTTP {status}: {}",
                        body.error_message()
                    )));
                }
                Ok(())
            }
            .await;
            *command_result_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
        });

        let session = norte_code_wire::SessionKey::new(session_id);
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
                command_result,
                false,
            ),
            move |(
                mut bytes,
                mut buffer,
                mut pending,
                session,
                mut finished,
                session_seq,
                command_result,
                mut command_reported,
            )| {
                let ack_http = ack_http.clone();
                let ack_url = ack_url.clone();
                let ack_token = ack_token.clone();
                async move {
                    if !command_reported {
                        let result = command_result
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone();
                        if let Some(Err(error)) = result {
                            command_reported = true;
                            finished = true;
                            pending.push_back((
                                ServerEvent {
                                    request_id: 0,
                                    session: Some(session.clone()),
                                    kind: norte_code_wire::ServerEventKind::Error {
                                        code: norte_code_wire::ErrorCode::Internal,
                                        message: error.to_string(),
                                    },
                                },
                                true,
                            ));
                        } else if result.is_some() {
                            command_reported = true;
                        }
                    }
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
                                    command_result,
                                    command_reported,
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
                                    SeqStep::Duplicate => {
                                        // Still advance terminal bookkeeping? no.
                                        if event.is_none() {
                                            // undecodable frame: skip yielding
                                        }
                                        continue;
                                    }
                                    SeqStep::Gap { expected } => {
                                        drop(seqs);
                                        finished = true;
                                        pending.push_back((
                                            ServerEvent {
                                                request_id: 0,
                                                session: Some(session.clone()),
                                                kind: norte_code_wire::ServerEventKind::Error {
                                                    code: norte_code_wire::ErrorCode::Internal,
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
                            if let Some(event) = event {
                                pending.push_back((event, terminal));
                            }
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
                                        serde_json::to_value(Ack { last_seq: seq })
                                            .unwrap_or(Value::Null),
                                        timestamp_millis(),
                                    ))
                                    .send()
                                    .await;
                            });
                        }
                        if finished {
                            continue;
                        }
                        #[allow(clippy::single_match_else)]
                        match bytes.next().await {
                            Some(Ok(chunk)) => {
                                buffer.extend_from_slice(&chunk);
                            }
                            Some(Err(_)) | None => {
                                if !command_reported {
                                    let result = command_result
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .clone();
                                    if let Some(Err(error)) = result {
                                        command_reported = true;
                                        finished = true;
                                        pending.push_back((
                                            ServerEvent {
                                                request_id: 0,
                                                session: Some(session.clone()),
                                                kind: norte_code_wire::ServerEventKind::Error {
                                                    code: norte_code_wire::ErrorCode::Internal,
                                                    message: error.to_string(),
                                                },
                                            },
                                            true,
                                        ));
                                    } else if result.is_some() {
                                        command_reported = true;
                                    }
                                }
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

    pub(crate) fn remember_sessions(&self, events: &[ServerEvent]) {
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
                norte_code_wire::ServerEventKind::SessionList { sessions } => {
                    for session in sessions {
                        cache.insert(session.session.as_str().to_owned(), session.clone());
                    }
                }
                norte_code_wire::ServerEventKind::SessionCreated { title } => {
                    if let Some(key) = &event.session {
                        cache.insert(
                            key.as_str().to_owned(),
                            norte_code_wire::SessionInfo {
                                session: key.clone(),
                                title: title.clone(),
                                provider: None,
                                model: None,
                                created_at: None,
                                phase: norte_code_wire::Phase::Idle,
                                reasoning: None,
                                parent: None,
                            },
                        );
                    }
                }
                norte_code_wire::ServerEventKind::SessionRenamed { title } => {
                    if let Some(key) = &event.session
                        && let Some(info) = cache.get_mut(key.as_str())
                    {
                        info.title = Some(title.clone());
                    }
                }
                norte_code_wire::ServerEventKind::ModelChanged { provider, model }
                | norte_code_wire::ServerEventKind::ModelQueued { provider, model } => {
                    if let Some(key) = &event.session
                        && let Some(info) = cache.get_mut(key.as_str())
                    {
                        info.provider = Some(provider.clone());
                        info.model = Some(model.clone());
                    }
                }
                norte_code_wire::ServerEventKind::SessionStats { .. } => {
                    if let Some(key) = &event.session
                        && let norte_code_wire::ServerEventKind::SessionStats {
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
                norte_code_wire::ServerEventKind::JobList { jobs: listed } => {
                    if let Some(key) = &event.session {
                        jobs.insert(key.as_str().to_owned(), listed.clone());
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) async fn ensure_attached(&self, session_id: &str) -> Result<(), ClientError> {
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
                serde_json::to_value(SessionAttach {
                    agent_id: self.agent_id.clone(),
                    agent_override: None,
                    create_if_missing: Some(false),
                    mode: None,
                    takeover: None,
                    resume_from_seq: self.last_seq(session_id),
                    capabilities: Some(self.capabilities.clone()),
                    // Client identity (RFC 6): diagnostics only. No behavior
                    // hangs off this; it names the client in the server log.
                    client_info: Some(ClientInfo {
                        name: Some("norte-cli".to_owned()),
                        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                    }),
                    options: None,
                })
                .unwrap_or(Value::Null),
                timestamp_millis(),
            ))
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP attach failed: {error}")))?;
        let status = response.status();
        let body: CommandResponseBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP attach response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(body.error_message()));
        }
        for frame in &body.events {
            self.note_frame_seq(frame);
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

    async fn send_message(
        &self,
        session: &str,
        text: &str,
        attachments: &[norte_code_wire::Attachment],
    ) -> Result<EventStream, ClientError> {
        self.submit_streaming(
            session,
            ClientRequest {
                request_id: 0,
                kind: norte_code_wire::ClientRequestKind::SendMessage {
                    session: Some(norte_code_wire::SessionKey::new(session)),
                    text: text.to_owned(),
                    attachments: attachments.to_vec(),
                    budget: None,
                    reasoning: None,
                    output: Vec::new(),
                    response_format: None,
                    metadata: None,
                    replay: false,
                },
            },
        )
        .await
    }

    async fn steer_message(
        &self,
        session: &str,
        text: &str,
        queue_mode: QueueMode,
    ) -> Result<EventStream, ClientError> {
        self.post_user_input(session, text, Self::raep_queue_policy(queue_mode))
            .await
    }

    async fn cancel_turn(&self, session: &str) -> Result<(), ClientError> {
        self.post_standard(
            session,
            FrameType::Standard(StandardFrameType::CancelTurn),
            serde_json::to_value(raep_types::CancelTurn { reason: None }).unwrap_or(Value::Null),
        )
        .await?;
        Ok(())
    }

    async fn set_model(
        &self,
        session: &str,
        provider: &str,
        model: &str,
    ) -> Result<EventStream, ClientError> {
        // `SESSION_CONFIGURE.agent_override` with the `model` fragment.
        let mut override_value = serde_json::json!({
            "model": { "id": model },
        });
        if !provider.is_empty() {
            override_value["provider"] = serde_json::Value::String(provider.to_owned());
        }
        self.submit_configure(session, override_value).await
    }

    async fn set_reasoning(
        &self,
        session: &str,
        reasoning: norte_code_wire::Reasoning,
    ) -> Result<EventStream, ClientError> {
        let override_value = Self::reasoning_override(&reasoning);
        self.submit_configure(session, override_value).await
    }

    async fn set_budget(
        &self,
        session: &str,
        budget: norte_code_wire::Budget,
    ) -> Result<EventStream, ClientError> {
        let override_value = Self::budget_override(&budget);
        self.submit_configure(session, override_value).await
    }

    async fn resume(&self, session: &str) -> Result<EventStream, ClientError> {
        // No `agent_override` equivalent: resume goes through the extension
        // command as a `ClientRequestKind::Resume`.
        let request = ClientRequest {
            request_id: 0,
            kind: norte_code_wire::ClientRequestKind::Resume {
                session: norte_code_wire::SessionKey::new(session),
                replay: true,
            },
        };
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(self.command_url(session))
            .headers(headers)
            .json(&Envelope::new(
                FrameType::Extension("norte_code.command".to_owned()),
                serde_json::to_value(&request)
                    .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
                timestamp_millis(),
            ))
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP request failed: {error}")))?;
        let status = response.status();
        let response_body: CommandResponseBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(response_body.error_message()));
        }
        let events = response_body
            .events
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    async fn fork_session(&self, session: &str) -> Result<SessionSummary, ClientError> {
        // `SessionFork` has no payload struct in raep-types yet, so the
        // standard frame carries the session id it forks, in the shape the
        // spec's fork request uses.
        let events = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::SessionFork),
                serde_json::to_value(ForkPayload {
                    session_id: session.to_owned(),
                })
                .unwrap_or(Value::Null),
            )
            .await?;
        for frame in events {
            if let Some(event) = server_event_from_frame(&frame).ok()
                && let norte_code_wire::ServerEventKind::SessionForked { .. } = event.kind
                && let Some(key) = event.session
            {
                return Ok(SessionSummary {
                    session_id: key.as_str().to_owned(),
                    title: None,
                });
            }
        }
        Err(ClientError::new(
            "the server did not return the forked session",
        ))
    }

    async fn export_session(&self, session: &str) -> Result<EventStream, ClientError> {
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::SessionExport),
                serde_json::json!({ "include": {} }),
            )
            .await?;
        let events = frames
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    async fn import_session(
        &self,
        session: &str,
        bundle: serde_json::Value,
    ) -> Result<EventStream, ClientError> {
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::SessionImport),
                serde_json::json!({ "bundle": bundle }),
            )
            .await?;
        let events = frames
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    async fn compact_session(&self, session: &str) -> Result<EventStream, ClientError> {
        // No `agent_override` equivalent: compact goes through the extension
        // command as `ClientRequestKind::Compact`.
        let request = ClientRequest {
            request_id: 0,
            kind: norte_code_wire::ClientRequestKind::Compact {
                session: norte_code_wire::SessionKey::new(session),
            },
        };
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(self.command_url(session))
            .headers(headers)
            .json(&Envelope::new(
                FrameType::Extension("norte_code.command".to_owned()),
                serde_json::to_value(&request)
                    .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
                timestamp_millis(),
            ))
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP request failed: {error}")))?;
        let status = response.status();
        let response_body: CommandResponseBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(response_body.error_message()));
        }
        let events = response_body
            .events
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .get(format!("{}/raep/v1/sessions", self.base_url))
            .headers(headers)
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP list sessions failed: {error}")))?;
        let body: SessionListBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP session list: {error}")))?;
        Ok(body.sessions)
    }

    async fn create_session(&self, title: Option<&str>) -> Result<SessionSummary, ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(format!("{}/raep/v1/sessions", self.base_url))
            .headers(headers)
            .json(&CreateSessionBody {
                agent_id: self.agent_id.clone(),
                title: title.map(str::to_owned),
            })
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP create session failed: {error}")))?;
        let status = response.status();
        let body: SessionSummary = response.json().await.map_err(|error| {
            ClientError::new(format!("invalid RAEP create session response: {error}"))
        })?;
        if !status.is_success() {
            return Err(ClientError::new("RAEP create session was rejected"));
        }
        Ok(body)
    }

    async fn rename_session(&self, session: &str, title: &str) -> Result<(), ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .patch(format!(
                "{}/raep/v1/sessions/{}",
                self.base_url,
                percent_encode(session)
            ))
            .headers(headers)
            .json(&RenameSessionBody {
                title: title.to_owned(),
            })
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP rename session failed: {error}")))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::new("RAEP rename session was rejected"))
        }
    }

    async fn delete_session(&self, session: &str) -> Result<(), ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .delete(format!(
                "{}/raep/v1/sessions/{}",
                self.base_url,
                percent_encode(session)
            ))
            .headers(headers)
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP delete session failed: {error}")))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::new("RAEP delete session was rejected"))
        }
    }
}

/// A session list body from the RAEP management resource.
#[derive(Debug, serde::Deserialize)]
struct SessionListBody {
    #[serde(default)]
    sessions: Vec<SessionSummary>,
}

/// A create-session request body.
#[derive(Debug, serde::Serialize)]
struct CreateSessionBody {
    agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

/// A rename-session request body.
#[derive(Debug, serde::Serialize)]
struct RenameSessionBody {
    title: String,
}
