//! The REST surface of a Norte Code host, as one closed endpoint catalog.
//!
//! Every client — the app today, any other tomorrow — builds requests through
//! [`Endpoint`] and never spells a path or owns a request shape. The shapes
//! that cross the wire live in `norte-code-wire`; this module binds them to a
//! method and a path and executes them.

mod catalog;
mod client;
mod conversation;
mod conversation_commands;
mod cost;
mod discovery;
mod endpoint;
mod history;
mod mcp;
mod plugins;
mod sessions;
mod shared;
mod skills;
mod types;

pub use client::RestClient;
pub use conversation_commands::CLIENT_NAME;
pub use endpoint::{
    AgentModelRef, AgentOverride, CreateSessionRequest, Endpoint, RenameSessionRequest,
    SessionEventsRequest, TelemetryQuery,
};
pub use types::*;

use raep_types::Envelope;
use serde::Deserialize;
use serde_json::Value;

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
    fn a_base_path_is_preserved() {
        let client = RestClient::new("https://example.test/raep-proxy/", "").expect("client");
        assert_eq!(
            client.url(&Endpoint::SessionCommands { id: "s" }),
            "https://example.test/raep-proxy/raep/v1/sessions/s/commands"
        );
    }

    #[test]
    fn a_rejected_url_is_not_a_client() {
        assert!(RestClient::new("ftp://example.com", "").is_err());
        assert!(RestClient::new("http://example.com?x=1", "").is_err());
        assert!(RestClient::new("http://example.com", "token").is_ok());
    }
}
