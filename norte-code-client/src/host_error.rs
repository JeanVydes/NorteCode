//! The refusal the host writes when it answers a request with an error.
//!
//! The host answers a rejected command with a RAEP error envelope, not with
//! the body the client would like to read. One place reads that envelope, so
//! every client learns why the host said no instead of showing raw JSON.

use serde::Deserialize;

/// The refusal the host reports.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct HostError {
    /// The machine-readable category.
    #[serde(default)]
    pub code: String,
    /// The message written for the person reading it.
    #[serde(default)]
    pub message: String,
    /// Whether the host considers the request worth retrying.
    #[serde(default)]
    pub retryable: bool,
}

impl HostError {
    /// Reads the refusal out of a response body.
    ///
    /// Returns [`None`] when the body is not an error envelope, so the caller
    /// can fall back to whatever else it knows about the answer.
    #[must_use]
    pub fn parse(body: &str) -> Option<Self> {
        serde_json::from_str::<ErrorEnvelope>(body)
            .ok()
            .and_then(|envelope| envelope.payload)
    }

    /// Whether the host says the session is not attached.
    ///
    /// The host answers that way when a session was detached while the client
    /// still believed it held the attachment, which is a recoverable state:
    /// the client can attach again and send the request once more.
    #[must_use]
    pub fn is_session_not_attached(&self) -> bool {
        self.code == "INVALID_FRAME" && self.message.contains("session is not attached")
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.code.is_empty() {
            return formatter.write_str(&self.message);
        }
        write!(formatter, "{} ({})", self.message, self.code)
    }
}

/// The error envelope the host writes: a RAEP frame whose payload is the
/// refusal.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    #[serde(default)]
    payload: Option<HostError>,
}

#[cfg(test)]
#[path = "host_error_tests.rs"]
mod tests;
