use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use norte::domain::message::Message;
use norte::id::NodeId;

/// Node kinds the core understands. Persisted kinds evolve additively; a reader
/// that meets an unknown kind reports a typed error instead of guessing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NodeKind {
    /// A message node.
    Message,
}

/// The schema version written on new nodes.
#[must_use]
pub const fn node_schema_version() -> u16 {
    1
}

/// An immutable node in the conversation graph.
///
/// Nodes belong to no session: sessions share ancestors, and branching only
/// adds nodes and moves a pointer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// The node's identifier.
    pub id: NodeId,
    /// The node this one follows, or `None` at the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<NodeId>,
    /// What the node holds.
    pub kind: NodeKind,
    /// The message the node carries.
    pub message: Message,
    /// When the node was created.
    pub created_at: DateTime<Utc>,
    /// Node-local extras, which the engine never interprets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    /// The version of the node's persisted shape.
    #[serde(default = "node_schema_version")]
    pub schema_version: u16,
}

impl Node {
    /// A node of `kind` carrying `message`, after `parent`.
    #[must_use]
    pub fn new(kind: NodeKind, message: Message, parent: Option<NodeId>) -> Self {
        Self {
            id: NodeId::new(),
            parent,
            kind,
            message,
            created_at: Utc::now(),
            metadata: BTreeMap::new(),
            schema_version: node_schema_version(),
        }
    }

    /// A message node after `parent`.
    #[must_use]
    pub fn message(message: Message, parent: Option<NodeId>) -> Self {
        Self::new(NodeKind::Message, message, parent)
    }
}
