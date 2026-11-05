//! Transport-neutral client contract for Norte Code.
//!
//! The client protocol is deliberately expressed in terms of
//! `norte-code-protocol` requests and events. An in-process adapter and a
//! remote RAEP adapter can therefore be used by the same CLI without moving
//! the agent loop into the client.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use futures_core::Stream;
use norte_code_protocol::{ClientRequest, ServerEvent};
use raep_types::{Envelope, FrameType};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::Value;
use std::pin::Pin;
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
    default_session: Option<String>,
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
        let bearer_token = bearer_token
            .map(|token| {
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|error| ClientError::new(format!("invalid bearer token: {error}")))
            })
            .transpose()?;
        Ok(Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            bearer_token,
            default_session,
        })
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
        Ok(Box::pin(iter(events)))
    }
}

#[async_trait]
impl AgentClient for RemoteClient {
    async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError> {
        self.submit_http(request).await
    }
}

fn server_event_from_frame(frame: &Value) -> Result<ServerEvent, ClientError> {
    frame
        .get("ext")
        .and_then(|ext| ext.get("norte_code"))
        .cloned()
        .ok_or_else(|| {
            ClientError::new(
                "RAEP frame has no norte_code compatibility event; \
                 canonical event decoding is not implemented for this client",
            )
        })
        .and_then(|event| {
            serde_json::from_value(event)
                .map_err(|error| ClientError::new(format!("invalid NorteCode event: {error}")))
        })
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
        | ClientRequestKind::Resume { session }
        | ClientRequestKind::AnswerQuestion { session, .. }
        | ClientRequestKind::SetModel { session, .. }
        | ClientRequestKind::SetReasoning { session, .. }
        | ClientRequestKind::SetBudget { session, .. }
        | ClientRequestKind::Compact { session } => Some(session),
        ClientRequestKind::SendMessage { session, .. }
        | ClientRequestKind::CostReport { session } => session.as_ref(),
        ClientRequestKind::CreateSession { .. }
        | ClientRequestKind::ListSessions
        | ClientRequestKind::ListModels
        | ClientRequestKind::RefreshCatalog
        | ClientRequestKind::ListSkills
        | ClientRequestKind::RefreshSkills => None,
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
}
