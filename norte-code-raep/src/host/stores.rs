//! The stores the host keeps: pins, artifacts, uploads and replay.

use async_trait::async_trait;

use raep_types::{AttachMode, Envelope, FrameId, TurnId};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

#[allow(clippy::wildcard_imports)]
use super::*;

/// Durable storage for RAEP frames.
#[async_trait]
pub trait ReplayStore: Send + Sync {
    /// Appends one emitted frame.
    async fn append(&self, frame: &Envelope<Value>) -> Result<(), HostError>;

    /// Loads frames for one session, including frames retained before restart.
    async fn load(&self, session_id: &str) -> Result<Vec<Envelope<Value>>, HostError>;

    /// Persists one request's idempotency record.
    async fn save_idempotency(&self, _record: &IdempotencyRecord) -> Result<(), HostError> {
        Ok(())
    }

    /// Loads idempotency records retained for one session.
    async fn load_idempotency(
        &self,
        _session_id: &str,
    ) -> Result<Vec<IdempotencyRecord>, HostError> {
        Ok(Vec::new())
    }
}

/// A durable request fingerprint and the response to return for a retry.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct IdempotencyRecord {
    /// Session scope (empty for connection-level commands).
    pub session_id: String,
    /// Client-provided request identifier.
    pub frame_id: FrameId,
    /// SHA-256 fingerprint of the complete request envelope.
    pub fingerprint: String,
    /// Optional client turn identifier.
    pub turn_id: Option<TurnId>,
    /// Fingerprint of the `USER_INPUT` payload, when applicable.
    pub turn_fingerprint: Option<String>,
    /// Norte Code's numeric turn key, used to restore client-ID correlation.
    pub service_turn_id: Option<u64>,
    /// Frames returned for the request.
    pub response: Vec<Envelope<Value>>,
}

/// A stored upload that can be referenced by a user input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactRef {
    /// Opaque upload identifier.
    pub id: String,
    /// MIME type supplied by the client.
    pub mime_type: String,
    /// Number of stored bytes.
    pub bytes: u64,
}

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

/// Durable pin storage for `CONTEXT_PIN` (RFC 18.5).
///
/// Pins are session-scoped sets of turn ids. The host persists them here
/// so they survive a restart; the in-memory map in `HostState` is only a
/// cache.
#[async_trait]
pub trait PinStore: Send + Sync {
    /// Reads all pins for one session.
    async fn load(&self, session_id: &str) -> Result<Vec<String>, HostError>;
    /// Replaces all pins for one session.
    async fn save(&self, session_id: &str, pins: &[String]) -> Result<(), HostError>;
}

/// JSON-file pin storage for the RAEP server.
///
/// One file per session: `pins/<session_id>.json` with a JSON array of
/// turn ids. Writes are atomic (tmp + rename); reads tolerate a missing
/// file as `[]`.
#[derive(Clone, Debug)]
pub struct FilePinStore {
    dir: PathBuf,
}

impl FilePinStore {
    /// Opens a pin store rooted at `dir` (created on first write).
    #[must_use]
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_owned(),
        }
    }

    fn path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.json"))
    }
}

#[async_trait]
impl PinStore for FilePinStore {
    async fn load(&self, session_id: &str) -> Result<Vec<String>, HostError> {
        let path = self.path(session_id);
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(HostError::ReplayJournal(error.to_string())),
        };
        serde_json::from_str::<Vec<String>>(&content)
            .map_err(|error| HostError::ReplayJournal(format!("pin file: {error}")))
    }

    async fn save(&self, session_id: &str, pins: &[String]) -> Result<(), HostError> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        let path = self.path(session_id);
        let tmp = path.with_extension("json.tmp");
        let content = serde_json::to_string(pins).map_err(HostError::Serialization)?;
        tokio::fs::write(&tmp, content)
            .await
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        Ok(())
    }
}

