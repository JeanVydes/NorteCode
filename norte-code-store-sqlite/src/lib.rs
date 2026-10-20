//! A `SQLite` [`norte::Store`] and [`norte::BlobStore`] for Norte Code.
//!
//! Implements Norte's durability contract with compare-and-swap on the session
//! version and one transaction per commit, and adds Norte Code's own tables
//! (logical sessions, pending questions, settings and blobs). It passes Norte's
//! `store_conformance` test suite.
//!
//! `SQLite` is synchronous, so every call runs on the blocking pool behind one
//! connection guarded by a mutex. That is enough for a single desktop host; a
//! server build would pool connections instead.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use norte::{
    ApprovalId, ApprovalRecord, BlobId, BlobStore, CallId, Commit, Error, Grant, GrantId,
    GrantScope, Node, NodeId, Result, Session, SessionId, Store, ToolName,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// A failure opening or configuring the database file.
#[derive(Debug, Error)]
pub enum SqliteError {
    /// The database could not be opened or migrated.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The database directory could not be created.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// One logical session as Norte Code's own tables hold it.
///
/// The Norte session id is where the conversation lives; the rest is host
/// bookkeeping a client needs to list and resume work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSession {
    /// The stable client-facing key.
    pub key: String,
    /// The Norte session it maps to.
    pub north: SessionId,
    /// The user-visible title, when one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The `provider/model` the session last used.
    pub model: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// The last phase Norte Code observed, as its protocol names it.
    pub phase: String,
    /// Host counters (billed messages, reported cost), opaque to the store.
    #[serde(default)]
    pub stats: Value,
}

/// A question a turn is parked on, durable across a restart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredQuestion {
    /// The question id shown to the client.
    pub id: String,
    /// The session that asked.
    pub session: SessionId,
    /// The tool call it belongs to, the durable key.
    pub call_id: String,
    /// What the model wants to know.
    pub question: String,
    /// The answer, once a human gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// Norte Code's own persistence, beside Norte's [`Store`].
///
/// The agent keeps an in-memory map for speed and mirrors every change here so a
/// restart can rebuild it.
#[async_trait]
pub trait SessionDirectory: Send + Sync {
    /// Every stored session, newest first.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn sessions(&self) -> Result<Vec<StoredSession>>;
    /// Inserts or replaces one session.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn save_session(&self, session: &StoredSession) -> Result<()>;
    /// Removes one session by key. Missing keys are not an error.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn delete_session(&self, key: &str) -> Result<()>;
    /// The durable question for a call, if one was recorded.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn question(&self, call_id: &str) -> Result<Option<StoredQuestion>>;
    /// Records a question a turn is waiting on.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn save_question(&self, question: &StoredQuestion) -> Result<()>;
    /// Records the answer to a question. Returns `false` when the answer was
    /// already given, so a replayed answer is idempotent.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn answer_question(&self, call_id: &str, answer: &str) -> Result<bool>;
}

/// A directory that keeps everything in memory, for tests and one-shot runs.
#[derive(Debug, Default)]
pub struct InMemoryDirectory {
    sessions: Mutex<BTreeMap<String, StoredSession>>,
    questions: Mutex<BTreeMap<String, StoredQuestion>>,
}

