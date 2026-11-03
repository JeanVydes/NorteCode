use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime::domain::node::Node;
use crate::runtime::domain::session::{Phase, Session};
use norte::error::{Error, Result};
use norte::id::{NodeId, SessionId};

/// One atomic transition. A store applies all of it or none of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    /// The session the commit belongs to.
    pub session_id: SessionId,
    /// `None` creates the session. `Some(version)` must match the stored
    /// version or the store reports a conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<u64>,
    /// Parent first.
    pub nodes: Vec<Node>,
    /// The branch head after the commit; `None` keeps the current one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<NodeId>,
    /// The phase the session moves to.
    pub phase: Phase,
    /// The turn counter after the commit.
    pub turn: u64,
    /// Replaces the session metadata when set; `None` keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, Value>>,
}

impl Commit {
    /// Structural validation every store performs before writing anything.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for duplicate node ids or a head that is
    /// not part of the commit.
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
        if let Some(head) = self.head {
            let committed = seen.contains(&head);
            if !committed {
                return Err(Error::InvalidInput(format!(
                    "commit head {head} is not part of the commit"
                )));
            }
        }
        Ok(())
    }

    /// Applies the commit to a loaded session. Stores call this only after the
    /// version check and the write have succeeded.
    pub fn apply(self, session: &mut Session) {
        if let Some(metadata) = self.metadata {
            session.metadata = metadata;
        }
        session.head = self.head.or(session.head);
        session.phase = self.phase;
        session.turn = self.turn;
        session.version = session.version.saturating_add(1);
        session.updated_at = chrono::Utc::now();
    }
}
