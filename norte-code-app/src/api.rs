//! The app's view of the shared client.
//!
//! Everything protocol-facing lives in `norte-code-client`: the endpoint
//! catalog, the REST client and the shapes it returns. This module only holds
//! what is specific to this UI: the capability vocabulary, the conversation
//! mapping and the platform event stream.

use crate::domain::{Capability, ConversationItem, HistoryKind, HistoryRole, MessageDelivery};

pub use norte_code_client::{
    ApiError, CatalogDetail, CostReport, HealthStatus, Liveness, McpCatalog, PluginChange,
    PluginSummary, ProtocolInfo, Readiness, RestClient, ServerInfo, SkillCatalog, ToolRule,
    now_millis,
};
use norte_code_wire::HistorySpeaker;

/// The client the pages hold.
pub type ApiClient = RestClient;

/// Reads the capabilities the app understands.
pub trait ServerInfoExt {
    /// The advertised capabilities, decoded into the app's vocabulary.
    fn decoded_capabilities(&self) -> Vec<Capability>;
}

impl ServerInfoExt for ServerInfo {
    fn decoded_capabilities(&self) -> Vec<Capability> {
        self.capabilities
            .iter()
            .map(|capability| Capability::from_wire(capability))
            .collect()
    }
}

/// Opens the event stream of a session.
///
/// # Errors
///
/// Returns [`ApiError`] when the stream cannot be opened.
pub async fn open_session_events(
    client: &RestClient,
    session: &str,
    agent: &str,
) -> Result<crate::sse::SessionEventStream, ApiError> {
    let url = client.events_url(session, Some(agent));
    crate::sse::SessionEventStream::connect(&url, client.bearer_token()).await
}

/// Folds a history page into the conversation items the transcript draws.
#[must_use]
pub fn conversation_history(items: &[norte_code_wire::HistoryItem]) -> Vec<ConversationItem> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| ConversationItem {
            id: format!("{}:{index}", item.id),
            turn_id: item.turn_id.clone(),
            role: match item.role {
                HistorySpeaker::User => HistoryRole::User,
                HistorySpeaker::Assistant => HistoryRole::Assistant,
                HistorySpeaker::Tool => HistoryRole::Tool,
                HistorySpeaker::SystemEvent => HistoryRole::SystemEvent,
            },
            kind: HistoryKind::from_wire(&item.kind),
            content: item.content.clone(),
            created_at: item.created_at,
            delivery: MessageDelivery::Sent,
            tool: None,
            usage: None,
        })
        .collect()
}
