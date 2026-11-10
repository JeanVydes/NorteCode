//! The REST surface of a Norte Code host, as one closed endpoint catalog.
//!
//! Every client — the app today, any other tomorrow — builds requests through
//! [`Endpoint`] and never spells a path or owns a request shape. The shapes
//! that cross the wire live in `norte-code-wire`; this module binds them to a
//! method and a path and executes them.

use reqwest::Method;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use norte_code_wire::{AgentRef, ClientRequest, ClientRequestKind, SessionKey};
use raep_types::{Envelope, FrameType};

/// What went wrong talking to the host.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The configured base URL is not an HTTP(S) URL.
    #[error("Invalid server URL: {0}")]
    InvalidUrl(String),
    /// The request did not complete.
    #[error("Request failed: {0}")]
    Request(String),
    /// The host answered with a non-success status.
    #[error("Server returned HTTP {0}")]
    HttpStatus(u16),
    /// The answer did not match the shape the endpoint declares.
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

/// The error body the host writes when it rejects a request.
#[derive(Clone, Debug, Deserialize)]
pub struct RemoteError {
    /// Written for the person reading it.
    #[serde(default)]
    pub message: String,
}

/// The answer to one command frame.
#[derive(Clone, Debug, Deserialize)]
pub struct CommandResponse {
    /// Whether the host accepted the frame.
    #[serde(default)]
    pub accepted: bool,
    /// The events the command produced.
    #[serde(default)]
    pub events: Vec<Envelope<Value>>,
    /// Why the host rejected it, when it did.
    #[serde(default)]
    pub error: Option<RemoteError>,
}

impl CommandResponse {
    /// The rejection message, or a generic one.
    #[must_use]
    pub fn error_message(&self) -> String {
        self.error.as_ref().map_or_else(
            || "RAEP command failed".to_owned(),
            |error| error.message.clone(),
        )
    }
}

/// A `SESSION_CREATE` body.
#[derive(Clone, Debug, Serialize)]
pub struct CreateSessionRequest<'a> {
    /// The agent the session runs.
    pub agent_id: &'a str,
    /// Its label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<&'a str>,
    /// The model it starts on, applied before the first turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_override: Option<AgentOverride<'a>>,
}

/// The model a session starts on.
#[derive(Clone, Debug, Serialize)]
pub struct AgentOverride<'a> {
    /// The model reference.
    pub model: AgentModelRef<'a>,
    /// The provider that serves it.
    pub provider: &'a str,
}

/// The model half of an override.
#[derive(Clone, Debug, Serialize)]
pub struct AgentModelRef<'a> {
    /// The model id.
    pub id: &'a str,
}

/// A `PATCH /sessions/{id}` body.
#[derive(Clone, Debug, Serialize)]
pub struct RenameSessionRequest<'a> {
    /// The new label.
    pub title: &'a str,
}

/// The query of a telemetry read.
#[derive(Clone, Copy, Debug, Default)]
pub struct TelemetryQuery<'a> {
    /// Only the rows of this session, when named.
    pub session: Option<&'a str>,
}

/// The query of an event stream.
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionEventsRequest<'a> {
    /// The session to attach to.
    pub session: &'a str,
    /// The agent, when the client names one.
    pub agent: Option<&'a str>,
}

