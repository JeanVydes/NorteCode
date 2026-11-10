//! A `SQLite` [`norte_code_protocol::Store`] and [`norte::BlobStore`] for Norte Code.
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
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::costs::CostRecord;
use crate::store::{Commit, Node, Session, Store};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use norte::{BlobId, BlobStore, CallId, Error, NodeId, Result, SessionId};
use norte_code_wire::Phase;
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
    /// The provider that served the session.
    ///
    /// It has a column of its own: a provider and a model are two facts, and a
    /// row that carried both in one string had to be split again to be read.
    #[serde(default)]
    pub provider: String,
    /// The model id the session last used.
    pub model: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// The last phase Norte Code observed, as its protocol names it.
    pub phase: Phase,
    /// Host counters (billed messages, reported cost), opaque to the store.
    #[serde(default)]
    pub stats: Value,
    /// Tools this session refuses without asking again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_tools: Vec<String>,
    /// Tools this session allows without asking again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// The Norte session this one was compacted from, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compacted_from: Option<SessionId>,
    /// The directory the session was working in, when the host recorded one.
    ///
    /// A session is about a tree: a path it read, a file it wrote, the project
    /// instructions it was given. Resuming one from another directory was
    /// resuming it somewhere the work does not apply — the tools re-root at the
    /// process's own directory, so a resumed session silently read the wrong
    /// files. The host stores where it was and goes back there.
    ///
    /// It is optional and it is in the blob rather than in a column: a session
    /// stored before this field existed has no directory, and a schema a reader
    /// has to migrate is a schema every reader has to keep a copy of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

/// How one request ended.
///
/// It is an enum and not a word because a reader that matched on the word had a
/// catch-all arm, so a status this build did not know was counted as an error —
/// silently, and wrongly. A row written by a newer build is a parse error that
/// names the variant, which is a thing to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    /// The provider answered.
    Ok,
    /// The request failed.
    Error,
    /// The caller stopped it.
    Cancelled,
}

impl RequestStatus {
    /// Reads the word the column holds.
    ///
    /// An unknown word is an error and not a default: a row written by a newer
    /// build is a thing to report, and counting it as an error — which is what
    /// a catch-all arm did — is a wrong number nobody can trace.
    ///
    /// # Errors
    ///
    /// Returns a message naming the row and the word when the word is not one
    /// this build knows.
    fn parse(word: &str, what: &str) -> std::result::Result<Self, Fault> {
        match word {
            "ok" => Ok(Self::Ok),
            "error" => Ok(Self::Error),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(Fault::Corrupt {
                what: format!("the status of {what}"),
                reason: format!("`{other}` is not a status this build knows"),
            }),
        }
    }

