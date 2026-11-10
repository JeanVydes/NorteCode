use crate::domain::{Capability, ConversationItem, HistoryKind, HistoryRole, MessageDelivery};
use norte_code_wire::CanonicalEvent;
use norte_code_wire::{CatalogModelInfo, CatalogProviderInfo};
use raep_types::TurnId;
use raep_types::{ClientInfo, Envelope, FrameType, SessionAttach, StandardFrameType};
use reqwest::Url;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

/// Which model a session starts on, as `SessionCreate` accepts it.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentOverride<'a> {
    pub model: AgentModelRef<'a>,
    pub provider: &'a str,
}

/// The model half of an override.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentModelRef<'a> {
    pub id: &'a str,
}

/// The `SESSION_EXPORT` body: every optional section off, so the bundle holds
/// the conversation and its metadata only.
#[derive(Clone, Copy, Debug, Default, Serialize)]
struct ExportRequest {
    include: ExportInclude,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct ExportInclude {
    children: bool,
    uploads: bool,
    job_output: bool,
    outputs: bool,
    workspace: bool,
}

// The settings `SET_REASONING` and `SET_BUDGET` carry are the protocol's own
// values, shared with the terminal client and the host's decoder.
use norte_code_wire::{
    Budget, ClientRequest, ClientRequestKind, McpFailure, McpInfo, Reasoning, ReasoningScope,
    SessionKey, SkillInfo,
};

/// The editable catalogue as the host returns it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CatalogDetail {
    #[serde(default)]
    pub providers: Vec<CatalogProviderInfo>,
    #[serde(default)]
    pub models: Vec<CatalogModelInfo>,
}

