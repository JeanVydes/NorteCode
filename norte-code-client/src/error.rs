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
