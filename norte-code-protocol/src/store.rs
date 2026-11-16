#![allow(missing_docs)]
//! The durable-state contracts used by Norte Code's own runtime.
//!
//! This module replaces the storage contracts that used to come from
//! `norte::harness`. They live here so the runtime no longer depends on
//! that crate.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use norte::{Error, Message, NodeId, Result, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

/// An immutable node in the conversation graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<NodeId>,
    pub kind: NodeKind,
    pub message: Message,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    #[serde(default = "node_schema_version")]
    pub schema_version: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NodeKind {
    Message,
}

#[must_use]
pub const fn node_schema_version() -> u16 {
    1
}

impl Node {
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

    #[must_use]
    pub fn message(message: Message, parent: Option<NodeId>) -> Self {
        Self::new(NodeKind::Message, message, parent)
    }
}

/// Durable conversation state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<NodeId>,
    pub phase: norte_code_wire::Phase,
    pub version: u64,
    pub turn: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    #[must_use]
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            id: SessionId::new(),
            head: None,
            phase: norte_code_wire::Phase::Idle,
            version: 0,
            turn: 0,
            metadata: BTreeMap::new(),
            created_at: now,
            updated_at: now,
        }
    }
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.phase.is_idle()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// One atomic transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<u64>,
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<NodeId>,
    pub phase: norte_code_wire::Phase,
    pub turn: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, Value>>,
}

impl Commit {
    /// Validates commit.
    /// # Errors
    /// Returns invalid input.
    pub fn validate(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for node in &self.nodes {
            if !seen.insert(node.id) {
                return Err(Error::InvalidInput(format!(
                    "commit contains duplicate node {}",
                    node.id
                )));
            }
        }
        if let Some(head) = self.head
            && !seen.contains(&head)
        {
            return Err(Error::InvalidInput(format!(
                "commit head {head} is not part of the commit"
            )));
        }
        Ok(())
    }

    pub fn apply(self, session: &mut Session) {
        if let Some(metadata) = self.metadata {
            session.metadata = metadata;
        }
        session.head = self.head.or(session.head);
        session.phase = self.phase;
        session.turn = self.turn;
        session.version = session.version.saturating_add(1);
        session.updated_at = Utc::now();
    }
}

/// Persistence port.
#[async_trait]
pub trait Store: Send + Sync {
    async fn commit(&self, commit: Commit) -> Result<Session>;
    async fn load(&self, session: SessionId) -> Result<Session>;
    async fn node(&self, id: NodeId) -> Result<Node>;
    async fn ancestors(&self, id: NodeId) -> Result<Vec<NodeId>>;
    async fn children(&self, id: NodeId) -> Result<Vec<NodeId>>;
    async fn history(&self, head: Option<NodeId>) -> Result<Vec<norte::Message>> {
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

pub use norte::blob::InMemoryBlobStore;
/// Re-exported blob contracts from Norte core.
pub use norte::{BlobId, BlobStore};

#[derive(Default)]
struct Tables {
    sessions: HashMap<SessionId, Session>,
    nodes: HashMap<NodeId, Node>,
    children: HashMap<NodeId, Vec<NodeId>>,
}

/// Complete in-memory store.
#[derive(Default)]
pub struct InMemoryStore {
    tables: RwLock<Tables>,
}

impl InMemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Store for InMemoryStore {
    async fn commit(&self, commit: Commit) -> Result<Session> {
        commit.validate()?;
        let session_id = commit.session_id;
        let mut tables = self.tables.write().await;

        let mut session = match commit.expected_version {
            None => {
                if let Some(existing) = tables.sessions.get(&session_id) {
                    return Err(Error::StoreConflict {
                        session: session_id,
                        expected: None,
                        actual: existing.version,
                    });
                }
                let mut session = Session::new();
                session.id = session_id;
                session
            }
            Some(expected) => {
                let existing = tables
                    .sessions
                    .get(&session_id)
                    .cloned()
                    .ok_or(Error::SessionNotFound(session_id))?;
                if existing.version != expected {
                    return Err(Error::StoreConflict {
                        session: session_id,
                        expected: Some(expected),
                        actual: existing.version,
                    });
                }
                existing
            }
        };

        for node in &commit.nodes {
            if tables.nodes.contains_key(&node.id) {
                return Err(Error::InvalidInput(format!(
                    "node {} already exists",
                    node.id
                )));
            }
        }
        for node in &commit.nodes {
            tables.children.entry(node.id).or_default();
            if let Some(parent) = node.parent {
                tables.children.entry(parent).or_default().push(node.id);
            }
        }
        for node in &commit.nodes {
            tables.nodes.insert(node.id, node.clone());
        }

        commit.apply(&mut session);
        tables.sessions.insert(session.id, session.clone());
        Ok(session)
    }

    async fn load(&self, session: SessionId) -> Result<Session> {
        self.tables
            .read()
            .await
            .sessions
            .get(&session)
            .cloned()
            .ok_or(Error::SessionNotFound(session))
    }

    async fn node(&self, id: NodeId) -> Result<Node> {
        self.tables
            .read()
            .await
            .nodes
            .get(&id)
            .cloned()
            .ok_or(Error::NodeNotFound(id))
    }

    async fn ancestors(&self, id: NodeId) -> Result<Vec<NodeId>> {
        let tables = self.tables.read().await;
        if !tables.nodes.contains_key(&id) {
            return Err(Error::NodeNotFound(id));
        }
        let mut chain = Vec::new();
        let mut cursor = Some(id);
        while let Some(current) = cursor {
            chain.push(current);
            cursor = tables.nodes.get(&current).and_then(|node| node.parent);
        }
        chain.reverse();
        Ok(chain)
    }

    async fn children(&self, id: NodeId) -> Result<Vec<NodeId>> {
        Ok(self
            .tables
            .read()
            .await
            .children
            .get(&id)
            .cloned()
            .unwrap_or_default())
    }
}