/// Durable storage for client-uploaded artifacts.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Stores bytes and returns their opaque reference.
    async fn put(&self, mime_type: &str, bytes: &[u8]) -> Result<ArtifactRef, HostError>;

    /// Reads an uploaded artifact by its content-addressed identifier.
    async fn get(&self, id: &str) -> Result<Vec<u8>, HostError>;
}

/// Blob-backed artifact storage for the RAEP server.
pub struct BlobArtifactStore {
    blobs: Arc<dyn norte::BlobStore>,
}

impl BlobArtifactStore {
    /// Creates an artifact adapter over Norte's content-addressed blob store.
    #[must_use]
    pub fn new(blobs: Arc<dyn norte::BlobStore>) -> Self {
        Self { blobs }
    }
}

#[async_trait]
impl ArtifactStore for BlobArtifactStore {
    async fn put(&self, mime_type: &str, bytes: &[u8]) -> Result<ArtifactRef, HostError> {
        let id = self
            .blobs
            .put(bytes)
            .await
            .map_err(|error| HostError::Artifact(error.to_string()))?;
        Ok(ArtifactRef {
            id: id.to_string(),
            mime_type: mime_type.to_owned(),
            bytes: bytes.len() as u64,
        })
    }

    async fn get(&self, id: &str) -> Result<Vec<u8>, HostError> {
        let id = id
            .parse()
            .map_err(|_| HostError::Artifact("invalid artifact id".to_owned()))?;
        self.blobs
            .get(id)
            .await
            .map_err(|error| HostError::Artifact(error.to_string()))
    }
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

/// A newline-delimited JSON replay journal.
pub struct FileReplayStore {
    path: PathBuf,
    max_frames: usize,
    lock: tokio::sync::Mutex<()>,
}

/// SQLite-backed replay storage safe for multiple server processes.
pub struct SqliteReplayStore {
    path: PathBuf,
    max_frames: usize,
}

impl SqliteReplayStore {
    /// Opens or creates the replay table in the supplied database.
    ///
    /// # Errors
    ///
    /// Returns an error when SQLite cannot open the database or create its
    /// schema.
    pub fn open(path: impl Into<PathBuf>, max_frames: usize) -> Result<Self, HostError> {
        if max_frames == 0 {
            return Err(HostError::ReplayJournal(
                "replay journal capacity must be positive".to_owned(),
            ));
        }
        let path = path.into();
        let connection = rusqlite::Connection::open(&path)
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS raep_replay (
                    session_id TEXT NOT NULL,
                    seq INTEGER NOT NULL,
                    frame TEXT NOT NULL,
                    PRIMARY KEY (session_id, seq)
                );
                CREATE INDEX IF NOT EXISTS idx_raep_replay_session_seq
                    ON raep_replay(session_id, seq);
                CREATE TABLE IF NOT EXISTS raep_idempotency (
                    session_id TEXT NOT NULL,
                    frame_id TEXT NOT NULL,
                    turn_id TEXT,
                    fingerprint TEXT NOT NULL,
                    turn_fingerprint TEXT,
                    service_turn_id INTEGER,
                    response TEXT NOT NULL,
                    PRIMARY KEY (session_id, frame_id)
                );
                CREATE INDEX IF NOT EXISTS idx_raep_idempotency_session
                    ON raep_idempotency(session_id);",
            )
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        Ok(Self { path, max_frames })
    }
}

