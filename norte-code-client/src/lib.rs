//! Transport-neutral client contract for Norte Code.
//!
//! The client protocol is deliberately expressed in terms of
//! `norte-code-protocol` requests and events. An in-process adapter and a
//! remote RAEP adapter can therefore be used by the same CLI without moving
//! the agent loop into the client.

#![forbid(unsafe_code)]

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use norte_code_protocol::{ClientRequest, ServerEvent};

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