const CLIENT_ID: &str = "norte-code-dioxus";
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base_url: Url,
    bearer_token: Option<String>,
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Invalid server URL: {0}")]
    InvalidUrl(String),
    #[error("Request failed: {0}")]
    Request(String),
    #[error("Server returned HTTP {0}")]
    HttpStatus(u16),
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub server_version: String,
    #[serde(default)]
    pub protocol: Option<ProtocolInfo>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub limits: ServerLimits,
    #[serde(default)]
    pub default_agent: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Live,
    Ready,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Liveness {
    pub status: HealthStatus,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Readiness {
    pub status: HealthStatus,
    pub agent_id: String,
    pub agents: usize,
    pub replay: bool,
    pub artifacts: bool,
    pub limits: ReadinessLimits,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ReadinessLimits {
    pub max_connections: usize,
    pub command_timeout_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProtocolInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerLimits {
    #[serde(rename = "max_frame_bytes")]
    pub frame_bytes: Option<u64>,
    #[serde(rename = "max_concurrent_jobs")]
    pub concurrent_jobs: Option<u64>,
    #[serde(rename = "max_artifact_bytes")]
    pub artifact_bytes: Option<u64>,
    #[serde(rename = "max_prompt_bytes")]
    pub prompt_bytes: Option<u64>,
}

use norte_code_wire::AgentSummary;

// The model list is the protocol's model description, not a second one.
use norte_code_wire::{ModelInfo as ModelSummary, QuotaReport};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct PluginSummary {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub loaded: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct PluginChange {
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct SkillCatalog {
    #[serde(default)]
    pub skills: Vec<SkillInfo>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ToolRule {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub allowed: bool,
    #[serde(default)]
    pub scope: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct McpCatalog {
    #[serde(default)]
    pub servers: Vec<McpInfo>,
    #[serde(default)]
    pub failures: Vec<McpFailure>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct CostReport {
    #[serde(default)]
    pub total_usd: Option<f64>,
    #[serde(default)]
    pub requests: u64,
}

// The session list is the shared shape, decoded from the same definition the
// host serializes.
use norte_code_wire::SessionSummary;

/// Where a child session stands.
///
/// The same type the event stream carries, not a second enum that has to be
/// translated: two names for one state is how `RUNNING` and `Streaming` drift
/// apart and a subagent shows as waiting when it is working.
use norte_code_wire::ChildSessionSummary;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct UploadReceipt {
    pub upload_id: String,
    pub mime_type: String,
    pub bytes: u64,
}

// The telemetry shapes are the wire's: the host serializes them from the same
// definitions this client decodes.
use norte_code_wire::TelemetryReport;

// The job and plan shapes are the wire's, not this client's: the host serializes
// them from the same definitions, so a status cannot disagree with the server.
use norte_code_wire::{JobSummary, PlanSummary};

#[derive(Deserialize)]
struct AgentListResponse {
    #[serde(default)]
    agents: Vec<AgentSummary>,
}

#[derive(Deserialize)]
struct SessionListResponse {
    #[serde(default)]
    sessions: Vec<SessionSummary>,
}

#[derive(Deserialize)]
struct PlanListResponse {
    #[serde(default)]
    entries: Vec<PlanSummary>,
}

#[derive(Deserialize)]
struct ChildSessionListResponse {
    #[serde(default)]
    children: Vec<ChildSessionSummary>,
}

#[derive(Deserialize)]
pub(crate) struct CommandResponse {
    #[serde(default)]
    pub(crate) accepted: bool,
    #[serde(default)]
    pub(crate) events: Vec<Envelope<Value>>,
    #[serde(default)]
    pub(crate) error: Option<RemoteError>,
}

#[derive(Deserialize)]
pub(crate) struct RemoteError {
    #[serde(default)]
    pub(crate) message: String,
}

#[derive(Deserialize)]
struct RemoteErrorResponse {
    #[serde(default)]
    error: Option<RemoteError>,
}

#[derive(Serialize)]
struct CreateSessionRequest<'a> {
    agent_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    /// The model the session starts on, applied before its first turn. The
    /// host translates it through the same path as `SESSION_CONFIGURE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_override: Option<AgentOverride<'a>>,
}

#[derive(Serialize)]
struct RenameSessionRequest<'a> {
    title: &'a str,
}

#[derive(Serialize)]
struct HistoryGetRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    before_turn_id: Option<String>,
    limit: u64,
    include_rewound: bool,
}

#[derive(Deserialize)]
struct HistoryPage {
    #[serde(default)]
    items: Vec<HistoryItem>,
}

#[derive(Deserialize)]
struct HistoryItem {
    role: HistoryRole,
    kind: HistoryKind,
    content: String,
    #[serde(default)]
    turn_id: Option<String>,
    timestamp: i64,
}

/// The payload of a frame that carries none.
#[derive(Serialize)]
struct EmptyRequest {}

#[derive(Deserialize)]
struct AgentCatalogResponse {
    #[serde(default)]
    entries: Vec<ModelSummary>,
}

#[derive(Deserialize)]
struct PluginListResponse {
    #[serde(default)]
    plugins: Vec<PluginSummary>,
}

#[derive(Deserialize)]
struct PluginChangedResponse {
    #[serde(default)]
    operation: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct AgentCatalogSkillsResponse {
    #[serde(default)]
    skills: Vec<SkillInfo>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct McpListResponse {
    #[serde(default)]
    servers: Vec<McpInfo>,
    #[serde(default)]
    failures: Vec<McpFailure>,
}

#[derive(Deserialize)]
struct CostUpdatedResponse {
    total_usd: f64,
    requests: u64,
}

#[derive(Deserialize)]
struct CostNotReportedResponse {
    requests: u64,
}

mod catalog;
mod chat;
mod discovery;
mod mcp;
mod plugins;
mod sessions;
mod skills;
mod telemetry;

impl ApiClient {
    pub fn new(base_url: &str, bearer_token: &str) -> Result<Self, ApiError> {
        let mut base_url =
            Url::parse(base_url.trim()).map_err(|error| ApiError::InvalidUrl(error.to_string()))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(ApiError::InvalidUrl(
                "Only HTTP and HTTPS server URLs are supported.".to_owned(),
            ));
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            return Err(ApiError::InvalidUrl(
                "The server URL cannot contain a query or fragment.".to_owned(),
            ));
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        Ok(Self {
            http: reqwest::Client::new(),
            base_url,
            bearer_token: (!bearer_token.trim().is_empty()).then(|| bearer_token.trim().to_owned()),
        })
    }

    pub async fn set_session_model(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
    ) -> Result<(), ApiError> {
        self.norte_code_command(
            session_id,
            ClientRequestKind::SetModel {
                session: SessionKey::new(session_id),
                provider: provider.to_owned(),
                model: model.to_owned(),
            },
        )
        .await
        .map(|_| ())
    }

    async fn norte_code_command(
        &self,
        session_id: &str,
        kind: ClientRequestKind,
    ) -> Result<CommandResponse, ApiError> {
        let request = ClientRequest {
            request_id: u64::try_from(now_millis()?)
                .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            kind,
        };
        let response = self
            .post_frame(
                session_id,
                FrameType::Extension("norte_code.command".to_owned()),
                &request,
            )
            .await?;
        if response.accepted {
            Ok(response)
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Norte Code command was rejected.".to_owned(),
                |error| error.message,
            )))
        }
    }

    pub async fn attach(&self, session_id: &str, agent_id: &str) -> Result<(), ApiError> {
        let payload = SessionAttach {
            agent_id: agent_id.to_owned(),
            agent_override: None,
            create_if_missing: Some(false),
            mode: None,
            takeover: None,
            resume_from_seq: None,
            capabilities: None,
            client_info: Some(ClientInfo {
                name: Some(CLIENT_ID.to_owned()),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            options: None,
        };
        let response = self
            .post_frame(
                session_id,
                FrameType::Standard(StandardFrameType::SessionAttach),
                &payload,
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Session attachment was rejected.".to_owned(),
                |e| e.message,
            )))
        }
    }

    pub async fn open_session_events(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<crate::sse::SessionEventStream, ApiError> {
        let mut url = self.session_url(session_id, "events")?;
        url.query_pairs_mut()
            .append_pair("create_if_missing", "false");
        if !agent_id.trim().is_empty() {
            url.query_pairs_mut().append_pair("agent_id", agent_id);
        }
        #[cfg(target_arch = "wasm32")]
        {
            crate::sse::SessionEventStream::connect(url.as_str(), self.bearer_token.as_deref())
                .await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::sse::SessionEventStream::connect(
                &self.http,
                url.as_str(),
                self.bearer_token.as_deref(),
            )
            .await
        }
    }

    pub async fn detach(&self, session_id: &str) -> Result<(), ApiError> {
        let response = self
            .post_frame(
                session_id,
                FrameType::Standard(StandardFrameType::SessionDetach),
                &EmptyRequest {},
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Session detachment was rejected.".to_owned(),
                |e| e.message,
            )))
        }
    }

    pub async fn upload(
        &self,
        session_id: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<UploadReceipt, ApiError> {
        let url = self.session_url(session_id, "uploads")?;
        self.send_json(
            self.auth(
                self.http
                    .post(url)
                    .header(reqwest::header::CONTENT_TYPE, mime_type)
                    .body(bytes),
            ),
        )
        .await
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let url = self.resource_url(path)?;
        self.send_json(self.auth(self.http.get(url))).await
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let response = request
            .send()
            .await
            .map_err(|error| ApiError::Request(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.json::<RemoteErrorResponse>().await.ok();
            return Err(body.and_then(|body| body.error).map_or_else(
                || ApiError::HttpStatus(status.as_u16()),
                |error| ApiError::InvalidResponse(error.message),
            ));
        }
        response
            .json()
            .await
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))
    }

    async fn send_status(&self, request: reqwest::RequestBuilder) -> Result<(), ApiError> {
        let response = request
            .send()
            .await
            .map_err(|error| ApiError::Request(error.to_string()))?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let body = response.json::<RemoteErrorResponse>().await.ok();
        Err(body.and_then(|body| body.error).map_or_else(
            || ApiError::HttpStatus(status.as_u16()),
            |error| ApiError::InvalidResponse(error.message),
        ))
    }

    async fn post_frame<P: Serialize>(
        &self,
        session_id: &str,
        kind: FrameType,
        payload: &P,
    ) -> Result<CommandResponse, ApiError> {
        let url = self.session_url(session_id, "commands")?;
        let payload = serde_json::to_value(payload)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let mut frame = Envelope::new(kind, payload, now_millis()?);
        frame.session_id = session_id.to_owned();
        self.send_json(self.auth(self.http.post(url).json(&frame)))
            .await
    }

    /// Posts one frame to the session's command endpoint.
    pub(crate) async fn post_command(
        &self,
        session_id: &str,
        frame: &Envelope<Value>,
    ) -> Result<CommandResponse, ApiError> {
        let url = self.session_url(session_id, "commands")?;
        self.send_json(self.auth(self.http.post(url).json(frame)))
            .await
    }

    async fn post_turn_frame<P: Serialize>(
        &self,
        session_id: &str,
        kind: FrameType,
        turn_id: &TurnId,
        payload: &P,
    ) -> Result<CommandResponse, ApiError> {
        let url = self.session_url(session_id, "commands")?;
        let payload = serde_json::to_value(payload)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let mut frame = Envelope::new(kind, payload, now_millis()?);
        frame.session_id = session_id.to_owned();
        frame.turn_id = Some(turn_id.clone());
        self.send_json(self.auth(self.http.post(url).json(&frame)))
            .await
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.bearer_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    fn resource_url(&self, path: &str) -> Result<Url, ApiError> {
        self.base_url
            .join(path)
            .map_err(|error| ApiError::InvalidUrl(error.to_string()))
    }

    fn session_url(&self, session_id: &str, suffix: &str) -> Result<Url, ApiError> {
        let mut url = self.session_resource_url(session_id)?;
        url.path_segments_mut()
            .map_err(|()| {
                ApiError::InvalidUrl("Server URL cannot contain path segments.".to_owned())
            })?
            .push(suffix);
        Ok(url)
    }

    fn session_resource_url(&self, session_id: &str) -> Result<Url, ApiError> {
        let mut url = self.resource_url("raep/v1/sessions")?;
        url.path_segments_mut()
            .map_err(|()| {
                ApiError::InvalidUrl("Server URL cannot contain path segments.".to_owned())
            })?
            .push(session_id);
        Ok(url)
    }
}

fn decode_payload<T: DeserializeOwned>(payload: &Value) -> Result<T, ApiError> {
    serde_json::from_value(payload.clone())
        .map_err(|error| ApiError::InvalidResponse(error.to_string()))
}

pub(crate) fn now_millis() -> Result<i64, ApiError> {
    #[cfg(target_arch = "wasm32")]
    {
        let timestamp = js_sys::Date::now();
        if !timestamp.is_finite() {
            return Err(ApiError::InvalidResponse(
                "The browser clock returned an invalid timestamp.".to_owned(),
            ));
        }
        timestamp.round().to_string().parse().map_err(|error| {
            ApiError::InvalidResponse(format!("Could not read the browser clock: {error}"))
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))
            .and_then(|duration| {
                i64::try_from(duration.as_millis())
                    .map_err(|error| ApiError::InvalidResponse(error.to_string()))
            })
    }
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
