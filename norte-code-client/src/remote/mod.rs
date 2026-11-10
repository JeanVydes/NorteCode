mod commands;
mod session;
mod stream;

use norte_code_wire::{ClientRequest, ServerEvent};
use raep_types::{ClientInfo, Envelope, FrameType, SessionAttach, StandardFrameType};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::decode::{percent_encode, session_from_request, timestamp_millis};
use crate::error::ClientError;
use crate::frame::CommandResponseBody;

/// HTTP client for the `NorteCode` RAEP command endpoint.
///
/// The transport returns canonical RAEP envelopes, while the compatibility
/// extension carries the original `NorteCode` event until all consumers
/// migrate.
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
