//! Upload indexing: what the host keeps about an uploaded file.

use async_trait::async_trait;

use std::collections::HashMap;
use std::path::PathBuf;

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
}
/// Durable or in-memory index for session-scoped upload metadata.
#[async_trait]
pub trait UploadIndex: Send + Sync {
    /// Inserts a new upload record.
    async fn insert(&self, record: UploadRecord) -> Result<(), HostError>;

    /// Looks up an upload by its opaque identifier.
    async fn get(&self, id: &str) -> Result<Option<UploadRecord>, HostError>;

    /// Keeps uploads referenced by an accepted turn for the session lifetime.
    async fn mark_referenced(
        &self,
        session_id: &str,
        owner: Option<&str>,
        ids: &[String],
    ) -> Result<(), HostError>;
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
}
/// SQLite-backed upload metadata that survives server restarts.
pub struct SqliteUploadIndex {
    path: PathBuf,
}
impl SqliteUploadIndex {
    /// Opens or creates the upload metadata table.
    ///
    /// # Errors
    ///
    /// Returns an error when SQLite cannot open the database or create its
    /// schema.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, HostError> {
        let path = path.into();
        let connection = rusqlite::Connection::open(&path)
            .map_err(|error| HostError::Artifact(error.to_string()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| HostError::Artifact(error.to_string()))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS raep_uploads (
                    upload_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    owner TEXT,
                    blob_id TEXT NOT NULL,
                    mime_type TEXT NOT NULL,
                    bytes INTEGER NOT NULL,
                    sha256 TEXT NOT NULL,
                    expires_at INTEGER NOT NULL,
                    referenced INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_raep_uploads_session
                    ON raep_uploads(session_id, upload_id);",
            )
            .map_err(|error| HostError::Artifact(error.to_string()))?;
        Ok(Self { path })
    }
}
#[async_trait]
impl UploadIndex for SqliteUploadIndex {
    async fn insert(&self, record: UploadRecord) -> Result<(), HostError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .execute(
                    "DELETE FROM raep_uploads WHERE referenced = 0 AND expires_at <= ?1",
                    rusqlite::params![now_millis()],
                )
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .execute(
                    "INSERT INTO raep_uploads
                     (upload_id, session_id, owner, blob_id, mime_type, bytes, sha256, expires_at, referenced)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        record.id,
                        record.session_id,
                        record.owner,
                        record.blob_id,
                        record.mime_type,
                        i64::try_from(record.bytes)
                            .map_err(|_| HostError::Artifact("upload is too large".to_owned()))?,
                        record.sha256,
                        record.expires_at,
                        record.referenced,
                    ],
                )
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|error| HostError::Artifact(error.to_string()))?
    }

    async fn get(&self, id: &str) -> Result<Option<UploadRecord>, HostError> {
        let path = self.path.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            use rusqlite::OptionalExtension;
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .query_row(
                    "SELECT upload_id, session_id, owner, blob_id, mime_type, bytes, sha256, expires_at, referenced
                     FROM raep_uploads WHERE upload_id = ?1",
                    rusqlite::params![id],
                    |row| {
                        let bytes: i64 = row.get(5)?;
                        let bytes = u64::try_from(bytes).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                5,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?;
                        Ok(UploadRecord {
                            id: row.get(0)?,
                            session_id: row.get(1)?,
                            owner: row.get(2)?,
                            blob_id: row.get(3)?,
                            mime_type: row.get(4)?,
                            bytes,
                            sha256: row.get(6)?,
                            expires_at: row.get(7)?,
                            referenced: row.get(8)?,
                        })
                    },
                )
                .optional()
                .map_err(|error| HostError::Artifact(error.to_string()))
        })
        .await
        .map_err(|error| HostError::Artifact(error.to_string()))?
    }

    async fn mark_referenced(
        &self,
        session_id: &str,
        owner: Option<&str>,
        ids: &[String],
    ) -> Result<(), HostError> {
        let path = self.path.clone();
        let session_id = session_id.to_owned();
        let owner = owner.map(str::to_owned);
        let ids = ids.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            let transaction = connection
                .transaction()
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            for id in ids {
                let updated = transaction
                    .execute(
                        "UPDATE raep_uploads SET referenced = 1
                         WHERE upload_id = ?1 AND session_id = ?2 AND owner IS ?3
                           AND (referenced = 1 OR expires_at > ?4)",
                        rusqlite::params![id, session_id, owner, now_millis()],
                    )
                    .map_err(|error| HostError::Artifact(error.to_string()))?;
                if updated != 1 {
                    return Err(HostError::Artifact("upload not found".to_owned()));
                }
            }
            transaction
                .commit()
                .map_err(|error| HostError::Artifact(error.to_string()))
        })
        .await
        .map_err(|error| HostError::Artifact(error.to_string()))?
    }
}