impl InMemoryDirectory {
    /// An empty directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionDirectory for InMemoryDirectory {
    async fn sessions(&self) -> Result<Vec<StoredSession>> {
        let mut sessions: Vec<StoredSession> = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.created_at));
        Ok(sessions)
    }

    async fn save_session(&self, session: &StoredSession) -> Result<()> {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session.key.clone(), session.clone());
        Ok(())
    }

    async fn delete_session(&self, key: &str) -> Result<()> {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
        Ok(())
    }

    async fn question(&self, call_id: &str) -> Result<Option<StoredQuestion>> {
        Ok(self
            .questions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(call_id)
            .cloned())
    }

    async fn save_question(&self, question: &StoredQuestion) -> Result<()> {
        let mut questions = self
            .questions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        questions
            .entry(question.call_id.clone())
            .and_modify(|stored| {
                if stored.answer.is_none() {
                    stored.answer.clone_from(&question.answer);
                }
                stored.id.clone_from(&question.id);
                stored.session = question.session;
                stored.question.clone_from(&question.question);
            })
            .or_insert_with(|| question.clone());
        Ok(())
    }

    async fn answer_question(&self, call_id: &str, answer: &str) -> Result<bool> {
        let mut questions = self
            .questions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(question) = questions.get_mut(call_id) else {
            return Ok(false);
        };
        if question.answer.is_some() {
            return Ok(false);
        }
        question.answer = Some(answer.to_owned());
        Ok(true)
    }
}

