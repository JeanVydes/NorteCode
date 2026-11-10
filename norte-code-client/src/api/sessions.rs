use super::endpoint::{CreateSessionRequest, RenameSessionRequest};
use super::types::{ChildSessionListResponse, SessionListResponse};
use super::{ApiError, Endpoint, RestClient};

impl RestClient {
    /// The sessions the principal can see.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn sessions(&self) -> Result<Vec<norte_code_wire::SessionSummary>, ApiError> {
        Ok(self
            .execute::<SessionListResponse>(Endpoint::Sessions)
            .await?
            .sessions)
    }

    /// The children of a session.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn child_sessions(
        &self,
        session: &str,
    ) -> Result<Vec<norte_code_wire::ChildSessionSummary>, ApiError> {
        Ok(self
            .execute::<ChildSessionListResponse>(Endpoint::ChildSessions { id: session })
            .await?
            .children)
    }

    /// Creates a session, optionally pinned to a model from its first turn.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn create_session(
        &self,
        agent_id: &str,
        title: Option<&str>,
        model: Option<(&str, &str)>,
    ) -> Result<norte_code_wire::SessionSummary, ApiError> {
        let agent_override = model.map(|(provider, id)| super::AgentOverride {
            model: super::AgentModelRef { id },
            provider,
        });
        self.execute(Endpoint::CreateSession(CreateSessionRequest {
            agent_id,
            title,
            agent_override,
        }))
        .await
    }

    /// Renames a session.
    ///
    /// # Errors
    ///
    /// Returns the transport error or a non-success status.
    pub async fn rename_session(&self, session: &str, title: &str) -> Result<(), ApiError> {
        self.execute_empty(Endpoint::RenameSession {
            id: session,
            request: RenameSessionRequest { title },
        })
        .await
    }

    /// Deletes a session and everything keyed by it.
    ///
    /// # Errors
    ///
    /// Returns the transport error or a non-success status.
    pub async fn delete_session(&self, session: &str) -> Result<(), ApiError> {
        self.execute_empty(Endpoint::DeleteSession { id: session })
            .await
    }
}