    /// The word the column holds.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for RequestStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One provider request, with everything a local telemetry report needs.
///
/// A request row is written even when the provider fails, so the report counts
/// attempts rather than only billed answers. Token and cost fields stay
/// optional: `None` means the provider did not report that counter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestRecord {
    /// A stable request id.
    pub id: String,
    /// The logical session key.
    pub session: String,
    /// The Norte session the request belongs to.
    pub north_session: String,
    /// The turn inside that session.
    pub turn: u64,
    /// The provider id.
    pub provider: String,
    /// The model id.
    pub model: String,
    /// The base `URL` the request went to, when the provider declares one.
    ///
    /// A column rather than a metadata key because it is grouped and filtered
    /// on: one provider id can serve from more than one endpoint, and a latency
    /// change is worth attributing to one of them.
    pub endpoint: Option<String>,
    /// When the request started.
    pub started_at: DateTime<Utc>,
    /// How long the request took, including streaming.
    pub duration_ms: u64,
    /// Time to the first answer delta, when one arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    /// Time to the last answer delta, when one arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_token_ms: Option<u64>,
    /// Prompt tokens the provider reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Answer tokens the provider reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Reasoning tokens the provider reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Prompt tokens served from the provider cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Prompt tokens written to the provider cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// A local bytes/4 estimate, used only when usage is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_input_tokens: Option<u64>,
    /// Whether the provider served any prompt token from cache.
    pub cache_hit: bool,
    /// Tools the model called in this request, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<String>,
    /// The reasoning setting the request carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Whether the command sandbox was enabled for this request's run.
    #[serde(default)]
    pub sandbox: bool,
    /// Whether that sandbox allowed network access.
    #[serde(default)]
    pub sandbox_network: bool,
    /// The generation parameters the request carried.
    #[serde(default)]
    pub params: Value,
    /// The vendor extensions the request carried.
    #[serde(default)]
    pub extensions: Value,
    /// Anything else the host measured.
    #[serde(default)]
    pub metadata: Value,
    /// The cost the provider reported, in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Why generation stopped, as the wire named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// How the request ended.
    pub status: RequestStatus,
    /// The provider or transport error, when the request failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The kind of provider failure, when the request failed.
    ///
    /// The typed half of `error`: a report should count rate limits without
    /// parsing messages.
    pub error_kind: Option<String>,
    /// How long the provider asked the caller to wait before retrying.
    pub retry_after_ms: Option<u64>,
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
    /// Closed choices, when the model offered any.
    #[serde(default)]
    pub options: Vec<String>,
    /// Whether more than one option may be chosen.
    #[serde(default)]
    pub multiple: bool,
    /// The answer, once a human gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// A tool call that started but never recorded finishing.
///
/// At startup these are the calls a crash may have interrupted. A journaled
/// tool is not run again with the same call id, because its effect may already
/// have happened while Norte only guarantees commit-at-most-once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEffect {
    /// The session the call belongs to.
    pub session: SessionId,
    /// The call that started.
    pub call_id: CallId,
    /// The tool that started.
    pub tool: String,
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
    /// Removes one session and everything the database holds for it.
    ///
    /// The key, the Norte session it names, its node tree, and the cost,
    /// telemetry, effect and question rows written under either the
    /// key or the session id. Missing keys are not an error. It is one
    /// transaction, because a delete that removes the key and leaves the rows
    /// behind is a database that grows a little with every session forgotten.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn delete_session(&self, key: &str) -> Result<()>;
    /// Marks stored phases that cannot survive a process as idle.
    ///
    /// `streaming` describes a live turn; a hard kill leaves it behind, and no
    /// process is streaming that session after a restart. Returns how many rows
    /// changed.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn repair_interrupted(&self) -> Result<usize>;
    /// Records one model request in the cost ledger.
    ///
    /// The key is `(session, index)`, so recording the same request twice
    /// replaces its row rather than duplicating the spend.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn record_cost(&self, record: &CostRecord) -> Result<()>;
    /// Every ledger row, optionally only one session's, oldest first.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn costs(&self, session: Option<&str>) -> Result<Vec<CostRecord>>;
    /// Records one provider request for local telemetry.
    ///
    /// The id is the key, so replaying the same request replaces its row.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn record_request(&self, record: &RequestRecord) -> Result<()>;
    /// Every telemetry request, optionally only one session's, oldest first.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn requests(&self, session: Option<&str>) -> Result<Vec<RequestRecord>>;
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
    /// Records that a journaled tool is about to run.
    ///
    /// Synchronous because the engine calls it from the tool-metadata callback,
    /// immediately before the executor, with no await point in between.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    fn effect_started(
        &self,
        session: SessionId,
        call_id: CallId,
        tool: &str,
        epoch: &str,
    ) -> Result<()>;
    /// Records that a journaled tool returned. A row that was never started is
    /// left alone (a denied call never reaches the metadata callback).
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    fn effect_finished(&self, session: SessionId, call_id: CallId, is_error: bool) -> Result<()>;
    /// Every call that started and has no recorded finish, from any process.
    ///
    /// # Errors
    ///
    /// Propagates a database failure.
    async fn unfinished_effects(&self) -> Result<Vec<PendingEffect>>;
}

/// A directory that keeps everything in memory, for tests and one-shot runs.
#[derive(Debug, Default)]
pub struct InMemoryDirectory {
    sessions: Mutex<BTreeMap<String, StoredSession>>,
    questions: Mutex<BTreeMap<String, StoredQuestion>>,
    effects: Mutex<BTreeMap<(SessionId, CallId), EffectEntry>>,
    costs: Mutex<BTreeMap<(String, u64), CostRecord>>,
    requests: Mutex<BTreeMap<String, RequestRecord>>,
}

#[derive(Debug)]
struct EffectEntry {
    tool: String,
    done: bool,
    #[allow(dead_code)]
    is_error: Option<bool>,
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

    async fn repair_interrupted(&self) -> Result<usize> {
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        let mut repaired = 0;
        for session in sessions.values_mut() {
            if session.phase == Phase::Streaming {
                session.phase = Phase::Idle;
                repaired += 1;
            }
        }
        Ok(repaired)
    }

    async fn record_cost(&self, record: &CostRecord) -> Result<()> {
        self.costs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((record.session.clone(), record.index), record.clone());
        Ok(())
    }

    async fn costs(&self, session: Option<&str>) -> Result<Vec<CostRecord>> {
        let costs = self.costs.lock().unwrap_or_else(PoisonError::into_inner);
        let mut records: Vec<CostRecord> = costs
            .values()
            .filter(|record| session.is_none_or(|key| record.session == key))
            .cloned()
            .collect();
        records.sort_by(|left, right| {
            (left.recorded_at, left.session.as_str(), left.index).cmp(&(
                right.recorded_at,
                right.session.as_str(),
                right.index,
            ))
        });
        Ok(records)
    }