/// A `SQLite` store: Norte's graph, approvals and blobs, plus host tables.
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore").finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// Opens (or creates) the database at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`SqliteError`] when the file cannot be opened or migrated.
    pub fn open(path: impl AsRef<Path>) -> std::result::Result<Self, SqliteError> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Self::from_connection(Connection::open(path)?)
    }

    /// An in-memory database, for tests and one-shot runs.
    ///
    /// # Errors
    ///
    /// Returns [`SqliteError`] when the schema cannot be created.
    pub fn in_memory() -> std::result::Result<Self, SqliteError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> std::result::Result<Self, SqliteError> {
        // WAL keeps readers from blocking the single writer; it is a no-op for
        // an in-memory database.
        let _ = connection.pragma_update(None, "journal_mode", "WAL");
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    async fn run<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> std::result::Result<T, Fault> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let guard = connection.lock().unwrap_or_else(PoisonError::into_inner);
            work(&guard).map_err(Error::from)
        })
        .await
        .map_err(|error| Error::Other(format!("the database task failed: {error}")))?
    }

    async fn transact<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Transaction<'_>) -> std::result::Result<T, Fault> + Send + 'static,
    {
        self.run(move |connection| {
            let transaction = connection.unchecked_transaction().map_err(Fault::from)?;
            let value = work(&transaction)?;
            transaction.commit().map_err(Fault::from)?;
            Ok(value)
        })
        .await
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id      TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    data    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS nodes (
    seq    INTEGER PRIMARY KEY AUTOINCREMENT,
    id     TEXT NOT NULL UNIQUE,
    parent TEXT,
    data   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS nodes_parent ON nodes(parent);
CREATE TABLE IF NOT EXISTS approvals (
    id           TEXT PRIMARY KEY,
    session      TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    data         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS approvals_session ON approvals(session);
CREATE TABLE IF NOT EXISTS blobs (
    id    TEXT PRIMARY KEY,
    bytes BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS logical_sessions (
    key        TEXT PRIMARY KEY,
    north      TEXT NOT NULL,
    title      TEXT,
    model      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    phase      TEXT NOT NULL,
    stats      TEXT NOT NULL,
    data       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS questions (
    call_id  TEXT PRIMARY KEY,
    id       TEXT NOT NULL,
    session  TEXT NOT NULL,
    question TEXT NOT NULL,
    answer   TEXT
);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// The internal error a blocking section can raise.
///
/// It exists because a `SQLite` call fails with a `rusqlite` error while the
/// store also has to raise Norte's typed errors; the conversion happens at the
/// boundary, once.
#[derive(Debug, Error)]
enum Fault {
    #[error("sqlite: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("store conflict on session {session}: expected {expected:?}, found {actual}")]
    Conflict {
        session: SessionId,
        expected: Option<u64>,
        actual: u64,
    },
    #[error("session not found: {0}")]
    SessionNotFound(SessionId),
    #[error("node not found: {0}")]
    NodeNotFound(NodeId),
    #[error("{0}")]
    InvalidInput(String),
    #[error("{what} is corrupt: {reason}")]
    Corrupt { what: String, reason: String },
}

impl From<Fault> for Error {
    fn from(fault: Fault) -> Self {
        match fault {
            Fault::Db(error) => Error::Other(format!("sqlite: {error}")),
            Fault::Conflict {
                session,
                expected,
                actual,
            } => Error::StoreConflict {
                session,
                expected,
                actual,
            },
            Fault::SessionNotFound(session) => Error::SessionNotFound(session),
            Fault::NodeNotFound(node) => Error::NodeNotFound(node),
            Fault::InvalidInput(message) => Error::InvalidInput(message),
            Fault::Corrupt { what, reason } => Error::Other(format!("{what} is corrupt: {reason}")),
        }
    }
}

fn encode<T: Serialize>(value: &T, what: &str) -> std::result::Result<String, Fault> {
    serde_json::to_string(value).map_err(|error| Fault::Corrupt {
        what: what.to_owned(),
        reason: error.to_string(),
    })
}

fn decode<T: serde::de::DeserializeOwned>(text: &str, what: &str) -> std::result::Result<T, Fault> {
    serde_json::from_str(text).map_err(|error| Fault::Corrupt {
        what: what.to_owned(),
        reason: error.to_string(),
    })
}

/// A session as stored: the session with its grants lifted out for the same
/// reason as [`ApprovalEnvelope`].
#[derive(Serialize, Deserialize)]
struct SessionEnvelope {
    session: Session,
    #[serde(default)]
    grants: Vec<GrantDto>,
}

fn encode_session(session: &Session) -> std::result::Result<String, Fault> {
    let grants: Vec<GrantDto> = session.grants.iter().map(GrantDto::from).collect();
    let mut session = session.clone();
    session.grants.clear();
    encode(&SessionEnvelope { session, grants }, "session")
}

fn decode_session(text: &str) -> std::result::Result<Session, Fault> {
    // Sessions written before the envelope existed carry no grants beyond what
    // the plain encoding can represent.
    let Ok(envelope) = decode::<SessionEnvelope>(text, "session") else {
        return decode::<Session>(text, "session");
    };
    let mut session = envelope.session;
    session.grants = envelope.grants.into_iter().map(Grant::from).collect();
    Ok(session)
}

/// An approval as stored: the record with its grants lifted out.
///
/// `GrantScope` is an internally tagged enum with newtype variants around
/// strings, which `serde_json` cannot represent. Storing the grants beside the
/// record keeps every other field as it is.
#[derive(Serialize, Deserialize)]
struct ApprovalEnvelope {
    record: ApprovalRecord,
    #[serde(default)]
    grants: Vec<GrantDto>,
}

#[derive(Serialize, Deserialize)]
struct GrantDto {
    id: GrantId,
    scope: ScopeDto,
    issued_at: DateTime<Utc>,
    issued_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revoked_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ScopeDto {
    Call { id: CallId },
    Tool { name: ToolName },
    Session { id: SessionId },
    Global,
}

impl From<&GrantScope> for ScopeDto {
    fn from(scope: &GrantScope) -> Self {
        match scope {
            GrantScope::Call(id) => Self::Call { id: *id },
            GrantScope::Tool(name) => Self::Tool { name: name.clone() },
            GrantScope::Session(id) => Self::Session { id: *id },
            _ => Self::Global,
        }
    }
}

impl From<ScopeDto> for GrantScope {
    fn from(scope: ScopeDto) -> Self {
        match scope {
            ScopeDto::Call { id } => Self::Call(id),
            ScopeDto::Tool { name } => Self::Tool(name),
            ScopeDto::Session { id } => Self::Session(id),
            ScopeDto::Global => Self::Global,
        }
    }
}

impl From<&Grant> for GrantDto {
    fn from(grant: &Grant) -> Self {
        Self {
            id: grant.id,
            scope: ScopeDto::from(&grant.scope),
            issued_at: grant.issued_at,
            issued_by: grant.issued_by.clone(),
            revoked_at: grant.revoked_at,
        }
    }
}

impl From<GrantDto> for Grant {
    fn from(grant: GrantDto) -> Self {
        Self {
            id: grant.id,
            scope: grant.scope.into(),
            issued_at: grant.issued_at,
            issued_by: grant.issued_by,
            revoked_at: grant.revoked_at,
        }
    }
}

fn encode_approval(record: &ApprovalRecord) -> std::result::Result<String, Fault> {
    let grants: Vec<GrantDto> = record.grants.iter().map(GrantDto::from).collect();
    let mut record = record.clone();
    record.grants.clear();
    encode(&ApprovalEnvelope { record, grants }, "approval")
}

fn decode_approval(text: &str) -> std::result::Result<ApprovalRecord, Fault> {
    let envelope: ApprovalEnvelope = decode(text, "approval")?;
    let mut record = envelope.record;
    record.grants = envelope.grants.into_iter().map(Grant::from).collect();
    Ok(record)
}

#[async_trait]
impl Store for SqliteStore {
    async fn commit(&self, commit: Commit) -> Result<Session> {
        commit.validate()?;
        let session_id = commit.session_id;
        self.transact(move |transaction| {
            let existing: Option<(i64, String)> = transaction
                .query_row(
                    "SELECT version, data FROM sessions WHERE id = ?1",
                    params![session_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(Fault::from)?;
            let mut session = match (commit.expected_version, existing) {
                (None, Some((version, _))) => {
                    return Err(Fault::Conflict {
                        session: session_id,
                        expected: None,
                        actual: u64::try_from(version).unwrap_or(0),
                    });
                }
                (None, None) => {
                    let mut session = Session::new();
                    session.id = session_id;
                    session
                }
                (Some(_), None) => return Err(Fault::SessionNotFound(session_id)),
                (Some(expected), Some((version, data))) => {
                    if u64::try_from(version).ok() != Some(expected) {
                        return Err(Fault::Conflict {
                            session: session_id,
                            expected: Some(expected),
                            actual: u64::try_from(version).unwrap_or(0),
                        });
                    }
                    decode_session(&data)?
                }
            };
            for node in &commit.nodes {
                let exists: Option<i64> = transaction
                    .query_row(
                        "SELECT 1 FROM nodes WHERE id = ?1",
                        params![node.id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(Fault::from)?;
                if exists.is_some() {
                    return Err(Fault::InvalidInput(format!(
                        "node {} already exists",
                        node.id
                    )));
                }
                transaction
                    .execute(
                        "INSERT INTO nodes (id, parent, data) VALUES (?1, ?2, ?3)",
                        params![
                            node.id.to_string(),
                            node.parent.map(|parent| parent.to_string()),
                            encode(node, "node")?,
                        ],
                    )
                    .map_err(Fault::from)?;
            }
            commit.apply(&mut session);
            transaction
                .execute(
                    "INSERT INTO sessions (id, version, data) VALUES (?1, ?2, ?3)
                     ON CONFLICT(id) DO UPDATE SET version = excluded.version, data = excluded.data",
                    params![
                        session.id.to_string(),
                        i64::try_from(session.version).unwrap_or(i64::MAX),
                        encode_session(&session)?,
                    ],
                )
                .map_err(Fault::from)?;
            Ok(session)
        })
        .await
    }

    async fn load(&self, session: SessionId) -> Result<Session> {
        let data: Option<String> = self
            .run(move |connection| {
                connection
                    .query_row(
                        "SELECT data FROM sessions WHERE id = ?1",
                        params![session.to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(Fault::from)
            })
            .await?;
        data.map_or_else(
            || Err(Error::SessionNotFound(session)),
            |data| decode_session(&data).map_err(Error::from),
        )
    }

    async fn node(&self, id: NodeId) -> Result<Node> {
        let data: Option<String> = self
            .run(move |connection| {
                connection
                    .query_row(
                        "SELECT data FROM nodes WHERE id = ?1",
                        params![id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(Fault::from)
            })
            .await?;
        data.map_or_else(
            || Err(Error::NodeNotFound(id)),
            |data| decode(&data, "node").map_err(Error::from),
        )
    }

    async fn ancestors(&self, id: NodeId) -> Result<Vec<NodeId>> {
        let ids: Vec<String> = self
            .run(move |connection| {
                let mut cursor = Some(id.to_string());
                let mut chain = Vec::new();
                while let Some(current) = cursor {
                    let row: Option<Option<String>> = connection
                        .query_row(
                            "SELECT parent FROM nodes WHERE id = ?1",
                            params![current],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(Fault::from)?;
                    match row {
                        None => return Err(Fault::NodeNotFound(id)),
                        Some(parent) => {
                            chain.push(current);
                            cursor = parent;
                        }
                    }
                }
                chain.reverse();
                Ok(chain)
            })
            .await?;
        ids.into_iter()
            .map(|value| value.parse().map_err(|error: Error| error))
            .collect()
    }

    async fn children(&self, id: NodeId) -> Result<Vec<NodeId>> {
        let ids: Vec<String> = self
            .run(move |connection| {
                let mut statement = connection
                    .prepare("SELECT id FROM nodes WHERE parent = ?1 ORDER BY seq")
                    .map_err(Fault::from)?;
                let mapped = statement
                    .query_map(params![id.to_string()], |row| row.get(0))
                    .map_err(Fault::from)?;
                mapped
                    .collect::<rusqlite::Result<Vec<String>>>()
                    .map_err(Fault::from)
            })
            .await?;
        ids.into_iter()
            .map(|value| value.parse().map_err(|error: Error| error))
            .collect()
    }

    async fn save_approval(&self, record: ApprovalRecord) -> Result<()> {
        let id = record.id;
        let session = record.session;
        let requested_at = record.requested_at.to_rfc3339();
        let data = encode_approval(&record).map_err(Error::from)?;
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO approvals (id, session, requested_at, data) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET session = excluded.session,
                       requested_at = excluded.requested_at, data = excluded.data",
                    params![id.to_string(), session.to_string(), requested_at, data],
                )
                .map_err(Fault::from)?;
            Ok(())
        })
        .await
    }

    async fn approval(&self, id: ApprovalId) -> Result<Option<ApprovalRecord>> {
        let data: Option<String> = self
            .run(move |connection| {
                connection
                    .query_row(
                        "SELECT data FROM approvals WHERE id = ?1",
                        params![id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(Fault::from)
            })
            .await?;
        data.map(|data| decode_approval(&data).map_err(Error::from))
            .transpose()
    }

    async fn approvals(&self, session: SessionId) -> Result<Vec<ApprovalRecord>> {
        let rows: Vec<String> = self
            .run(move |connection| {
                let mut statement = connection
                    .prepare("SELECT data FROM approvals WHERE session = ?1 ORDER BY requested_at")
                    .map_err(Fault::from)?;
                let mapped = statement
                    .query_map(params![session.to_string()], |row| row.get(0))
                    .map_err(Fault::from)?;
                mapped
                    .collect::<rusqlite::Result<Vec<String>>>()
                    .map_err(Fault::from)
            })
            .await?;
        rows.iter()
            .map(|data| decode_approval(data).map_err(Error::from))
            .collect()
    }
}

#[async_trait]
impl BlobStore for SqliteStore {
    async fn put(&self, bytes: &[u8]) -> Result<BlobId> {
        let id = BlobId::of(bytes);
        let key = id;
        let bytes = bytes.to_vec();
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT OR IGNORE INTO blobs (id, bytes) VALUES (?1, ?2)",
                    params![key.to_string(), bytes],
                )
                .map_err(Fault::from)?;
            Ok(())
        })
        .await?;
        Ok(id)
    }

    async fn get(&self, id: BlobId) -> Result<Vec<u8>> {
        let bytes: Option<Vec<u8>> = self
            .run(move |connection| {
                connection
                    .query_row(
                        "SELECT bytes FROM blobs WHERE id = ?1",
                        params![id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(Fault::from)
            })
            .await?;
        bytes.ok_or(Error::BlobNotFound(id))
    }

    async fn contains(&self, id: BlobId) -> Result<bool> {
        let found: Option<i64> = self
            .run(move |connection| {
                connection
                    .query_row(
                        "SELECT 1 FROM blobs WHERE id = ?1",
                        params![id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(Fault::from)
            })
            .await?;
        Ok(found.is_some())
    }
}

#[async_trait]
impl SessionDirectory for SqliteStore {
    async fn sessions(&self) -> Result<Vec<StoredSession>> {
        let rows: Vec<String> = self
            .run(|connection| {
                let mut statement = connection
                    .prepare("SELECT data FROM logical_sessions ORDER BY created_at DESC")
                    .map_err(Fault::from)?;
                let mapped = statement
                    .query_map([], |row| row.get(0))
                    .map_err(Fault::from)?;
                mapped
                    .collect::<rusqlite::Result<Vec<String>>>()
                    .map_err(Fault::from)
            })
            .await?;
        rows.iter()
            .map(|data| decode(data, "stored session").map_err(Error::from))
            .collect()
    }

    async fn save_session(&self, session: &StoredSession) -> Result<()> {
        let session = session.clone();
        self.run(move |connection| {
            let stats = encode(&session.stats, "session stats")?;
            let data = encode(&session, "stored session")?;
            connection
                .execute(
                    "INSERT INTO logical_sessions (key, north, title, model, created_at, phase, stats, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(key) DO UPDATE SET north = excluded.north, title = excluded.title,
                       model = excluded.model, created_at = excluded.created_at,
                       phase = excluded.phase, stats = excluded.stats, data = excluded.data",
                    params![
                        session.key,
                        session.north.to_string(),
                        session.title,
                        session.model,
                        session.created_at.to_rfc3339(),
                        session.phase,
                        stats,
                        data,
                    ],
                )
                .map_err(Fault::from)?;
            Ok(())
        })
        .await
    }

    async fn delete_session(&self, key: &str) -> Result<()> {
        let key = key.to_owned();
        self.run(move |connection| {
            connection
                .execute("DELETE FROM logical_sessions WHERE key = ?1", params![key])
                .map_err(Fault::from)?;
            Ok(())
        })
        .await
    }

    async fn question(&self, call_id: &str) -> Result<Option<StoredQuestion>> {
        let call_id = call_id.to_owned();
        let row: Option<(String, String, String, String, Option<String>)> = self
            .run(move |connection| {
                connection
                    .query_row(
                        "SELECT id, session, call_id, question, answer FROM questions WHERE call_id = ?1",
                        params![call_id],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(Fault::from)
            })
            .await?;
        row.map(|(id, session, call_id, question, answer)| {
            let session = session
                .parse::<SessionId>()
                .map_err(|error: Error| Error::Other(error.to_string()))?;
            Ok::<_, Error>(StoredQuestion {
                id,
                session,
                call_id,
                question,
                answer,
            })
        })
        .transpose()
    }

    async fn save_question(&self, question: &StoredQuestion) -> Result<()> {
        let question = question.clone();
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO questions (call_id, id, session, question, answer)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(call_id) DO UPDATE SET id = excluded.id,
                       session = excluded.session, question = excluded.question,
                       answer = COALESCE(questions.answer, excluded.answer)",
                    params![
                        question.call_id,
                        question.id,
                        question.session.to_string(),
                        question.question,
                        question.answer,
                    ],
                )
                .map_err(Fault::from)?;
            Ok(())
        })
        .await
    }

    async fn answer_question(&self, call_id: &str, answer: &str) -> Result<bool> {
        let call_id = call_id.to_owned();
        let answer = answer.to_owned();
        self.run(move |connection| {
            let changed = connection
                .execute(
                    "UPDATE questions SET answer = ?2 WHERE call_id = ?1 AND answer IS NULL",
                    params![call_id, answer],
                )
                .map_err(Fault::from)?;
            Ok(changed > 0)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte::{CallId, Message, Phase, ToolName};
    use std::collections::VecDeque;

    #[tokio::test]
    async fn passes_nortes_store_conformance() {
        let store = SqliteStore::in_memory().expect("open");
        norte_testing::store_conformance(&store).await;
    }

    #[tokio::test]
    async fn reopens_the_same_database() {
        let directory = std::env::temp_dir().join(format!(
            "norte-code-sqlite-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).expect("directory");
        let path = directory.join("sessions.db");

        let session_id = SessionId::new();
        {
            let store = SqliteStore::open(&path).expect("open");
            store
                .commit(Commit {
                    session_id,
                    expected_version: None,
                    nodes: Vec::new(),
                    head: None,
                    phase: Phase::Idle,
                    turn: 0,
                    queue: VecDeque::new(),
                    grants: Vec::new(),
                    metadata: None,
                })
                .await
                .expect("create");
            let node = Node::message(Message::user("persisted"), None);
            store
                .commit(Commit {
                    session_id,
                    expected_version: Some(1),
                    nodes: vec![node.clone()],
                    head: Some(node.id),
                    phase: Phase::Idle,
                    turn: 1,
                    queue: VecDeque::new(),
                    grants: Vec::new(),
                    metadata: None,
                })
                .await
                .expect("append");
        }

        let store = SqliteStore::open(&path).expect("reopen");
        let session = store.load(session_id).await.expect("load");
        assert_eq!(session.version, 2);
        let history = store.history(session.head).await.expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].text(), "persisted");

        let record = ApprovalRecord::new(
            session_id,
            CallId::new(),
            ToolName::new("read_file").expect("name"),
            &serde_json::json!({"path": "a"}),
            chrono::Duration::minutes(5),
        );
        store.save_approval(record.clone()).await.expect("approval");
        let reopened = SqliteStore::open(&path).expect("reopen again");
        let loaded = reopened
            .approval(record.id)
            .await
            .expect("load approval")
            .expect("present");
        assert_eq!(loaded, record);

        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn the_directory_round_trips_sessions_and_questions() {
        let store = SqliteStore::in_memory().expect("open");
        let session = StoredSession {
            key: "abc".to_owned(),
            north: SessionId::new(),
            title: Some("first".to_owned()),
            model: "openai/gpt".to_owned(),
            created_at: Utc::now(),
            phase: "idle".to_owned(),
            stats: serde_json::json!({"cost": 0.5}),
        };
        store.save_session(&session).await.expect("save");
        assert_eq!(store.sessions().await.expect("list"), vec![session.clone()]);

        let question = StoredQuestion {
            id: "q0".to_owned(),
            session: session.north,
            call_id: "call-1".to_owned(),
            question: "Which file?".to_owned(),
            answer: None,
        };
        store.save_question(&question).await.expect("question");
        assert!(
            store
                .answer_question("call-1", "src/lib.rs")
                .await
                .expect("answer")
        );
        assert!(
            !store
                .answer_question("call-1", "again")
                .await
                .expect("answer")
        );
        let loaded = store
            .question("call-1")
            .await
            .expect("load question")
            .expect("present");
        assert_eq!(loaded.answer.as_deref(), Some("src/lib.rs"));

        store.delete_session("abc").await.expect("delete");
        assert!(store.sessions().await.expect("list").is_empty());
    }
}