#[async_trait]
impl ReplayStore for SqliteReplayStore {
    async fn append(&self, frame: &Envelope<Value>) -> Result<(), HostError> {
        let path = self.path.clone();
        let max_frames = i64::try_from(self.max_frames)
            .map_err(|_| HostError::ReplayJournal("replay capacity is too large".to_owned()))?;
        let frame = frame.clone();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let encoded = serde_json::to_string(&frame)?;
            let sequence = frame.seq.ok_or_else(|| {
                HostError::ReplayJournal("replay frame has no sequence".to_owned())
            })?;
            let session_id = frame.session_id.clone();
            connection
                .execute(
                    "INSERT OR REPLACE INTO raep_replay(session_id, seq, frame)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![session_id, sequence, encoded],
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .execute(
                    "DELETE FROM raep_replay
                     WHERE session_id = ?1
                       AND seq NOT IN (
                           SELECT seq FROM raep_replay
                           WHERE session_id = ?1
                           ORDER BY seq DESC LIMIT ?2
                       )",
                    rusqlite::params![frame.session_id, max_frames],
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn load(&self, session_id: &str) -> Result<Vec<Envelope<Value>>, HostError> {
        let path = self.path.clone();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let mut statement = connection
                .prepare(
                    "SELECT frame FROM raep_replay
                     WHERE session_id = ?1 ORDER BY seq ASC",
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let rows = statement
                .query_map(rusqlite::params![session_id], |row| row.get::<_, String>(0))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            rows.map(|row| {
                let encoded = row.map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                serde_json::from_str(&encoded).map_err(HostError::from)
            })
            .collect()
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn save_idempotency(&self, record: &IdempotencyRecord) -> Result<(), HostError> {
        let path = self.path.clone();
        let max_entries = i64::try_from(self.max_frames).map_err(|_| {
            HostError::ReplayJournal("idempotency capacity is too large".to_owned())
        })?;
        let record = record.clone();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let response = serde_json::to_string(&record.response)?;
            connection
                .execute(
                    "INSERT OR REPLACE INTO raep_idempotency
                     (session_id, frame_id, turn_id, fingerprint, turn_fingerprint,
                      service_turn_id, response)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        record.session_id,
                        record.frame_id.as_str(),
                        record.turn_id.as_ref().map(TurnId::as_str),
                        record.fingerprint,
                        record.turn_fingerprint,
                        record.service_turn_id,
                        response
                    ],
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .execute(
                    "DELETE FROM raep_idempotency
                     WHERE session_id = ?1
                       AND frame_id NOT IN (
                           SELECT frame_id FROM raep_idempotency
                           WHERE session_id = ?1
                           ORDER BY rowid DESC LIMIT ?2
                       )",
                    rusqlite::params![record.session_id, max_entries],
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn load_idempotency(
        &self,
        session_id: &str,
    ) -> Result<Vec<IdempotencyRecord>, HostError> {
        let path = self.path.clone();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let mut statement = connection
                .prepare(
                    "SELECT frame_id, turn_id, fingerprint, turn_fingerprint,
                            service_turn_id, response
                     FROM raep_idempotency
                     WHERE session_id = ?1 ORDER BY rowid ASC",
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let rows = statement
                .query_map(rusqlite::params![session_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<u64>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            rows.map(|row| {
                let (frame_id, turn_id, fingerprint, turn_fingerprint, service_turn_id, response) =
                    row.map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                let frame_id = FrameId::new(frame_id)
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                let turn_id = turn_id
                    .map(TurnId::new)
                    .transpose()
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                let response = serde_json::from_str(&response)?;
                Ok(IdempotencyRecord {
                    session_id: session_id.clone(),
                    frame_id,
                    fingerprint,
                    turn_id,
                    turn_fingerprint,
                    service_turn_id,
                    response,
                })
            })
            .collect()
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }
}

impl FileReplayStore {
    /// Opens a journal at `path`, creating its parent directory when needed.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory cannot be created.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, HostError> {
        let path = path.into();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        }
        Ok(Self {
            path,
            max_frames: 10_000,
            lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Opens a journal with a bounded number of frames retained per session.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory cannot be created or the
    /// capacity is zero.
    pub fn open_with_capacity(
        path: impl Into<PathBuf>,
        max_frames: usize,
    ) -> Result<Self, HostError> {
        if max_frames == 0 {
            return Err(HostError::ReplayJournal(
                "replay journal capacity must be positive".to_owned(),
            ));
        }
        let mut store = Self::open(path)?;
        store.max_frames = max_frames;
        Ok(store)
    }
}

#[async_trait]
impl ReplayStore for FileReplayStore {
    async fn append(&self, frame: &Envelope<Value>) -> Result<(), HostError> {
        let _guard = self.lock.lock().await;
        let line = serde_json::to_string(frame)?;
        let path = self.path.clone();
        let max_frames = self.max_frames;
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let mut by_session: HashMap<String, Vec<Envelope<Value>>> = HashMap::new();
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                for existing in content.lines().filter(|line| !line.trim().is_empty()) {
                    let frame = serde_json::from_str::<Envelope<Value>>(existing)
                        .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                    by_session
                        .entry(frame.session_id.clone())
                        .or_default()
                        .push(frame);
                }
            }
            let appended = serde_json::from_str::<Envelope<Value>>(&line)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            by_session
                .entry(appended.session_id.clone())
                .or_default()
                .push(appended);
            let mut retained = by_session
                .into_values()
                .flat_map(|mut frames| {
                    if frames.len() > max_frames {
                        frames.drain(..frames.len() - max_frames);
                    }
                    frames
                })
                .collect::<Vec<_>>();
            retained.sort_by_key(|frame| frame.seq.unwrap_or_default());
            let temporary = path.with_extension("jsonl.tmp");
            let mut file = std::fs::File::create(&temporary)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            for frame in retained {
                let line = serde_json::to_string(&frame)?;
                writeln!(file, "{line}")
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            }
            std::fs::rename(temporary, path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn load(&self, session_id: &str) -> Result<Vec<Envelope<Value>>, HostError> {
        let _guard = self.lock.lock().await;
        let path = self.path.clone();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            if !Path::new(&path).exists() {
                return Ok(Vec::new());
            }
            let content = std::fs::read_to_string(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str::<Envelope<Value>>(line)
                        .map_err(|error| HostError::ReplayJournal(error.to_string()))
                })
                .filter_map(|result| match result {
                    Ok(frame) if frame.session_id == session_id => Some(Ok(frame)),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect()
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn save_idempotency(&self, record: &IdempotencyRecord) -> Result<(), HostError> {
        let _guard = self.lock.lock().await;
        let path = self.path.with_extension("idempotency.jsonl");
        let record = record.clone();
        let max_entries = self.max_frames;
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let mut records = Vec::new();
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                for line in content.lines().filter(|line| !line.trim().is_empty()) {
                    let existing = serde_json::from_str::<IdempotencyRecord>(line)
                        .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                    if existing.session_id != record.session_id
                        || existing.frame_id != record.frame_id
                    {
                        records.push(existing);
                    }
                }
            }
            records.push(record);
            let mut seen = HashMap::<String, usize>::new();
            let mut retained = records
                .into_iter()
                .rev()
                .filter(|entry| {
                    let count = seen.entry(entry.session_id.clone()).or_default();
                    let keep = *count < max_entries;
                    *count += 1;
                    keep
                })
                .collect::<Vec<_>>();
            retained.reverse();
            let temporary = path.with_extension("idempotency.jsonl.tmp");
            let mut file = std::fs::File::create(&temporary)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            for entry in retained {
                writeln!(file, "{}", serde_json::to_string(&entry)?)
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            }
            std::fs::rename(temporary, path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn load_idempotency(
        &self,
        session_id: &str,
    ) -> Result<Vec<IdempotencyRecord>, HostError> {
        let _guard = self.lock.lock().await;
        let path = self.path.with_extension("idempotency.jsonl");
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            if !path.exists() {
                return Ok(Vec::new());
            }
            let content = std::fs::read_to_string(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str::<IdempotencyRecord>(line)
                        .map_err(|error| HostError::ReplayJournal(error.to_string()))
                })
                .filter_map(|record| match record {
                    Ok(record) if record.session_id == session_id => Some(Ok(record)),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect()
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }
}

#[derive(Default)]
pub(super) struct HostState {
    pub(super) attached: HashSet<String>,
    pub(super) attachment_modes: HashMap<String, AttachMode>,
    pub(super) sequences: HashMap<String, u64>,
    pub(super) replay: HashMap<String, VecDeque<Envelope<Value>>>,
    pub(super) idempotency: HashMap<(String, FrameId), CachedRequest>,
    pub(super) idempotency_order: VecDeque<(String, FrameId)>,
    pub(super) loaded_replay: HashSet<String>,
    pub(super) session_owners: HashMap<String, String>,
    pub(super) artifact_owners: HashMap<String, String>,
    pub(super) exports: HashMap<String, StoredExport>,
    pub(super) imports: HashMap<String, StoredImport>,
    pub(super) session_agents: HashMap<String, String>,
    pub(super) session_create_keys: HashMap<String, String>,
    pub(super) session_create_order: VecDeque<String>,
    pub(super) client_capabilities: HashMap<String, raep_types::ClientCapabilities>,
    pub(super) pending_output: HashMap<String, Vec<raep_types::Modality>>,
    pub(super) turn_output: HashMap<(String, u64), Vec<raep_types::Modality>>,
    pub(super) pending_turn_ids: HashMap<String, VecDeque<TurnId>>,
    pub(super) turn_aliases: HashMap<(String, u64), TurnId>,
    pub(super) turn_alias_order: VecDeque<(String, u64)>,
    pub(super) active_turns: HashMap<String, TurnId>,
    pub(super) turn_reservations: HashMap<String, TurnId>,
    pub(super) turn_request_fingerprints: HashMap<(String, TurnId), String>,
    pub(super) turn_request_frames: HashMap<(String, TurnId), Envelope<Value>>,
    pub(super) turn_request_order: VecDeque<(String, TurnId)>,
    pub(super) loaded_idempotency: HashSet<String>,
    pub(super) queued_turns: HashMap<String, VecDeque<QueuedTurn>>,
    /// Pinned turns per session (RFC 18.5). Held in-memory; the service
    /// has no pin table yet, so pins do not survive a restart.
    pub(super) pinned_turns: HashMap<String, Vec<String>>,
}

#[derive(Clone)]
pub(crate) struct QueuedTurn {
    pub(super) turn_id: TurnId,
    pub(super) text: String,
    pub(super) attachments: Vec<norte_code_wire::Attachment>,
    pub(super) budget: Option<norte_code_wire::Budget>,
    pub(super) reasoning: Option<norte_code_wire::Reasoning>,
    pub(super) metadata: Option<Value>,
    pub(super) output: Vec<raep_types::Modality>,
    /// `USER_INPUT.response_format` (RFC 12.6), validated for shape in
    /// `validate_user_input` and carried to the service as
    /// `SendMessage.response_format`. `None` is `{type: "text"}`.
    pub(super) response_format: Option<norte_code_wire::ResponseFormat>,
    /// `USER_INPUT.replaces_turn_id` (RFC 18.2): `SESSION_REWIND` to that
    /// turn with `inclusive: true`, `cancel_pending: false`, then run this
    /// input as a new turn, in one step. `queue_policy` does not apply.
    pub(super) replaces_turn_id: Option<String>,
}

pub(super) struct CachedRequest {
    pub(super) fingerprint: String,
    pub(super) response: Vec<Envelope<Value>>,
}

pub(super) struct StoredExport {
    pub(super) bytes: Vec<u8>,
    pub(super) owner: Option<String>,
    pub(super) expires_at: i64,
}

#[derive(Clone)]
pub(super) struct StoredImport {
    pub(super) manifest: crate::bundle::BundleManifest,
    pub(super) history: Vec<norte_code_agent::service::ImportedHistoryItem>,
    pub(super) title: Option<String>,
    pub(super) owner: String,
    pub(super) expires_at: i64,
}
