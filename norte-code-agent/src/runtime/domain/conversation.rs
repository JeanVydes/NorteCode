use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime::Phase;
use norte::domain::message::Message;
use norte::error::Result;
use norte::id::SessionId;
use norte::model::TurnOptions;

/// Schema version of the [`Conversation`] envelope.
///
/// Readers reject an envelope whose version they do not understand instead of
/// guessing (FR-EXP-3).
pub const CONVERSATION_SCHEMA_VERSION: u16 = 1;

/// A serializable, self-contained conversation: the session's durable identity
/// and its ordered message history (FR-EXP-1).
///
/// It carries no store-internal detail — node ids, versions and timestamps are
/// the store's bookkeeping, not conversation content (FR-GRAPH-4). This is the
/// portable form of a chat: export it, keep or ship it as JSON, import it back
/// elsewhere, and the next generation reproduces the exact same wire bytes.
///
/// # Cache safety
///
/// The session id is part of the envelope on purpose. Adapters that pass a
/// session-derived `prompt_cache_key` (`OpenAI`) derive it from this id, so a
/// conversation re-imported under the same id keeps the same cache key. Because
/// reconstruction is deterministic and append-only (FR-RECON-1, FR-RECON-2) and
/// the messages round-trip losslessly, the exported prefix is byte-identical to
/// what was sent before export. Importing under a *different* id is an explicit
/// fork and will miss the provider cache.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    /// Envelope schema version, for forward-compatible readers.
    pub schema_version: u16,
    /// The session this conversation belongs to, preserved across import.
    pub session: SessionId,
    /// Durable phase at export time, restored on import.
    pub phase: Phase,
    /// Turn counter at export time, restored on import.
    pub turn: u64,
    /// Host metadata attached to the session.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    /// The effective request options of the last turn, when the exporting engine
    /// knows them (FR-EXP-7). Restored on import as the session’s defaults so a
    /// later resume reproduces the complete request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<TurnOptions>,
    /// The ordered message history, oldest first. This is the array form.
    pub messages: Vec<Message>,
}

impl Conversation {
    /// Serializes to canonical, compact JSON.
    ///
    /// The bytes are stable: the same conversation always produces the same
    /// string, and `Conversation::from_json(&conversation.to_json()?)? ==
    /// conversation` (FR-EXP-2). Maps are `BTreeMap` and object keys are sorted,
    /// so serialization never depends on iteration order.
    ///
    /// # Errors
    ///
    /// Returns a codec error if the conversation cannot be serialized.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Parses a conversation written by [`to_json`](Conversation::to_json).
    ///
    /// # Errors
    ///
    /// Returns a codec error if the JSON is malformed or the schema version is
    /// not understood.
    pub fn from_json(json: &str) -> Result<Self> {
        let conversation: Self = serde_json::from_str(json)?;
        if conversation.schema_version != CONVERSATION_SCHEMA_VERSION {
            return Err(norte::error::Error::SchemaVersion {
                kind: "conversation".to_owned(),
                found: conversation.schema_version,
            });
        }
        Ok(conversation)
    }

    /// The message history, oldest first.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The session id this conversation is bound to.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session
    }
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