/// Every HTTP endpoint the host serves.
///
/// Data-bearing variants carry their request struct, so a path can never be
/// built with the wrong body and a new endpoint is one variant.
#[derive(Clone, Debug)]
pub enum Endpoint<'a> {
    /// Server description and negotiated protocol.
    Info,
    /// Liveness probe.
    HealthLive,
    /// Readiness probe.
    HealthReady,
    /// The principal's quota.
    Quota,
    /// Request telemetry.
    Telemetry(TelemetryQuery<'a>),
    /// The agents the host exposes.
    Agents,
    /// One agent.
    Agent {
        /// The agent id.
        id: &'a str,
    },
    /// The sessions the principal can see.
    Sessions,
    /// Creates a session.
    CreateSession(CreateSessionRequest<'a>),
    /// One session.
    Session {
        /// The session id.
        id: &'a str,
    },
    /// Renames a session.
    RenameSession {
        /// The session id.
        id: &'a str,
        /// The new title.
        request: RenameSessionRequest<'a>,
    },
    /// Deletes a session.
    DeleteSession {
        /// The session id.
        id: &'a str,
    },
    /// The children of a session.
    ChildSessions {
        /// The session id.
        id: &'a str,
    },
    /// The event stream of a session.
    SessionEvents(SessionEventsRequest<'a>),
    /// The command endpoint of a session.
    SessionCommands {
        /// The session id.
        id: &'a str,
    },
    /// The command endpoint of the default agent.
    Commands,
    /// Uploads one artifact into a session.
    SessionUpload {
        /// The session id.
        id: &'a str,
    },
    /// Uploads one artifact without a session.
    Artifacts,
    /// Downloads an artifact.
    Artifact {
        /// The artifact id.
        id: &'a str,
    },
    /// Downloads an exported bundle.
    Export {
        /// The export id.
        id: &'a str,
    },
    /// Imports a bundle.
    Imports,
    /// Commits an import.
    ImportCommit {
        /// The import id.
        id: &'a str,
    },
    /// Deletes an import without committing it.
    DeleteImport {
        /// The import id.
        id: &'a str,
    },
}

impl Endpoint<'_> {
    /// The HTTP method of the endpoint.
    #[must_use]
    pub fn method(&self) -> Method {
        match self {
            Self::CreateSession(_)
            | Self::SessionCommands { .. }
            | Self::Commands
            | Self::SessionUpload { .. }
            | Self::Artifacts
            | Self::Imports
            | Self::ImportCommit { .. } => Method::POST,
            Self::RenameSession { .. } => Method::PATCH,
            Self::DeleteSession { .. } | Self::DeleteImport { .. } => Method::DELETE,
            Self::Info
            | Self::HealthLive
            | Self::HealthReady
            | Self::Quota
            | Self::Telemetry(_)
            | Self::Agents
            | Self::Agent { .. }
            | Self::Sessions
            | Self::Session { .. }
            | Self::ChildSessions { .. }
            | Self::SessionEvents(_)
            | Self::Artifact { .. }
            | Self::Export { .. } => Method::GET,
        }
    }

    /// The path the endpoint lives at, relative to the server root.
    #[must_use]
    pub fn path(&self) -> String {
        match self {
            Self::Info => "/raep/v1/info".to_owned(),
            Self::HealthLive => "/raep/v1/health/live".to_owned(),
            Self::HealthReady => "/raep/v1/health/ready".to_owned(),
            Self::Quota => "/raep/v1/quota".to_owned(),
            Self::Telemetry(query) => query.session.map_or_else(
                || "/raep/v1/telemetry".to_owned(),
                |session| format!("/raep/v1/telemetry?session={}", encode(session)),
            ),
            Self::Agents => "/raep/v1/agents".to_owned(),
            Self::Agent { id } => format!("/raep/v1/agents/{}", encode(id)),
            Self::Sessions | Self::CreateSession(_) => "/raep/v1/sessions".to_owned(),
            Self::Session { id } | Self::RenameSession { id, .. } | Self::DeleteSession { id } => {
                format!("/raep/v1/sessions/{}", encode(id))
            }
            Self::ChildSessions { id } => {
                format!("/raep/v1/sessions/{}/children", encode(id))
            }
            Self::SessionEvents(request) => {
                let mut path = format!(
                    "/raep/v1/sessions/{}/events?create_if_missing=false",
                    encode(request.session)
                );
                if let Some(agent) = request.agent.filter(|agent| !agent.trim().is_empty()) {
                    path.push_str("&agent_id=");
                    path.push_str(&encode(agent));
                }
                path
            }
            Self::SessionCommands { id } => {
                format!("/raep/v1/sessions/{}/commands", encode(id))
            }
            Self::Commands => "/raep/v1/commands".to_owned(),
            Self::SessionUpload { id } => {
                format!("/raep/v1/sessions/{}/uploads", encode(id))
            }
            Self::Artifacts => "/raep/v1/artifacts".to_owned(),
            Self::Artifact { id } => format!("/raep/v1/artifacts/{}", encode(id)),
            Self::Export { id } => format!("/raep/v1/exports/{}", encode(id)),
            Self::Imports => "/raep/v1/imports".to_owned(),
            Self::ImportCommit { id } => format!("/raep/v1/imports/{}/commit", encode(id)),
            Self::DeleteImport { id } => format!("/raep/v1/imports/{}", encode(id)),
        }
    }

    /// The JSON body, when the endpoint carries one.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::InvalidResponse`] when a request struct cannot be
    /// serialized, which a closed struct cannot do.
    pub fn body(&self) -> Result<Option<Value>, ApiError> {
        let body = match self {
            Self::CreateSession(request) => Some(
                serde_json::to_value(request)
                    .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            ),
            Self::RenameSession { request, .. } => Some(
                serde_json::to_value(request)
                    .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            ),
            _ => None,
        };
        Ok(body)
    }
}

/// Percent-encodes a path or query segment.
fn encode(value: &str) -> String {
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

/// The REST side of a Norte Code host.
///
/// It owns the base URL, the bearer token and one HTTP client. Streaming is
/// the platform's job: the frame decoder is shared, the socket is not.
#[derive(Clone)]
pub struct RestClient {
    http: reqwest::Client,
    base_url: String,
    bearer_token: Option<String>,
}

impl RestClient {
    /// Creates a client for a server root such as `http://127.0.0.1:8787`.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::InvalidUrl`] when the URL is not HTTP(S) or carries
    /// a query or fragment.
    pub fn new(base_url: &str, bearer_token: &str) -> Result<Self, ApiError> {
        let trimmed = base_url.trim();
        let parsed = reqwest::Url::parse(trimmed)
            .map_err(|error| ApiError::InvalidUrl(error.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ApiError::InvalidUrl(
                "Only HTTP and HTTPS server URLs are supported.".to_owned(),
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(ApiError::InvalidUrl(
                "The server URL cannot contain a query or fragment.".to_owned(),
            ));
        }
        Ok(Self {
            http: reqwest::Client::new(),
            base_url: trimmed.trim_end_matches('/').to_owned(),
            bearer_token: (!bearer_token.trim().is_empty()).then(|| bearer_token.trim().to_owned()),
        })
    }

    /// The bearer token, for a caller that opens its own stream.
    #[must_use]
    pub fn bearer_token(&self) -> Option<&str> {
        self.bearer_token.as_deref()
    }

    /// The absolute URL of an endpoint.
    #[must_use]
    pub fn url(&self, endpoint: &Endpoint<'_>) -> String {
        format!("{}{}", self.base_url, endpoint.path())
    }

    /// Executes an endpoint and decodes its answer.
    ///
    /// # Errors
    ///
    /// Returns the transport error, a non-success status, or a body that does
    /// not match `T`.
    pub async fn execute<T: DeserializeOwned>(
        &self,
        endpoint: Endpoint<'_>,
    ) -> Result<T, ApiError> {
        let response = self.send(&endpoint, None).await?;
        decode(response).await
    }

    /// Executes an endpoint whose answer has no body.
    ///
    /// # Errors
    ///
    /// Returns the transport error or a non-success status.
    pub async fn execute_empty(&self, endpoint: Endpoint<'_>) -> Result<(), ApiError> {
        let response = self.send(&endpoint, None).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(status_error(response).await)
        }
    }

    /// Sends one frame to a session's command endpoint.
    ///
    /// # Errors
    ///
    /// Returns the transport error, a non-success status, or a rejected frame.
    pub async fn post_frame<P: Serialize>(
        &self,
        session: &str,
        kind: FrameType,
        payload: &P,
    ) -> Result<CommandResponse, ApiError> {
        let payload = serde_json::to_value(payload)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let mut frame = Envelope::new(kind, payload, now_millis()?);
        frame.session_id = session.to_owned();
        self.post_command(session, &frame).await
    }

    /// Sends one frame to the default agent's command endpoint.
    ///
    /// # Errors
    ///
    /// Returns the transport error or a non-success status.
    pub async fn post_agent_frame<P: Serialize>(
        &self,
        kind: FrameType,
        payload: &P,
    ) -> Result<CommandResponse, ApiError> {
        let payload = serde_json::to_value(payload)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let frame = Envelope::new(kind, payload, now_millis()?);
        self.post_frame_at(Endpoint::Commands, &frame).await
    }

    /// Sends a fully built frame to a session's command endpoint.
    ///
    /// # Errors
    ///
    /// Returns the transport error or a non-success status.
    pub async fn post_command(
        &self,
        session: &str,
        frame: &Envelope<Value>,
    ) -> Result<CommandResponse, ApiError> {
        self.post_frame_at(Endpoint::SessionCommands { id: session }, frame)
            .await
    }

    async fn post_frame_at(
        &self,
        endpoint: Endpoint<'_>,
        frame: &Envelope<Value>,
    ) -> Result<CommandResponse, ApiError> {
        let body = serde_json::to_value(frame)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let response = self.send(&endpoint, Some(body)).await?;
        decode(response).await
    }

    /// Sends a Norte Code command, typed, and reports the rejection message.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host rejected it.
    pub async fn command(
        &self,
        session: Option<&str>,
        kind: ClientRequestKind,
    ) -> Result<CommandResponse, ApiError> {
        let request = ClientRequest {
            request_id: u64::try_from(now_millis()?)
                .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            kind,
        };
        let kind = FrameType::Extension("norte_code.command".to_owned());
        let payload = serde_json::to_value(request)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let mut frame = Envelope::new(kind, payload, now_millis()?);
        let endpoint = match session {
            Some(session) => {
                frame.session_id = session.to_owned();
                Endpoint::SessionCommands { id: session }
            }
            // Catalogue writes, plugins, skills and MCP belong to the agent,
            // and the host serves them without inventing a session id.
            None => Endpoint::Commands,
        };
        let response = self
            .post_frame_at(endpoint, &frame)
            .await
            .map_err(|error| match error {
                ApiError::InvalidResponse(message) => ApiError::InvalidResponse(message),
                other => other,
            })?;
        if response.accepted {
            Ok(response)
        } else {
            Err(ApiError::InvalidResponse(response.error_message()))
        }
    }

    /// Uploads bytes into a session.
    ///
    /// # Errors
    ///
    /// Returns the transport error or a non-success status.
    pub async fn upload(
        &self,
        session: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<norte_code_wire::UploadReceipt, ApiError> {
        let request = self
            .http
            .post(self.url(&Endpoint::SessionUpload { id: session }))
            .header(reqwest::header::CONTENT_TYPE, mime_type)
            .body(bytes);
        let response = self
            .authorized(request)
            .send()
            .await
            .map_err(|error| request_error(&error))?;
        decode(response).await
    }

    /// The absolute URL of a session's event stream.
    #[must_use]
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::InvalidUrl`] when the client has no base URL, which
    /// construction prevents.
    pub fn events_url(&self, session: &str, agent: Option<&str>) -> String {
        self.url(&Endpoint::SessionEvents(SessionEventsRequest {
            session,
            agent,
        }))
    }

    async fn send(
        &self,
        endpoint: &Endpoint<'_>,
        body: Option<Value>,
    ) -> Result<reqwest::Response, ApiError> {
        let mut request = self.http.request(endpoint.method(), self.url(endpoint));
        let body = match (body, endpoint.body()?) {
            (Some(body), _) | (None, Some(body)) => Some(body),
            (None, None) => None,
        };
        if let Some(body) = body {
            request = request.json(&body);
        }
        self.authorized(request)
            .send()
            .await
            .map_err(|error| request_error(&error))
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.bearer_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
}

fn request_error(error: &reqwest::Error) -> ApiError {
    ApiError::Request(error.to_string())
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, ApiError> {
    let status = response.status();
    if !status.is_success() {
        return Err(status_error(response).await);
    }
    response
        .json::<T>()
        .await
        .map_err(|error| ApiError::InvalidResponse(error.to_string()))
}

async fn status_error(response: reqwest::Response) -> ApiError {
    let status = response.status().as_u16();
    let detail = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<RemoteErrorResponse>(&detail)
        .ok()
        .and_then(|body| body.error)
        .map(|error| error.message)
        .filter(|message| !message.trim().is_empty());
    match message {
        Some(message) => ApiError::InvalidResponse(message),
        None if detail.trim().is_empty() => ApiError::HttpStatus(status),
        None => ApiError::InvalidResponse(format!("HTTP {status}: {detail}")),
    }
}

#[derive(Deserialize)]
struct RemoteErrorResponse {
    #[serde(default)]
    error: Option<RemoteError>,
}

/// The current time in milliseconds since the epoch.
///
/// # Errors
///
/// Returns [`ApiError::InvalidResponse`] when the clock is outside the range a
/// frame timestamp can carry.
pub fn now_millis() -> Result<i64, ApiError> {
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

/// A session key for a named session.
#[must_use]
pub fn session_key(session: &str) -> SessionKey {
    SessionKey::new(session)
}

/// An agent reference for a named agent.
#[must_use]
pub fn agent_ref(id: &str) -> AgentRef {
    AgentRef {
        id: id.to_owned(),
        ..AgentRef::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_session_paths_with_the_id_encoded() {
        assert_eq!(
            Endpoint::Session { id: "a b" }.path(),
            "/raep/v1/sessions/a%20b"
        );
        assert_eq!(
            Endpoint::SessionCommands { id: "s" }.path(),
            "/raep/v1/sessions/s/commands"
        );
        assert_eq!(
            Endpoint::SessionEvents(SessionEventsRequest {
                session: "s",
                agent: Some("norte-code"),
            })
            .path(),
            "/raep/v1/sessions/s/events?create_if_missing=false&agent_id=norte-code"
        );
    }

    #[test]
    fn an_endpoint_with_a_request_carries_it_as_the_body() {
        let endpoint = Endpoint::CreateSession(CreateSessionRequest {
            agent_id: "norte-code",
            title: Some("hi"),
            agent_override: None,
        });
        let body = endpoint.body().expect("body").expect("some body");
        assert_eq!(body["agent_id"], "norte-code");
        assert_eq!(body["title"], "hi");

        assert!(Endpoint::Info.body().expect("body").is_none());
    }

    #[test]
    fn a_rejected_url_is_not_a_client() {
        assert!(RestClient::new("ftp://example.com", "").is_err());
        assert!(RestClient::new("http://example.com?x=1", "").is_err());
        assert!(RestClient::new("http://example.com", "token").is_ok());
    }
}
