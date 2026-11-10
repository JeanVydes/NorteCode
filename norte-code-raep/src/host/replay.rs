//! Replay stores: the event history a client can resume from.

use async_trait::async_trait;

use raep_types::{Envelope, FrameId, TurnId};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[allow(unused_imports)]
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
    ///
    /// The response body may be left empty: [`Self::idempotency_response`]
    /// reads it when a duplicate frame actually needs it.
    async fn load_idempotency(
        &self,
        _session_id: &str,
    ) -> Result<Vec<IdempotencyRecord>, HostError> {
        Ok(Vec::new())
    }

    /// Reads the stored response for one frame, when the record did not carry
    /// it.
    async fn idempotency_response(
        &self,
        _session_id: &str,
        _frame_id: &str,
    ) -> Result<Option<Vec<Envelope<Value>>>, HostError> {
        Ok(None)
    }
}
/// A newline-delimited JSON replay journal.
pub struct FileReplayStore {
    path: PathBuf,
    max_frames: usize,
    lock: tokio::sync::Mutex<()>,
}
/// SQLite-backed replay storage safe for multiple server processes.
///
/// One connection is kept behind a mutex rather than opened per frame: every
/// append used to be its own connection, and two writers in one process made
/// SQLite answer "database is locked" under load, which surfaced as a replay
/// journal error in the middle of a turn.
pub struct SqliteReplayStore {
    connection: Arc<std::sync::Mutex<rusqlite::Connection>>,
    max_frames: usize,
}

impl SqliteReplayStore {
    /// Opens or creates the replay tables in the supplied database.
    ///
    /// # Errors
    ///
    /// Returns an error when SQLite cannot open the database, enable WAL or
    /// create its schema.
    pub fn open(path: impl AsRef<Path>, max_frames: usize) -> Result<Self, HostError> {
        if max_frames == 0 {
            return Err(HostError::ReplayJournal(
                "replay journal capacity must be positive".to_owned(),
            ));
        }
        let connection = rusqlite::Connection::open(path)
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        // WAL lets a reader and this writer coexist. It is an error here and
        // not a silent fallback: without it every read blocks the writer.
        connection
            .pragma_update(None, "journal_mode", "WAL")
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
        Ok(Self {
            connection: Arc::new(std::sync::Mutex::new(connection)),
            max_frames,
        })
    }

    /// Runs one blocking database operation on the shared connection.
    async fn with_connection<T, F>(&self, work: F) -> Result<T, HostError>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> Result<T, HostError> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let guard = connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            work(&guard)
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }
}

#[async_trait]
impl ReplayStore for SqliteReplayStore {
    async fn append(&self, frame: &Envelope<Value>) -> Result<(), HostError> {
        let max_frames = i64::try_from(self.max_frames)
            .map_err(|_| HostError::ReplayJournal("replay capacity is too large".to_owned()))?;
        let frame = frame.clone();
        self.with_connection(move |connection| {
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
    }

    async fn load(&self, session_id: &str) -> Result<Vec<Envelope<Value>>, HostError> {
        let session_id = session_id.to_owned();
        self.with_connection(move |connection| {
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
    }

    async fn save_idempotency(&self, record: &IdempotencyRecord) -> Result<(), HostError> {
        let max_entries = i64::try_from(self.max_frames).map_err(|_| {
            HostError::ReplayJournal("idempotency capacity is too large".to_owned())
        })?;
        let record = record.clone();
        self.with_connection(move |connection| {
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
    }

    async fn idempotency_response(
        &self,
        session_id: &str,
        frame_id: &str,
    ) -> Result<Option<Vec<Envelope<Value>>>, HostError> {
        let session_id = session_id.to_owned();
        let frame_id = frame_id.to_owned();
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT response FROM raep_idempotency
                     WHERE session_id = ?1 AND frame_id = ?2",
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let mut rows = statement
                .query(rusqlite::params![session_id, frame_id])
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let Some(row) = rows
                .next()
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?
            else {
                return Ok(None);
            };
            let encoded: String = row
                .get(0)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            serde_json::from_str(&encoded)
                .map(Some)
                .map_err(HostError::from)
        })
        .await
    }

    async fn load_idempotency(
        &self,
        session_id: &str,
    ) -> Result<Vec<IdempotencyRecord>, HostError> {
        let session_id = session_id.to_owned();
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT frame_id, turn_id, fingerprint, turn_fingerprint,
                            service_turn_id, NULL
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
                        row.get::<_, Option<String>>(5)?,
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
                let response =
                    response.map_or(Ok(Vec::new()), |response| serde_json::from_str(&response))?;
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
