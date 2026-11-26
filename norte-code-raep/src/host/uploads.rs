//! Upload indexing: what the host keeps about an uploaded file.

use async_trait::async_trait;
use norte_code_wire::{ArtifactId, ArtifactOrigin, SessionArtifact};

use std::collections::HashMap;

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

/// An upload receipt returned by the session-scoped uploads endpoint.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct UploadReceipt {
    /// Opaque upload identifier.
    pub upload_id: String,
    /// Stored media type.
    pub mime_type: String,
    /// Number of stored bytes.
    pub bytes: u64,
    /// SHA-256 digest in lowercase hexadecimal.
    pub sha256: String,
    /// Expiration time in Unix milliseconds.
    pub expires_at: i64,
}
/// Metadata for a session-scoped upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadRecord {
    /// Opaque upload identifier returned to the client.
    pub id: String,
    /// Owning session.
    pub session_id: String,
    /// Owning authenticated principal, when authentication is enabled.
    pub owner: Option<String>,
    /// Content-addressed blob identifier in the Norte store.
    pub blob_id: String,
    /// Validated MIME type.
    pub mime_type: String,
    /// Number of uploaded bytes.
    pub bytes: u64,
    /// SHA-256 digest in lowercase hexadecimal.
    pub sha256: String,
    /// Expiration time in Unix milliseconds; referenced uploads do not expire.
    pub expires_at: i64,
    /// Whether a turn has referenced this upload.
    pub referenced: bool,
    /// Producer information for the session inventory.
    pub details: Option<ArtifactDetails>,
}
/// Descriptive information kept with an indexed artifact.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ArtifactDetails {
    /// Where this reference came from.
    pub origin: ArtifactOrigin,
    /// The producer-supplied name.
    pub name: Option<String>,
    /// When the host accepted the bytes.
    pub created_at: i64,
}

impl UploadRecord {
    pub(super) fn summary(&self) -> SessionArtifact {
        SessionArtifact {
            id: ArtifactId::new(self.id.clone()),
            origin: self.details.as_ref().map_or_else(
                || {
                    if self.id.starts_with("out_") {
                        ArtifactOrigin::AssistantOutput {
                            turn: None,
                            part_index: None,
                        }
                    } else {
                        ArtifactOrigin::UserUpload
                    }
                },
                |details| details.origin.clone(),
            ),
            name: self
                .details
                .as_ref()
                .and_then(|details| details.name.clone()),
            created_at: self.details.as_ref().map(|details| details.created_at),
            blob_id: self.blob_id.clone(),
            mime_type: self.mime_type.clone(),
            bytes: self.bytes,
        }
    }

    pub(super) fn visible(&self, session: &str, owner: Option<&str>) -> bool {
        self.session_id == session
            && self.owner.as_deref() == owner
            && (self.referenced || self.expires_at > now_millis())
    }
}
/// Durable or in-memory index for session-scoped upload metadata.
#[async_trait]
pub trait UploadIndex: Send + Sync {
    /// Inserts a new upload record.
    async fn insert(&self, record: UploadRecord) -> Result<(), HostError>;

    /// Writes the blob and its index entry.
    async fn store(
        &self,
        artifact_store: &dyn ArtifactStore,
        mut record: UploadRecord,
        bytes: &[u8],
    ) -> Result<(), HostError> {
        record.blob_id = artifact_store.put(&record.mime_type, bytes).await?.id;
        self.insert(record).await
    }

    /// Persists the principal bound to a session.
    async fn claim_session(&self, _session: &str, _owner: &str) -> Result<(), HostError> {
        Ok(())
    }

    /// Atomically stores a public endpoint's blob and its authenticated owner when supported.
    async fn store_global(
        &self,
        _mime: &str,
        _bytes: &[u8],
        _owner: &str,
    ) -> Result<Option<ArtifactRef>, HostError> {
        Ok(None)
    }

    /// Checks a durable ownership claim, when available.
    async fn owns_global(&self, _id: &str, _owner: &str) -> Result<bool, HostError> {
        Ok(false)
    }

    /// Reads the accessible entries for a session.
    async fn for_session(
        &self,
        session: &str,
        owner: Option<&str>,
    ) -> Result<Vec<UploadRecord>, HostError>;

    /// Removes a session's entries from an index managed independently of its graph.
    async fn delete_session(&self, session: &str) -> Result<(), HostError>;

    /// Looks up an upload by its opaque identifier.
    async fn get(&self, id: &str) -> Result<Option<UploadRecord>, HostError>;

    /// Keeps uploads referenced by an accepted turn for the session lifetime.
    async fn mark_referenced(
        &self,
        session_id: &str,
        owner: Option<&str>,
        ids: &[String],
    ) -> Result<(), HostError>;

    /// Removes every persisted upload receipt.
    async fn clear(&self) -> Result<(), HostError>;
}
/// In-memory upload index, useful for embedded hosts and tests.
#[derive(Default)]
pub struct MemoryUploadIndex {
    uploads: Mutex<HashMap<String, UploadRecord>>,
}
#[async_trait]
impl UploadIndex for MemoryUploadIndex {
    async fn insert(&self, record: UploadRecord) -> Result<(), HostError> {
        self.uploads.lock().await.insert(record.id.clone(), record);
        Ok(())
    }

    async fn for_session(
        &self,
        session: &str,
        owner: Option<&str>,
    ) -> Result<Vec<UploadRecord>, HostError> {
        Ok(self
            .uploads
            .lock()
            .await
            .values()
            .filter(|record| record.visible(session, owner))
            .cloned()
            .collect())
    }

    async fn delete_session(&self, session: &str) -> Result<(), HostError> {
        self.uploads
            .lock()
            .await
            .retain(|_, record| record.session_id != session);
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<UploadRecord>, HostError> {
        Ok(self.uploads.lock().await.get(id).cloned())
    }

    async fn mark_referenced(
        &self,
        session_id: &str,
        owner: Option<&str>,
        ids: &[String],
    ) -> Result<(), HostError> {
        let mut uploads = self.uploads.lock().await;
        for id in ids {
            let record = uploads
                .get_mut(id)
                .filter(|record| {
                    record.session_id == session_id && record.owner.as_deref() == owner
                })
                .ok_or_else(|| HostError::Artifact("upload not found".to_owned()))?;
            record.referenced = true;
        }
        Ok(())
    }

    async fn clear(&self) -> Result<(), HostError> {
        self.uploads.lock().await.clear();
        Ok(())
    }
}
#[path = "uploads_sqlite.rs"]
mod sqlite;
pub use sqlite::SqliteUploadIndex;
