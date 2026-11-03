use crate::runtime::domain::commit::Commit;
use crate::runtime::domain::node::Node;
use crate::runtime::domain::session::Session;
use norte::domain::message::Message;
use norte::error::Result;
use norte::id::{BlobId, NodeId, SessionId};

/// Persistence port for sessions, the conversation graph and approvals.
///
/// Implementations must make one `commit` atomic and enforce optimistic
/// concurrency through `expected_version`.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    /// Applies one commit. Returns the new session state on success and
    /// [`norte::Error::StoreConflict`] when the version does not match.
    async fn commit(&self, commit: Commit) -> Result<Session>;

    /// Loads a session, including its head and phase.
    async fn load(&self, session: SessionId) -> Result<Session>;

    /// Loads one node by id.
    async fn node(&self, id: NodeId) -> Result<Node>;

    /// Ancestor chain from the root down to `id`, inclusive.
    async fn ancestors(&self, id: NodeId) -> Result<Vec<NodeId>>;

    /// The direct children of `id`, in creation order.
    async fn children(&self, id: NodeId) -> Result<Vec<NodeId>>;

    /// The conversation the model sees: every ancestor message, root first.
    async fn history(&self, head: Option<NodeId>) -> Result<Vec<Message>> {
        let Some(head) = head else {
            return Ok(Vec::new());
        };
        let mut messages = Vec::new();
        for id in self.ancestors(head).await? {
            messages.push(self.node(id).await?.message);
        }
        Ok(messages)
    }
}

/// Content-addressed blob storage for media that must not live in graph nodes.
#[async_trait::async_trait]
pub trait BlobStore: Send + Sync {
    /// Idempotent: the same bytes always produce the same id.
    async fn put(&self, bytes: &[u8]) -> Result<BlobId>;

    /// The bytes stored under `id`, or a typed error when it is absent.
    async fn get(&self, id: BlobId) -> Result<Vec<u8>>;

    /// Whether `id` is present, without reading its bytes.
    async fn contains(&self, id: BlobId) -> Result<bool>;
}