    async fn record_request(&self, record: &RequestRecord) -> Result<()> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(record.id.clone(), record.clone());
        Ok(())
    }

    async fn requests(&self, session: Option<&str>) -> Result<Vec<RequestRecord>> {
        let requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
        let mut records: Vec<RequestRecord> = requests
            .values()
            .filter(|record| session.is_none_or(|key| record.session == key))
            .cloned()
            .collect();
        records.sort_by(|left, right| {
            (left.started_at, left.id.as_str()).cmp(&(right.started_at, right.id.as_str()))
        });
        Ok(records)
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
                stored.options.clone_from(&question.options);
                stored.multiple = question.multiple;
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

    fn effect_started(
        &self,
        session: SessionId,
        call_id: CallId,
        tool: &str,
        _epoch: &str,
    ) -> Result<()> {
        self.effects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                (session, call_id),
                EffectEntry {
                    tool: tool.to_owned(),
                    done: false,
                    is_error: None,
                },
            );
        Ok(())
    }

    fn effect_finished(&self, session: SessionId, call_id: CallId, is_error: bool) -> Result<()> {
        if let Some(entry) = self
            .effects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&(session, call_id))
        {
            entry.done = true;
            entry.is_error = Some(is_error);
        }
        Ok(())
    }

    async fn unfinished_effects(&self) -> Result<Vec<PendingEffect>> {
        Ok(self
            .effects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, entry)| !entry.done)
            .map(|((session, call_id), entry)| PendingEffect {
                session: *session,
                call_id: *call_id,
                tool: entry.tool.clone(),
            })
            .collect())
    }
}

