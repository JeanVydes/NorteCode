use async_trait::async_trait;
use futures_core::Stream;
use norte_code_wire::QueueMode;
use norte_code_wire::{ClientRequest, ServerEvent};

pub use norte_code_wire::SessionSummary;
use std::pin::Pin;

use crate::error::ClientError;

/// A stream of events answering one client request.
pub type EventStream = Pin<Box<dyn Stream<Item = ServerEvent> + Send>>;

/// A transport-independent Norte Code client.
///
/// Implementations may submit requests in-process, over RAEP WebSocket, or
/// through another transport. The stream remains the same protocol-level
/// event stream in every case.
///
/// Session management goes through the RAEP standard resource
/// (`/raep/v1/sessions`), turn input through `UserInput`, cancellation
/// through `CancelTurn`, and configuration through `SessionConfigure`:
/// those are operations the spec owns, so they are typed methods here, not
/// extension requests. Everything the spec does not cover (plugins, skills,
/// MCP, approvals, observability) still goes through `submit`.
#[async_trait]
pub trait AgentClient: Send + Sync {
    /// Submits one request and returns its event stream.
    async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError>;

    /// Lists the sessions the server holds, newest content first.
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ClientError>;

    /// Creates a session and returns its summary.
    async fn create_session(&self, title: Option<&str>) -> Result<SessionSummary, ClientError>;

    /// Renames a session.
    async fn rename_session(&self, session: &str, title: &str) -> Result<(), ClientError>;

    /// Deletes a session and everything keyed by it.
    async fn delete_session(&self, session: &str) -> Result<(), ClientError>;

    /// Sends one user message as a standard `UserInput` frame and streams
    /// the turn's events.
    async fn send_message(
        &self,
        session: &str,
        text: &str,
        attachments: &[norte_code_wire::Attachment],
    ) -> Result<EventStream, ClientError>;

    /// Sends one user message through the standard `UserInput` frame and
    /// streams the resulting events. The wire policy comes from the
    /// client's `queue_mode`: `reject` starts a new turn, `queue` steers,
    /// and `interrupt` cancels-then-steers on the server.
    async fn steer_message(
        &self,
        session: &str,
        text: &str,
        queue_mode: QueueMode,
    ) -> Result<EventStream, ClientError>;

    /// Cancels the session's running turn through `CancelTurn`.
    async fn cancel_turn(&self, session: &str) -> Result<(), ClientError>;

    /// Changes the session's model through `SessionConfigure`.
    async fn set_model(
        &self,
        session: &str,
        provider: &str,
        model: &str,
    ) -> Result<EventStream, ClientError>;

    /// Changes the session's reasoning through `SessionConfigure`.
    async fn set_reasoning(
        &self,
        session: &str,
        reasoning: norte_code_wire::Reasoning,
    ) -> Result<EventStream, ClientError>;

    /// Sets the session's budget through `SessionConfigure`.
    async fn set_budget(
        &self,
        session: &str,
        budget: norte_code_wire::Budget,
    ) -> Result<EventStream, ClientError>;

    /// Replays the session's history through `SessionAttach` with
    /// `resume_from_seq`, instead of a `Resume` command.
    async fn resume(&self, session: &str) -> Result<EventStream, ClientError>;

    /// Forks a session through `SessionFork`.
    async fn fork_session(&self, session: &str) -> Result<SessionSummary, ClientError>;

    /// Compacts a session through `SessionCompact`.
    async fn compact_session(&self, session: &str) -> Result<EventStream, ClientError>;

    /// Exports a session through `SessionExport`.
    async fn export_session(&self, session: &str) -> Result<EventStream, ClientError>;

    /// Imports a session through `SessionImport`.
    async fn import_session(
        &self,
        session: &str,
        bundle: serde_json::Value,
    ) -> Result<EventStream, ClientError>;
}

/// Converts a session summary into the protocol's session info shape.
#[must_use]
pub fn into_session_info(summary: SessionSummary) -> norte_code_wire::SessionInfo {
    norte_code_wire::SessionInfo {
        session: norte_code_wire::SessionKey::new(summary.session_id),
        title: summary.title,
        provider: None,
        model: None,
        created_at: None,
        phase: norte_code_wire::Phase::Idle,
        reasoning: None,
        parent: None,
    }
}
