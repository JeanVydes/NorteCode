use crate::host_error::HostError;

/// An error raised before a request can produce an event stream.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{message}")]
pub struct ClientError {
    message: String,
    refusal: Option<HostError>,
}

impl ClientError {
    /// Creates a client error with a user-facing message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            refusal: None,
        }
    }

    /// Creates a client error from a refusal the host reported.
    #[must_use]
    pub fn from_host(refusal: HostError) -> Self {
        Self {
            message: refusal.to_string(),
            refusal: Some(refusal),
        }
    }

    /// Returns the error message without formatting it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The refusal the host reported, when it reported one.
    #[must_use]
    pub fn refusal(&self) -> Option<&HostError> {
        self.refusal.as_ref()
    }

    /// Whether the host says the session is not attached.
    ///
    /// The host answers that way when the session was detached while the
    /// client still believed it held the attachment. Attaching again and
    /// sending the request once more recovers from it.
    #[must_use]
    pub fn is_session_not_attached(&self) -> bool {
        self.refusal
            .as_ref()
            .is_some_and(HostError::is_session_not_attached)
    }
}