/// A `SQLite` store: Norte's graph and blobs, plus host tables.
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
        if let Some(parent) = path.as_ref().parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
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
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL keeps readers from blocking the single writer; it is a no-op for
        // an in-memory database.
        let _ = connection.pragma_update(None, "journal_mode", "WAL");
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.execute_batch(SCHEMA)?;
        ensure_column(
            &connection,
            "questions",
            "options",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        ensure_column(
            &connection,
            "questions",
            "multiple",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(&connection, "requests", "last_token_ms", "INTEGER")?;
        ensure_column(
            &connection,
            "requests",
            "sandbox",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &connection,
            "requests",
            "sandbox_network",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &connection,
            "logical_sessions",
            "provider",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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

/// Adds a column to a table created by an older schema.
fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|name| name == column) {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
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
CREATE TABLE IF NOT EXISTS blobs (
    id    TEXT PRIMARY KEY,
    bytes BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS logical_sessions (
    key        TEXT PRIMARY KEY,
    north      TEXT NOT NULL,
    title      TEXT,
    provider   TEXT NOT NULL DEFAULT '',
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
    options  TEXT NOT NULL DEFAULT '[]',
    multiple INTEGER NOT NULL DEFAULT 0,
    answer   TEXT
);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS effects (
    session    TEXT NOT NULL,
    call_id    TEXT NOT NULL,
    tool       TEXT NOT NULL,
    status     TEXT NOT NULL,
    epoch      TEXT NOT NULL,
    is_error   INTEGER,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (session, call_id)
);
CREATE TABLE IF NOT EXISTS costs (
    session             TEXT NOT NULL,
    request_index       INTEGER NOT NULL,
    turn                INTEGER NOT NULL,
    model               TEXT NOT NULL,
    input_tokens        INTEGER,
    output_tokens       INTEGER,
    reasoning_tokens    INTEGER,
    cache_read_tokens   INTEGER,
    cache_write_tokens  INTEGER,
    cost_usd            REAL,
    recorded_at         TEXT NOT NULL,
    PRIMARY KEY (session, request_index)
);
CREATE INDEX IF NOT EXISTS costs_recorded_at ON costs(recorded_at);
CREATE INDEX IF NOT EXISTS costs_session ON costs(session);
CREATE TABLE IF NOT EXISTS requests (
    id                      TEXT PRIMARY KEY,
    session                 TEXT NOT NULL,
    north_session           TEXT NOT NULL,
    turn                    INTEGER NOT NULL,
    provider                TEXT NOT NULL,
    model                   TEXT NOT NULL,
    endpoint                TEXT,
    started_at              TEXT NOT NULL,
    duration_ms             INTEGER NOT NULL,
    first_token_ms          INTEGER,
    last_token_ms           INTEGER,
    input_tokens            INTEGER,
    output_tokens           INTEGER,
    reasoning_tokens        INTEGER,
    cache_read_tokens       INTEGER,
    cache_write_tokens      INTEGER,
    estimated_input_tokens  INTEGER,
    cache_hit               INTEGER NOT NULL,
    tool_calls              TEXT NOT NULL,
    reasoning               TEXT,
    params                  TEXT NOT NULL,
    extensions              TEXT NOT NULL,
    metadata                TEXT NOT NULL,
    cost_usd                REAL,
    finish_reason           TEXT,
    status                  TEXT NOT NULL,
    error                   TEXT,
    error_kind              TEXT,
    retry_after_ms          INTEGER,
    sandbox                 INTEGER NOT NULL DEFAULT 0,
    sandbox_network         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS requests_started_at ON requests(started_at);
CREATE INDEX IF NOT EXISTS requests_session ON requests(session);
CREATE INDEX IF NOT EXISTS requests_provider ON requests(provider);
CREATE INDEX IF NOT EXISTS requests_model ON requests(model);
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

fn sqlite_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn rust_u64(value: i64, what: &str) -> Result<u64> {
    u64::try_from(value).map_err(|error| {
        Error::from(Fault::Corrupt {
            what: what.to_owned(),
            reason: error.to_string(),
        })
    })
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

/// A session as stored.
fn encode_session(session: &Session) -> std::result::Result<String, Fault> {
    encode(session, "session")
}

/// A session as stored: either a bare session or a legacy envelope from when
/// grants were stored beside the session.
#[derive(Deserialize)]
struct SessionEnvelope {
    session: Session,
}

fn decode_session(text: &str) -> std::result::Result<Session, Fault> {
    if let Ok(envelope) = decode::<SessionEnvelope>(text, "session envelope") {
        return Ok(envelope.session);
    }
    decode::<Session>(text, "session")
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
#[allow(clippy::too_many_lines)]
impl SessionDirectory for SqliteStore {
    async fn sessions(&self) -> Result<Vec<StoredSession>> {
        let rows: Vec<(
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            String,
            String,
            String,
        )> = self
            .run(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT key, north, title, provider, model, created_at, phase, stats, data
                         FROM logical_sessions ORDER BY created_at DESC",
                    )
                    .map_err(Fault::from)?;
                let mapped = statement
                    .query_map([], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                        ))
                    })
                    .map_err(Fault::from)?;
                mapped
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(Fault::from)
            })
            .await?;
        rows.into_iter()
            .map(
                |(key, north, title, provider, model, created_at, phase, stats, data)| {
                    let extra: StoredSession = decode(&data, "stored session")?;
                    Ok(StoredSession {
                        key,
                        north: north.parse()?,
                        title,
                        provider,
                        model,
                        created_at: created_at.parse().map_err(|error: chrono::ParseError| {
                            Fault::Corrupt {
                                what: "stored session".to_owned(),
                                reason: error.to_string(),
                            }
                        })?,
                        phase: match phase.as_str() {
                            "streaming" => Phase::Streaming,
                            "awaiting_tool" => Phase::AwaitingTool,
                            _ => Phase::Idle,
                        },
                        stats: decode(&stats, "session stats")?,
                        denied_tools: extra.denied_tools,
                        allowed_tools: extra.allowed_tools,
                        compacted_from: extra.compacted_from,
                        workdir: extra.workdir,
                    })
                },
            )
            .collect()
    }

    async fn save_session(&self, session: &StoredSession) -> Result<()> {
        let session = session.clone();
        self.run(move |connection| {
            let stats = encode(&session.stats, "session stats")?;
            let data = encode(&session, "stored session")?;
            connection
                .execute(
                    "INSERT INTO logical_sessions (key, north, title, provider, model, created_at, phase, stats, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(key) DO UPDATE SET north = excluded.north, title = excluded.title,
                       provider = excluded.provider, model = excluded.model,
                       created_at = excluded.created_at,
                       phase = excluded.phase, stats = excluded.stats, data = excluded.data",
                    params![
                        session.key,
                        session.north.to_string(),
                        session.title,
                        session.provider,
                        session.model,
                        session.created_at.to_rfc3339(),
                        session.phase.to_string(),
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
        self.transact(move |transaction| {
            // The row that names the Norte session: the key alone does not say
            // which one it is, and everything else here is keyed by that id.
            let north: Option<String> = transaction
                .query_row(
                    "SELECT north FROM logical_sessions WHERE key = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Fault::from)?;
            transaction
                .execute("DELETE FROM logical_sessions WHERE key = ?1", params![key])
                .map_err(Fault::from)?;
            if let Some(north) = north {
                // The conversation is the ancestor chain of the session's head,
                // which is the only way the node tree is reachable: nodes carry
                // no session of their own, a parent is the message before them.
                // A session that was compacted has all of its nodes under the
                // current head, so walking back from it takes the whole tree.
                let head: Option<String> = transaction
                    .query_row(
                        "SELECT data FROM sessions WHERE id = ?1",
                        params![north],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(Fault::from)?
                    .and_then(|data| {
                        decode::<serde_json::Value>(&data, "session")
                            .ok()
                            .and_then(|value| {
                                value
                                    .get("head")
                                    .or_else(|| value.get("session").and_then(|s| s.get("head")))
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned)
                            })
                    });
                if let Some(head) = head {
                    transaction
                        .execute(
                            "WITH RECURSIVE chain(id, parent) AS (
                                 SELECT id, parent FROM nodes WHERE id = ?1
                                 UNION
                                 SELECT n.id, n.parent FROM nodes n JOIN chain c ON n.id = c.parent
                             )
                             DELETE FROM nodes WHERE id IN (SELECT id FROM chain)",
                            params![head],
                        )
                        .map_err(Fault::from)?;
                }
                // Every table that outlives a session keeps its own row, and a
                // session deleted from one alone is a key that names nothing: a
                // cost ledger, a telemetry row or an effect still pointing at a
                // session the list no longer has is what made a delete look
                // partial. `session` holds the id in some tables and the key in
                // others, so both spellings are cleared for the same session.
                for table in ["costs", "requests", "effects", "questions"] {
                    transaction
                        .execute(
                            &format!("DELETE FROM {table} WHERE session IN (?1, ?2)"),
                            params![key, north],
                        )
                        .map_err(Fault::from)?;
                }
                transaction
                    .execute("DELETE FROM sessions WHERE id = ?1", params![north])
                    .map_err(Fault::from)?;
            }
            Ok(())
        })
        .await
    }

    async fn repair_interrupted(&self) -> Result<usize> {
        self.run(|connection| {
            connection
                .execute(
                    "UPDATE logical_sessions SET phase = 'idle' WHERE phase = 'streaming'",
                    [],
                )
                .map_err(Fault::from)
        })
        .await
    }

    async fn record_cost(&self, record: &CostRecord) -> Result<()> {
        let record = record.clone();
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO costs (
                         session, request_index, turn, model, input_tokens, output_tokens,
                         reasoning_tokens, cache_read_tokens, cache_write_tokens, cost_usd,
                         recorded_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(session, request_index) DO UPDATE SET
                         turn = excluded.turn,
                         model = excluded.model,
                         input_tokens = excluded.input_tokens,
                         output_tokens = excluded.output_tokens,
                         reasoning_tokens = excluded.reasoning_tokens,
                         cache_read_tokens = excluded.cache_read_tokens,
                         cache_write_tokens = excluded.cache_write_tokens,
                         cost_usd = excluded.cost_usd,
                         recorded_at = excluded.recorded_at",
                    params![
                        record.session,
                        sqlite_u64(record.index),
                        sqlite_u64(record.turn),
                        record.model,
                        record.input_tokens.map(sqlite_u64),
                        record.output_tokens.map(sqlite_u64),
                        record.reasoning_tokens.map(sqlite_u64),
                        record.cache_read_tokens.map(sqlite_u64),
                        record.cache_write_tokens.map(sqlite_u64),
                        record.cost_usd,
                        record.recorded_at.to_rfc3339(),
                    ],
                )
                .map_err(Fault::from)?;
            Ok(())
        })
        .await
    }

    async fn costs(&self, session: Option<&str>) -> Result<Vec<CostRecord>> {
        let session = session.map(str::to_owned);
        let rows: Vec<(
            String,
            i64,
            i64,
            String,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<f64>,
            String,
        )> = self
            .run(move |connection| {
                let sql = if session.is_some() {
                    "SELECT session, request_index, turn, model, input_tokens, output_tokens,
                            reasoning_tokens, cache_read_tokens, cache_write_tokens, cost_usd,
                            recorded_at
                     FROM costs WHERE session = ?1 ORDER BY recorded_at, request_index"
                } else {
                    "SELECT session, request_index, turn, model, input_tokens, output_tokens,
                            reasoning_tokens, cache_read_tokens, cache_write_tokens, cost_usd,
                            recorded_at
                     FROM costs ORDER BY recorded_at, session, request_index"
                };
                let mut statement = connection.prepare(sql).map_err(Fault::from)?;
                let map = |row: &rusqlite::Row<'_>| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    ))
                };
                let mapped = if let Some(session) = session.as_deref() {
                    statement
                        .query_map(params![session], map)
                        .map_err(Fault::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(Fault::from)?
                } else {
                    statement
                        .query_map([], map)
                        .map_err(Fault::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(Fault::from)?
                };
                Ok(mapped)
            })
            .await?;
        rows.into_iter()
            .map(|row| {
                let (
                    session,
                    index,
                    turn,
                    model,
                    input_tokens,
                    output_tokens,
                    reasoning_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    cost_usd,
                    recorded_at,
                ) = row;
                Ok(CostRecord {
                    session,
                    turn: rust_u64(turn, "cost turn")?,
                    index: rust_u64(index, "cost request index")?,
                    model,
                    input_tokens: input_tokens
                        .map(|value| rust_u64(value, "input tokens"))
                        .transpose()?,
                    output_tokens: output_tokens
                        .map(|value| rust_u64(value, "output tokens"))
                        .transpose()?,
                    reasoning_tokens: reasoning_tokens
                        .map(|value| rust_u64(value, "reasoning tokens"))
                        .transpose()?,
                    cache_read_tokens: cache_read_tokens
                        .map(|value| rust_u64(value, "cache read tokens"))
                        .transpose()?,
                    cache_write_tokens: cache_write_tokens
                        .map(|value| rust_u64(value, "cache write tokens"))
                        .transpose()?,
                    cost_usd,
                    recorded_at: recorded_at.parse().map_err(|error: chrono::ParseError| {
                        Fault::Corrupt {
                            what: "cost record".to_owned(),
                            reason: error.to_string(),
                        }
                    })?,
                })
            })
            .collect()
    }

    async fn record_request(&self, record: &RequestRecord) -> Result<()> {
        let record = record.clone();
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO requests (
                         id, session, north_session, turn, provider, model, endpoint, started_at,
                         duration_ms, first_token_ms, last_token_ms, input_tokens, output_tokens,
                         reasoning_tokens, cache_read_tokens, cache_write_tokens,
                         estimated_input_tokens, cache_hit, tool_calls, reasoning,
                         params, extensions, metadata, cost_usd, finish_reason, status, error,
                         error_kind, retry_after_ms, sandbox, sandbox_network
                     ) VALUES (
                         ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                         ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27,
                         ?28, ?29, ?30, ?31
                     )
                     ON CONFLICT(id) DO UPDATE SET
                         session = excluded.session,
                         north_session = excluded.north_session,
                         turn = excluded.turn,
                         provider = excluded.provider,
                         model = excluded.model,
                         endpoint = excluded.endpoint,
                         started_at = excluded.started_at,
                         duration_ms = excluded.duration_ms,
                         first_token_ms = excluded.first_token_ms,
                         last_token_ms = excluded.last_token_ms,
                         input_tokens = excluded.input_tokens,
                         output_tokens = excluded.output_tokens,
                         reasoning_tokens = excluded.reasoning_tokens,
                         cache_read_tokens = excluded.cache_read_tokens,
                         cache_write_tokens = excluded.cache_write_tokens,
                         estimated_input_tokens = excluded.estimated_input_tokens,
                         cache_hit = excluded.cache_hit,
                         tool_calls = excluded.tool_calls,
                         reasoning = excluded.reasoning,
                         params = excluded.params,
                         extensions = excluded.extensions,
                         metadata = excluded.metadata,
                         cost_usd = excluded.cost_usd,
                         finish_reason = excluded.finish_reason,
                         status = excluded.status,
                         error = excluded.error,
                         error_kind = excluded.error_kind,
                         retry_after_ms = excluded.retry_after_ms,
                         sandbox = excluded.sandbox,
                         sandbox_network = excluded.sandbox_network",
                    params![
                        record.id,
                        record.session,
                        record.north_session,
                        sqlite_u64(record.turn),
                        record.provider,
                        record.model,
                        record.endpoint,
                        record.started_at.to_rfc3339(),
                        sqlite_u64(record.duration_ms),
                        record.first_token_ms.map(sqlite_u64),
                        record.last_token_ms.map(sqlite_u64),
                        record.input_tokens.map(sqlite_u64),
                        record.output_tokens.map(sqlite_u64),
                        record.reasoning_tokens.map(sqlite_u64),
                        record.cache_read_tokens.map(sqlite_u64),
                        record.cache_write_tokens.map(sqlite_u64),
                        record.estimated_input_tokens.map(sqlite_u64),
                        i64::from(record.cache_hit),
                        encode(&record.tool_calls, "request tool calls")?,
                        record.reasoning,
                        encode(&record.params, "request params")?,
                        encode(&record.extensions, "request extensions")?,
                        encode(&record.metadata, "request metadata")?,
                        record.cost_usd,
                        record.finish_reason,
                        record.status.as_str(),
                        record.error,
                        record.error_kind,
                        record.retry_after_ms.map(sqlite_u64),
                        i64::from(record.sandbox),
                        i64::from(record.sandbox_network),
                    ],
                )
                .map_err(Fault::from)?;
            Ok(())
        })
        .await
    }

    async fn requests(&self, session: Option<&str>) -> Result<Vec<RequestRecord>> {
        let session = session.map(str::to_owned);
        let rows: Vec<(
            String,         // id
            String,         // session
            String,         // north_session
            i64,            // turn
            String,         // provider
            String,         // model
            Option<String>, // endpoint
            String,         // started_at
            i64,            // duration_ms
            Option<i64>,    // first_token_ms
            Option<i64>,    // last_token_ms
            Option<i64>,    // input_tokens
            Option<i64>,    // output_tokens
            Option<i64>,    // reasoning_tokens
            Option<i64>,    // cache_read_tokens
            Option<i64>,    // cache_write_tokens
            Option<i64>,    // estimated_input_tokens
            i64,            // cache_hit
            String,         // tool_calls
            Option<String>, // reasoning
            String,         // params
            String,         // extensions
            String,         // metadata
            Option<f64>,    // cost_usd
            Option<String>, // finish_reason
            String,         // status
            Option<String>, // error
            Option<String>, // error_kind
            Option<i64>,    // retry_after_ms
            i64,            // sandbox
            i64,            // sandbox_network
        )> = self
            .run(move |connection| {
                let sql = if session.is_some() {
                    "SELECT id, session, north_session, turn, provider, model, endpoint, started_at,
                            duration_ms, first_token_ms, last_token_ms, input_tokens, output_tokens,
                            reasoning_tokens, cache_read_tokens, cache_write_tokens,
                            estimated_input_tokens, cache_hit, tool_calls, reasoning,
                            params, extensions, metadata, cost_usd, finish_reason, status, error,
                            error_kind, retry_after_ms, sandbox, sandbox_network
                     FROM requests WHERE session = ?1 ORDER BY started_at, id"
                } else {
                    "SELECT id, session, north_session, turn, provider, model, endpoint, started_at,
                            duration_ms, first_token_ms, last_token_ms, input_tokens, output_tokens,
                            reasoning_tokens, cache_read_tokens, cache_write_tokens,
                            estimated_input_tokens, cache_hit, tool_calls, reasoning,
                            params, extensions, metadata, cost_usd, finish_reason, status, error,
                            error_kind, retry_after_ms, sandbox, sandbox_network
                     FROM requests ORDER BY started_at, id"
                };
                let mut statement = connection.prepare(sql).map_err(Fault::from)?;
                let map = |row: &rusqlite::Row<'_>| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                        row.get(13)?,
                        row.get(14)?,
                        row.get(15)?,
                        row.get(16)?,
                        row.get(17)?,
                        row.get(18)?,
                        row.get(19)?,
                        row.get(20)?,
                        row.get(21)?,
                        row.get(22)?,
                        row.get(23)?,
                        row.get(24)?,
                        row.get(25)?,
                        row.get(26)?,
                        row.get(27)?,
                        row.get(28)?,
                        row.get(29)?,
                        row.get(30)?,
                    ))
                };
                let mapped = if let Some(session) = session.as_deref() {
                    statement
                        .query_map(params![session], map)
                        .map_err(Fault::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(Fault::from)?
                } else {
                    statement
                        .query_map([], map)
                        .map_err(Fault::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(Fault::from)?
                };
                Ok(mapped)
            })
            .await?;
        rows.into_iter()
            .map(|row| {
                let (
                    id,
                    session,
                    north_session,
                    turn,
                    provider,
                    model,
                    endpoint,
                    started_at,
                    duration_ms,
                    first_token_ms,
                    last_token_ms,
                    input_tokens,
                    output_tokens,
                    reasoning_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    estimated_input_tokens,
                    cache_hit,
                    tool_calls,
                    reasoning,
                    params,
                    extensions,
                    metadata,
                    cost_usd,
                    finish_reason,
                    status,
                    error,
                    error_kind,
                    retry_after_ms,
                    sandbox,
                    sandbox_network,
                ) = row;
                let status = RequestStatus::parse(&status, &id)?;
                let token = |value: Option<i64>, what: &str| {
                    value.map(|value| rust_u64(value, what)).transpose()
                };
                Ok(RequestRecord {
                    id,
                    session,
                    north_session,
                    turn: rust_u64(turn, "request turn")?,
                    provider,
                    model,
                    endpoint,
                    started_at: started_at.parse().map_err(|error: chrono::ParseError| {
                        Fault::Corrupt {
                            what: "request started_at".to_owned(),
                            reason: error.to_string(),
                        }
                    })?,
                    duration_ms: rust_u64(duration_ms, "request duration")?,
                    first_token_ms: token(first_token_ms, "first token")?,
                    last_token_ms: token(last_token_ms, "last token")?,
                    input_tokens: token(input_tokens, "input tokens")?,
                    output_tokens: token(output_tokens, "output tokens")?,
                    reasoning_tokens: token(reasoning_tokens, "reasoning tokens")?,
                    cache_read_tokens: token(cache_read_tokens, "cache read tokens")?,
                    cache_write_tokens: token(cache_write_tokens, "cache write tokens")?,
                    estimated_input_tokens: token(
                        estimated_input_tokens,
                        "estimated input tokens",
                    )?,
                    cache_hit: cache_hit != 0,
                    tool_calls: decode(&tool_calls, "request tool calls")?,
                    reasoning,
                    sandbox: sandbox != 0,
                    sandbox_network: sandbox_network != 0,
                    params: decode(&params, "request params")?,
                    extensions: decode(&extensions, "request extensions")?,
                    metadata: decode(&metadata, "request metadata")?,
                    cost_usd,
                    finish_reason,
                    status,
                    error,
                    error_kind,
                    retry_after_ms: retry_after_ms
                        .map(|value| rust_u64(value, "retry after"))
                        .transpose()?,
                })
            })
            .collect()
    }

    async fn question(&self, call_id: &str) -> Result<Option<StoredQuestion>> {
        let call_id = call_id.to_owned();
        let row: Option<(String, String, String, String, String, i64, Option<String>)> = self
            .run(move |connection| {
                connection
                    .query_row(
                        "SELECT id, session, call_id, question, options, multiple, answer
                         FROM questions WHERE call_id = ?1",
                        params![call_id],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                                row.get(6)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(Fault::from)
            })
            .await?;
        row.map(
            |(id, session, call_id, question, options, multiple, answer)| {
                let session = session
                    .parse::<SessionId>()
                    .map_err(|error: Error| Error::Other(error.to_string()))?;
                Ok::<_, Error>(StoredQuestion {
                    id,
                    session,
                    call_id,
                    question,
                    options: decode(&options, "question options")?,
                    multiple: multiple != 0,
                    answer,
                })
            },
        )
        .transpose()
    }

    async fn save_question(&self, question: &StoredQuestion) -> Result<()> {
        let question = question.clone();
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO questions
                       (call_id, id, session, question, options, multiple, answer)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(call_id) DO UPDATE SET id = excluded.id,
                       session = excluded.session, question = excluded.question,
                       options = excluded.options, multiple = excluded.multiple,
                       answer = COALESCE(questions.answer, excluded.answer)",
                    params![
                        question.call_id,
                        question.id,
                        question.session.to_string(),
                        question.question,
                        encode(&question.options, "question options")?,
                        i64::from(question.multiple),
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

    fn effect_started(
        &self,
        session: SessionId,
        call_id: CallId,
        tool: &str,
        epoch: &str,
    ) -> Result<()> {
        let connection = self.lock();
        connection
            .execute(
                "INSERT INTO effects (session, call_id, tool, status, epoch, is_error, updated_at)
                 VALUES (?1, ?2, ?3, 'intent', ?4, NULL, ?5)
                 ON CONFLICT(session, call_id) DO UPDATE SET tool = excluded.tool,
                   status = 'intent', epoch = excluded.epoch, is_error = NULL,
                   updated_at = excluded.updated_at",
                params![
                    session.to_string(),
                    call_id.to_string(),
                    tool,
                    epoch,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(Fault::from)?;
        Ok(())
    }

    fn effect_finished(&self, session: SessionId, call_id: CallId, is_error: bool) -> Result<()> {
        let connection = self.lock();
        connection
            .execute(
                "UPDATE effects SET status = 'done', is_error = ?3, updated_at = ?4
                 WHERE session = ?1 AND call_id = ?2",
                params![
                    session.to_string(),
                    call_id.to_string(),
                    is_error,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(Fault::from)?;
        Ok(())
    }

    async fn unfinished_effects(&self) -> Result<Vec<PendingEffect>> {
        let rows: Vec<(String, String, String)> = self
            .run(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT session, call_id, tool FROM effects
                         WHERE status = 'intent' ORDER BY updated_at",
                    )
                    .map_err(Fault::from)?;
                let mapped = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .map_err(Fault::from)?;
                mapped
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(Fault::from)
            })
            .await?;
        rows.into_iter()
            .map(|(session, call_id, tool)| {
                Ok(PendingEffect {
                    session: session.parse()?,
                    call_id: call_id.parse()?,
                    tool,
                })
            })
            .collect()
    }
}

/// The transport shape of a stored request row.
///
/// The event that carries telemetry is on the wire, so it holds the wire's
/// record; the store keeps this crate's row, with a parsed timestamp and a
/// status the database can query. One conversion, in one place, keeps the two
/// from drifting.
impl From<&RequestRecord> for norte_code_wire::TelemetryRecord {
    fn from(row: &RequestRecord) -> Self {
        Self {
            id: row.id.clone(),
            session: row.session.clone(),
            north_session: row.north_session.clone(),
            turn: row.turn,
            provider: row.provider.clone(),
            model: row.model.clone(),
            endpoint: row.endpoint.clone(),
            started_at: row.started_at.to_rfc3339(),
            duration_ms: row.duration_ms,
            first_token_ms: row.first_token_ms,
            last_token_ms: row.last_token_ms,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            reasoning_tokens: row.reasoning_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_write_tokens,
            estimated_input_tokens: row.estimated_input_tokens,
            cache_hit: row.cache_hit,
            tool_calls: row.tool_calls.clone(),
            reasoning: row.reasoning.clone(),
            sandbox: row.sandbox,
            sandbox_network: row.sandbox_network,
            params: row.params.clone(),
            extensions: row.extensions.clone(),
            metadata: row.metadata.clone(),
            cost_usd: row.cost_usd,
            finish_reason: row.finish_reason.clone(),
            status: match row.status {
                RequestStatus::Ok => norte_code_wire::RequestStatus::Ok,
                RequestStatus::Error => norte_code_wire::RequestStatus::Error,
                RequestStatus::Cancelled => norte_code_wire::RequestStatus::Cancelled,
            },
            error: row.error.clone(),
            error_kind: row.error_kind.clone(),
            retry_after_ms: row.retry_after_ms,
        }
    }
}

/// Reads a wire record back into the row a report summarizes.
///
/// # Errors
///
/// Returns the parse error when `started_at` is not RFC 3339: a report built
/// on a guessed timestamp is worse than one that says the row is unreadable.
impl TryFrom<norte_code_wire::TelemetryRecord> for RequestRecord {
    type Error = chrono::ParseError;

    fn try_from(
        record: norte_code_wire::TelemetryRecord,
    ) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            id: record.id,
            session: record.session,
            north_session: record.north_session,
            turn: record.turn,
            provider: record.provider,
            model: record.model,
            endpoint: record.endpoint,
            started_at: chrono::DateTime::parse_from_rfc3339(&record.started_at)?
                .with_timezone(&chrono::Utc),
            duration_ms: record.duration_ms,
            first_token_ms: record.first_token_ms,
            last_token_ms: record.last_token_ms,
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            reasoning_tokens: record.reasoning_tokens,
            cache_read_tokens: record.cache_read_tokens,
            cache_write_tokens: record.cache_write_tokens,
            estimated_input_tokens: record.estimated_input_tokens,
            cache_hit: record.cache_hit,
            tool_calls: record.tool_calls,
            reasoning: record.reasoning,
            sandbox: record.sandbox,
            sandbox_network: record.sandbox_network,
            params: record.params,
            extensions: record.extensions,
            metadata: record.metadata,
            cost_usd: record.cost_usd,
            finish_reason: record.finish_reason,
            status: match record.status {
                norte_code_wire::RequestStatus::Ok => RequestStatus::Ok,
                norte_code_wire::RequestStatus::Error => RequestStatus::Error,
                norte_code_wire::RequestStatus::Cancelled => RequestStatus::Cancelled,
            },
            error: record.error,
            error_kind: record.error_kind,
            retry_after_ms: record.retry_after_ms,
        })
    }
}

#[cfg(test)]
#[path = "sqlite_tests.rs"]
mod tests;
