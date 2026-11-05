//! Client adapters for the in-process service.

use std::sync::Arc;

use async_trait::async_trait;
use norte_code_client::{AgentClient, ClientError, EventStream};
use norte_code_protocol::ClientRequest;

use crate::Service;

/// An in-process implementation of the transport-neutral client contract.
///
/// This adapter is the migration seam: the CLI can use it today, while a
/// future RAEP adapter can implement the same contract without changing the
/// CLI's request and event handling.
pub struct InProcessClient {
    service: Arc<Service>,
}

impl InProcessClient {
    /// Wraps an existing service without taking ownership of its internals.
    pub fn new(service: Arc<Service>) -> Self {
        Self { service }
    }

    /// Returns the service used by this adapter.
    ///
    /// This is retained for local-only host features that still need service
    /// introspection during the incremental migration.
    #[must_use]
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }
}

#[async_trait]
impl AgentClient for InProcessClient {
    async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError> {
        self.service
            .submit(request)
            .await
            .map_err(|error| ClientError::new(error.to_string()))
    }
}

#[async_trait]
impl AgentClient for Service {
    async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError> {
        self.submit(request)
            .await
            .map_err(|error| ClientError::new(error.to_string()))
    }
}
