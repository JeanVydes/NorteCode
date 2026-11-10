//! The REST client: one base URL, one token, one HTTP client.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use raep_types::{Envelope, FrameType};

use norte_code_wire::{ClientRequest, ClientRequestKind};

use super::endpoint::SessionEventsRequest;
use super::{ApiError, CommandResponse, Endpoint, RemoteError, now_millis};
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
