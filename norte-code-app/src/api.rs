use crate::domain::{Capability, ConversationItem, HistoryKind, HistoryRole, SessionStatus};
use raep_types::TurnId;
use raep_types::{ClientInfo, Envelope, FrameType, SessionAttach, StandardFrameType};
use reqwest::Url;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

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

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AgentSummary {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub provider: Option<AgentReference>,
    #[serde(default)]
    pub model: Option<AgentReference>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct AgentReference {
    #[serde(default)]
    pub id: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct SessionSummary {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: SessionStatus,
    #[serde(default)]
    pub agent: Option<AgentReference>,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub updated_at: Option<i64>,
    #[serde(default)]
    pub last_activity_at: Option<i64>,
    #[serde(default)]
    pub turn_count: Option<u64>,
    #[serde(default)]
    pub has_pending: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct TelemetryReport {
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub records: Vec<TelemetryRecord>,
    #[serde(default)]
    pub summary: TelemetrySummary,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct UploadReceipt {
    pub upload_id: String,
    pub mime_type: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct TelemetrySummary {
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub errors: u64,
    #[serde(default)]
    pub cancelled: u64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct TelemetryRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub first_token_ms: Option<u64>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub cache_write_tokens: Option<u64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub status: TelemetryStatus,
    #[serde(default)]
    pub tool_calls: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryStatus {
    #[default]
    Ok,
    Error,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobStatus {
    #[default]
    Running,
    Completed,
    Failed,
    Killed,
    TimedOut,
}

impl JobStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Killed => "Stopped",
            Self::TimedOut => "Timed out",
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            Self::Running => "status status-active",
            Self::Completed => "status status-success",
            Self::Failed | Self::TimedOut => "status status-error",
            Self::Killed => "status status-muted",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct JobSummary {
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub status: JobStatus,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub output_snapshot: Option<String>,
}

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
struct CommandResponse {
    #[serde(default)]
    accepted: bool,
    #[serde(default)]
    events: Vec<Envelope<Value>>,
    #[serde(default)]
    error: Option<RemoteError>,
}

#[derive(Deserialize)]
struct RemoteError {
    #[serde(default)]
    message: String,
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

#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum JobFilterStatus {
    Running,
    Completed,
    Failed,
    Killed,
    TimedOut,
}

#[derive(Serialize)]
struct JobListRequest {
    status: Option<Vec<JobFilterStatus>>,
    limit: u64,
}

#[derive(Deserialize)]
struct JobListResponse {
    #[serde(default)]
    jobs: Vec<JobSummary>,
}

#[derive(Serialize)]
struct CancelJobRequest {
    grace_ms: u64,
}

#[derive(Serialize)]
struct EmptyRequest {}

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

    pub async fn info(&self) -> Result<ServerInfo, ApiError> {
        self.get_json("raep/v1/info").await
    }

    pub async fn agents(&self) -> Result<Vec<AgentSummary>, ApiError> {
        let response: AgentListResponse = self.get_json("raep/v1/agents").await?;
        Ok(response.agents)
    }

    pub async fn sessions(&self) -> Result<Vec<SessionSummary>, ApiError> {
        let response: SessionListResponse = self.get_json("raep/v1/sessions").await?;
        Ok(response.sessions)
    }

    pub async fn create_session(
        &self,
        agent_id: &str,
        title: Option<&str>,
    ) -> Result<SessionSummary, ApiError> {
        let url = self.resource_url("raep/v1/sessions")?;
        self.send_json(
            self.http
                .post(url)
                .json(&CreateSessionRequest { agent_id, title }),
        )
        .await
    }

    pub async fn telemetry(&self, session_id: Option<&str>) -> Result<TelemetryReport, ApiError> {
        let mut url = self.resource_url("raep/v1/telemetry")?;
        if let Some(session_id) = session_id {
            url.query_pairs_mut().append_pair("session", session_id);
        }
        self.send_json(self.http.get(url)).await
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

    pub async fn send_text(&self, session_id: &str, text: &str) -> Result<TurnId, ApiError> {
        let turn_id = TurnId::generate();
        let request = crate::wire::UserInputRequest {
            prompt: Some(text),
            content: None,
        };
        self.send_user_input(session_id, &turn_id, &request).await?;
        Ok(turn_id)
    }

    pub async fn send_content(
        &self,
        session_id: &str,
        content: Vec<crate::wire::UserInputPart>,
    ) -> Result<TurnId, ApiError> {
        let turn_id = TurnId::generate();
        let request = crate::wire::UserInputRequest {
            prompt: None,
            content: Some(content),
        };
        self.send_user_input(session_id, &turn_id, &request).await?;
        Ok(turn_id)
    }

    async fn send_user_input(
        &self,
        session_id: &str,
        turn_id: &TurnId,
        request: &crate::wire::UserInputRequest<'_>,
    ) -> Result<(), ApiError> {
        let response = self
            .post_turn_frame(
                session_id,
                FrameType::Standard(StandardFrameType::UserInput),
                turn_id,
                &request,
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Message was rejected.".to_owned(),
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

    pub async fn history(&self, session_id: &str) -> Result<Vec<ConversationItem>, ApiError> {
        let response = self
            .post_frame(
                session_id,
                FrameType::Standard(StandardFrameType::HistoryGet),
                &HistoryGetRequest {
                    before_turn_id: None,
                    limit: 100,
                    include_rewound: false,
                },
            )
            .await?;
        let page = response
            .events
            .iter()
            .find(|frame| frame.kind == FrameType::Standard(StandardFrameType::HistoryPage))
            .ok_or_else(|| {
                ApiError::InvalidResponse("History page was not returned.".to_owned())
            })?;
        let page: HistoryPage = decode_payload(&page.payload)?;
        Ok(page
            .items
            .into_iter()
            .enumerate()
            .map(|(index, item)| ConversationItem {
                id: format!(
                    "{}:{}:{index}",
                    item.turn_id.as_deref().unwrap_or("unassigned"),
                    item.timestamp
                ),
                turn_id: item.turn_id,
                role: item.role,
                kind: item.kind,
                content: item.content,
                created_at: item.timestamp,
            })
            .collect())
    }

    pub async fn list_jobs(&self, session_id: &str) -> Result<Vec<JobSummary>, ApiError> {
        let response = self
            .post_frame(
                session_id,
                FrameType::Standard(StandardFrameType::JobList),
                &JobListRequest {
                    status: Some(vec![
                        JobFilterStatus::Running,
                        JobFilterStatus::Completed,
                        JobFilterStatus::Failed,
                        JobFilterStatus::Killed,
                        JobFilterStatus::TimedOut,
                    ]),
                    limit: 100,
                },
            )
            .await?;
        let result = response
            .events
            .iter()
            .find(|frame| frame.kind == FrameType::Standard(StandardFrameType::JobListResult))
            .ok_or_else(|| ApiError::InvalidResponse("Job list was not returned.".to_owned()))?;
        let result: JobListResponse = decode_payload(&result.payload)?;
        Ok(result.jobs)
    }

    pub async fn cancel_turn(&self, session_id: &str, turn_id: &TurnId) -> Result<(), ApiError> {
        let response = self
            .post_turn_frame(
                session_id,
                FrameType::Standard(StandardFrameType::CancelTurn),
                turn_id,
                &EmptyRequest {},
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Turn cancellation was rejected.".to_owned(),
                |e| e.message,
            )))
        }
    }

    pub async fn cancel_job(&self, session_id: &str, job_id: &str) -> Result<(), ApiError> {
        let url = self.session_url(session_id, "commands")?;
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::CancelJob),
            serde_json::to_value(CancelJobRequest { grace_ms: 5_000 })
                .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            now_millis(),
        );
        frame.session_id = session_id.to_owned();
        frame.job_id = Some(
            raep_types::JobId::new(job_id.to_owned())
                .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
        );
        let response: CommandResponse = self
            .send_json(self.auth(self.http.post(url.clone()).json(&frame)))
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Job cancellation was rejected.".to_owned(),
                |e| e.message,
            )))
        }
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

    async fn post_frame<P: Serialize>(
        &self,
        session_id: &str,
        kind: FrameType,
        payload: &P,
    ) -> Result<CommandResponse, ApiError> {
        let url = self.session_url(session_id, "commands")?;
        let payload = serde_json::to_value(payload)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let mut frame = Envelope::new(kind, payload, now_millis());
        frame.session_id = session_id.to_owned();
        self.send_json(self.auth(self.http.post(url).json(&frame)))
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
        let mut frame = Envelope::new(kind, payload, now_millis());
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
        let mut url = self.resource_url("raep/v1/sessions/")?;
        url.path_segments_mut()
            .map_err(|()| {
                ApiError::InvalidUrl("Server URL cannot contain path segments.".to_owned())
            })?
            .push(session_id)
            .push(suffix);
        Ok(url)
    }
}

pub async fn delay(duration: Duration) {
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(
        u32::try_from(duration.as_millis()).unwrap_or(u32::MAX),
    )
    .await;
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(duration).await;
}

fn decode_payload<T: DeserializeOwned>(payload: &Value) -> Result<T, ApiError> {
    serde_json::from_value(payload.clone())
        .map_err(|error| ApiError::InvalidResponse(error.to_string()))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
