//! Client adapters for the in-process service.

use std::sync::Arc;

use async_trait::async_trait;
use norte_code_client::{AgentClient, ClientError, EventStream, SessionSummary};
use norte_code_protocol::{ClientRequest, ClientRequestKind, ServerEventKind, SessionKey};

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

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ClientError> {
        let mut stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::ListSessions,
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))?;
        let mut sessions = Vec::new();
        while let Some(event) = futures_util::StreamExt::next(&mut stream).await {
            if let ServerEventKind::SessionList { sessions: listed } = event.kind {
                sessions.extend(listed.into_iter().map(|info| SessionSummary {
                    session_id: info.session.as_str().to_owned(),
                    title: info.title,
                }));
            } else if let ServerEventKind::Error { message, .. } = event.kind {
                return Err(ClientError::new(message));
            }
        }
        Ok(sessions)
    }

    async fn create_session(&self, title: Option<&str>) -> Result<SessionSummary, ClientError> {
        let mut stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::CreateSession {
                    title: title.map(str::to_owned),
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))?;
        while let Some(event) = futures_util::StreamExt::next(&mut stream).await {
            match event.kind {
                ServerEventKind::SessionCreated { title } => {
                    let session = event.session.ok_or_else(|| {
                        ClientError::new("the service did not return the new session")
                    })?;
                    return Ok(SessionSummary {
                        session_id: session.as_str().to_owned(),
                        title,
                    });
                }
                ServerEventKind::Error { message, .. } => return Err(ClientError::new(message)),
                _ => {}
            }
        }
        Err(ClientError::new(
            "the service did not return the new session",
        ))
    }

    async fn rename_session(&self, session: &str, title: &str) -> Result<(), ClientError> {
        let mut stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::RenameSession {
                    session: SessionKey::new(session),
                    title: title.to_owned(),
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))?;
        while let Some(event) = futures_util::StreamExt::next(&mut stream).await {
            if let ServerEventKind::Error { message, .. } = event.kind {
                return Err(ClientError::new(message));
            }
        }
        Ok(())
    }

    async fn delete_session(&self, session: &str) -> Result<(), ClientError> {
        let mut stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::DeleteSession {
                    session: SessionKey::new(session),
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))?;
        while let Some(event) = futures_util::StreamExt::next(&mut stream).await {
            if let ServerEventKind::Error { message, .. } = event.kind {
                return Err(ClientError::new(message));
            }
        }
        Ok(())
    }

    async fn send_message(
        &self,
        session: &str,
        text: &str,
        attachments: &[norte_code_protocol::Attachment],
    ) -> Result<EventStream, ClientError> {
        self.service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::SendMessage {
                    session: Some(SessionKey::new(session)),
                    text: text.to_owned(),
                    attachments: attachments.to_vec(),
                    budget: None,
                    reasoning: None,
                    replay: false,
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))
    }

    async fn cancel_turn(&self, session: &str) -> Result<(), ClientError> {
        let mut stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::CancelTurn {
                    session: SessionKey::new(session),
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))?;
        while let Some(event) = futures_util::StreamExt::next(&mut stream).await {
            if let ServerEventKind::Error { message, .. } = event.kind {
                return Err(ClientError::new(message));
            }
        }
        Ok(())
    }

    async fn set_model(
        &self,
        session: &str,
        provider: &str,
        model: &str,
    ) -> Result<EventStream, ClientError> {
        self.service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::SetModel {
                    session: SessionKey::new(session),
                    provider: provider.to_owned(),
                    model: model.to_owned(),
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))
    }

    async fn set_reasoning(
        &self,
        session: &str,
        reasoning: norte_code_protocol::Reasoning,
    ) -> Result<EventStream, ClientError> {
        self.service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::SetReasoning {
                    session: SessionKey::new(session),
                    scope: norte_code_protocol::ReasoningScope::Session,
                    reasoning,
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))
    }

    async fn set_budget(
        &self,
        session: &str,
        budget: norte_code_protocol::Budget,
    ) -> Result<EventStream, ClientError> {
        self.service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::SetBudget {
                    session: SessionKey::new(session),
                    budget,
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))
    }

    async fn resume(&self, session: &str) -> Result<EventStream, ClientError> {
        self.service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::Resume {
                    session: SessionKey::new(session),
                    replay: true,
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))
    }

    async fn fork_session(&self, session: &str) -> Result<SessionSummary, ClientError> {
        let mut stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::ForkSession {
                    session: SessionKey::new(session),
                    title: None,
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))?;
        while let Some(event) = futures_util::StreamExt::next(&mut stream).await {
            match event.kind {
                ServerEventKind::SessionForked { .. } => {
                    let session = event.session.ok_or_else(|| {
                        ClientError::new("the service did not return the forked session")
                    })?;
                    return Ok(SessionSummary {
                        session_id: session.as_str().to_owned(),
                        title: None,
                    });
                }
                ServerEventKind::Error { message, .. } => return Err(ClientError::new(message)),
                _ => {}
            }
        }
        Err(ClientError::new(
            "the service did not return the forked session",
        ))
    }

    async fn compact_session(&self, session: &str) -> Result<EventStream, ClientError> {
        self.service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::Compact {
                    session: SessionKey::new(session),
                },
            })
            .await
            .map_err(|error| ClientError::new(error.to_string()))
    }
}
