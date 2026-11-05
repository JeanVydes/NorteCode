//! Runtime-independent RAEP host adapter.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use norte_code_agent::Service;
use norte_code_protocol::{
    ClientRequest, ClientRequestKind, ServerEvent, ServerEventKind, SessionKey,
};
use raep_types::{
    AttachMode, Envelope, FrameId, FrameType, SessionAttach, StandardFrameType, UserInput,
    WIRE_VERSION,
};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore, broadcast};

use crate::NorteCodeExtension;
use crate::commands::AgentCommand;
use crate::payloads::{canonical_event_payload, standard_event_payload};

/// Host configuration independent of the HTTP listener.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Agent identifier accepted during attachment.
    pub agent_id: String,
    /// Advertised server version.
    pub server_version: String,
    /// Maximum number of emitted frames retained per session.
    pub replay_capacity: usize,
    /// Maximum frame size advertised to clients.
    pub max_frame_bytes: u64,
    /// Optional bearer token required by operational HTTP routes.
    pub auth_token: Option<String>,
    /// Optional bearer-token to principal mappings for multi-user ownership.
    pub auth_principals: HashMap<String, String>,
    /// Maximum size accepted for one uploaded artifact.
    pub max_artifact_bytes: usize,
    /// Maximum concurrent WebSocket and SSE connections.
    pub max_connections: usize,
    /// Maximum time spent handling one command, in milliseconds.
    pub command_timeout_ms: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            agent_id: "norte-code".to_owned(),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            replay_capacity: 10_000,
            max_frame_bytes: 1024 * 1024,
            auth_token: None,
            auth_principals: HashMap::new(),
            max_artifact_bytes: 16 * 1024 * 1024,
            max_connections: 256,
            command_timeout_ms: 120_000,
        }
    }
}

/// Errors raised while translating or executing a RAEP frame.
#[derive(Debug, Error)]
pub enum HostError {
    /// The frame is invalid or cannot be used in the current connection state.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    /// The Norte Code service rejected a request before streaming it.
    #[error("service request failed: {0}")]
    Service(String),
    /// Replay is unavailable for the requested sequence.
    #[error("replay unavailable: {0}")]
    Replay(String),
    /// A frame could not be serialized.
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The durable replay journal could not be read or written.
    #[error("replay journal failed: {0}")]
    ReplayJournal(String),
    /// An upload could not be stored or resolved.
    #[error("artifact failed: {0}")]
    Artifact(String),
}

/// Durable storage for RAEP frames.
#[async_trait]
pub trait ReplayStore: Send + Sync {
    /// Appends one emitted frame.
    async fn append(&self, frame: &Envelope<Value>) -> Result<(), HostError>;

    /// Loads frames for one session, including frames retained before restart.
    async fn load(&self, session_id: &str) -> Result<Vec<Envelope<Value>>, HostError>;
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
                    ON raep_replay(session_id, seq);",
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
}

#[derive(Default)]
struct HostState {
    attached: HashSet<String>,
    sequences: HashMap<String, u64>,
    replay: HashMap<String, VecDeque<Envelope<Value>>>,
    idempotency: HashMap<FrameId, Vec<Envelope<Value>>>,
    loaded_replay: HashSet<String>,
    session_owners: HashMap<String, String>,
    artifact_owners: HashMap<String, String>,
    session_agents: HashMap<String, String>,
}

/// A Norte Code service exposed through the RAEP wire.
pub struct NorteCodeRaepHost {
    service: Arc<Service>,
    services: HashMap<String, Arc<Service>>,
    config: HostConfig,
    state: Mutex<HostState>,
    events: broadcast::Sender<Envelope<Value>>,
    replay_store: Option<Arc<dyn ReplayStore>>,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    connections: Arc<Semaphore>,
}

impl NorteCodeRaepHost {
    /// Creates a host around the server-owned Norte Code service.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured agent identifier is empty or the
    /// replay capacity is zero.
    pub fn new(service: Arc<Service>, config: HostConfig) -> Result<Self, HostError> {
        if config.agent_id.is_empty() {
            return Err(HostError::InvalidFrame(
                "agent_id must not be empty".to_owned(),
            ));
        }
        if config.replay_capacity == 0 {
            return Err(HostError::InvalidFrame(
                "replay_capacity must be positive".to_owned(),
            ));
        }
        if config.max_connections == 0 {
            return Err(HostError::InvalidFrame(
                "max_connections must be positive".to_owned(),
            ));
        }
        let max_connections = config.max_connections;
        let (events, _) = broadcast::channel(config.replay_capacity.max(16));
        Ok(Self {
            services: HashMap::from([(config.agent_id.clone(), Arc::clone(&service))]),
            service,
            config,
            state: Mutex::new(HostState::default()),
            events,
            replay_store: None,
            artifact_store: None,
            connections: Arc::new(Semaphore::new(max_connections)),
        })
    }

    /// Creates a host exposing several named agent services.
    ///
    /// The entry named by `config.agent_id` is the default used by
    /// connection-scoped requests and by clients that do not name an agent.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured default agent is not present.
    pub fn with_agents(
        services: HashMap<String, Arc<Service>>,
        config: HostConfig,
    ) -> Result<Self, HostError> {
        let service = services.get(&config.agent_id).cloned().ok_or_else(|| {
            HostError::InvalidFrame(format!(
                "no service configured for agent `{}`",
                config.agent_id
            ))
        })?;
        let mut host = Self::new(service, config)?;
        host.services = services;
        Ok(host)
    }

    /// Creates a host with a durable RAEP replay journal.
    ///
    /// # Errors
    ///
    /// Returns an error when the host configuration or journal path is invalid.
    pub fn with_replay_store(
        service: Arc<Service>,
        config: HostConfig,
        replay_store: Arc<dyn ReplayStore>,
    ) -> Result<Self, HostError> {
        let mut host = Self::new(service, config)?;
        host.replay_store = Some(replay_store);
        Ok(host)
    }

    /// Enables a durable replay journal on an already configured host.
    #[must_use]
    pub fn with_replay_store_instance(mut self, replay_store: Arc<dyn ReplayStore>) -> Self {
        self.replay_store = Some(replay_store);
        self
    }

    /// Enables durable storage for client-uploaded artifacts.
    #[must_use]
    pub fn with_artifact_store(mut self, artifact_store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(artifact_store);
        self
    }

    /// Stores an uploaded artifact.
    ///
    /// # Errors
    ///
    /// Returns an error when artifact storage is disabled or the upload fails.
    pub async fn put_artifact(
        &self,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<ArtifactRef, HostError> {
        self.put_artifact_as(mime_type, bytes, None).await
    }

    /// Stores an artifact and records ownership for an authenticated principal.
    ///
    /// # Errors
    ///
    /// Returns an error when storage is disabled, the artifact exceeds the
    /// configured limit, or the backing store rejects the write.
    pub async fn put_artifact_as(
        &self,
        mime_type: &str,
        bytes: &[u8],
        principal: Option<&str>,
    ) -> Result<ArtifactRef, HostError> {
        if bytes.len() > self.config.max_artifact_bytes {
            return Err(HostError::Artifact(format!(
                "artifact exceeds {} byte limit",
                self.config.max_artifact_bytes
            )));
        }
        let reference = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("artifact storage is disabled".to_owned()))?
            .put(mime_type, bytes)
            .await?;
        if let Some(principal) = principal {
            self.state
                .lock()
                .await
                .artifact_owners
                .insert(reference.id.clone(), principal.to_owned());
        }
        Ok(reference)
    }

    /// Reads an uploaded artifact.
    ///
    /// # Errors
    ///
    /// Returns an error when artifact storage is disabled or the identifier is
    /// invalid or missing.
    pub async fn get_artifact(&self, id: &str) -> Result<Vec<u8>, HostError> {
        self.get_artifact_as(id, None).await
    }

    /// Reads an artifact after checking its authenticated owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the artifact is not owned by the principal,
    /// storage is disabled, or the backing store cannot resolve the id.
    pub async fn get_artifact_as(
        &self,
        id: &str,
        principal: Option<&str>,
    ) -> Result<Vec<u8>, HostError> {
        if let Some(principal) = principal
            && let Some(owner) = self.state.lock().await.artifact_owners.get(id)
            && owner != principal
        {
            return Err(HostError::Artifact(
                "artifact belongs to another principal".to_owned(),
            ));
        }
        self.artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("artifact storage is disabled".to_owned()))?
            .get(id)
            .await
    }

    /// Returns the server-owned agent service.
    #[must_use]
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }

    /// Returns the configured agent identifier.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.config.agent_id
    }

    fn service_for_agent(&self, agent_id: &str) -> Result<Arc<Service>, HostError> {
        let requested = if agent_id.is_empty() {
            &self.config.agent_id
        } else {
            agent_id
        };
        self.services
            .get(requested)
            .cloned()
            .ok_or_else(|| HostError::InvalidFrame(format!("unknown agent `{requested}`")))
    }

    async fn service_for_session(&self, session_id: &str) -> Result<Arc<Service>, HostError> {
        let state = self.state.lock().await;
        let agent_id = state
            .session_agents
            .get(session_id)
            .map_or(self.config.agent_id.as_str(), String::as_str);
        self.service_for_agent(agent_id)
    }

    /// Returns the configured bearer token, when transport authentication is enabled.
    #[must_use]
    pub fn auth_token(&self) -> Option<&str> {
        self.config.auth_token.as_deref()
    }

    /// Resolves a bearer token to its configured principal.
    ///
    /// When authentication is disabled, all requests use the anonymous
    /// principal. A single legacy token maps to `default` unless an explicit
    /// principal mapping exists.
    #[must_use]
    pub fn authenticate_bearer(&self, token: &str) -> Option<String> {
        let configured = self.config.auth_token.as_deref();
        if configured.is_none() && self.config.auth_principals.is_empty() {
            return None;
        }
        if configured != Some(token) && !self.config.auth_principals.contains_key(token) {
            return None;
        }
        Some(
            self.config
                .auth_principals
                .get(token)
                .cloned()
                .unwrap_or_else(|| "default".to_owned()),
        )
    }

    /// Returns the principal used for unauthenticated local operation.
    #[must_use]
    pub fn anonymous_principal(&self) -> &'static str {
        "anonymous"
    }

    /// Returns whether this host requires transport authentication.
    #[must_use]
    pub fn authentication_enabled(&self) -> bool {
        self.config.auth_token.is_some() || !self.config.auth_principals.is_empty()
    }

    /// Attempts to reserve one long-lived transport connection.
    pub fn try_connection(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(&self.connections).try_acquire_owned().ok()
    }

    /// Returns the configured command timeout.
    #[must_use]
    pub fn command_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.config.command_timeout_ms)
    }

    /// Returns the maximum accepted RAEP frame size.
    #[must_use]
    pub fn max_frame_bytes(&self) -> usize {
        usize::try_from(self.config.max_frame_bytes).unwrap_or(usize::MAX)
    }

    /// Returns a receiver for live frames emitted by any attached connection.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope<Value>> {
        self.events.subscribe()
    }

    async fn current_seq(&self, session_id: &str) -> u64 {
        self.state
            .lock()
            .await
            .sequences
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }

    /// Returns the RAEP handshake frame.
    ///
    /// The handshake payload uses only JSON values constructed from validated
    /// host configuration, so serialization cannot fail.
    #[must_use]
    pub fn hello(&self) -> Envelope<Value> {
        Envelope::new(
            standard(StandardFrameType::Hello),
            json!({
                "protocol": {"name": "RAEP", "version": WIRE_VERSION},
                "server_version": self.config.server_version,
                "wire_versions": [WIRE_VERSION],
                "capabilities": [
                    "transport.ws",
                    "transport.sse",
                    "agent.norte_code",
                    "agent.commands",
                    "events.replay",
                    "events.canonical",
                    "auth.principals",
                    "replay.sqlite"
                ],
                "extensions": [NorteCodeExtension::Command.as_str()],
                "limits": {
                    "max_frame_bytes": self.config.max_frame_bytes,
                    "replay_buffer_events": self.config.replay_capacity,
                    "max_artifact_bytes": self.config.max_artifact_bytes,
                },
                "heartbeat_s": 20,
                "draining": false,
                "deprecations": []
            }),
            now_millis(),
        )
    }

    /// Returns discovery information for an HTTP info route.
    #[must_use]
    pub fn info(&self) -> Value {
        let default_model = self
            .services
            .get(&self.config.agent_id)
            .and_then(|service| service.default_spec())
            .map(|spec| json!({"provider": spec.provider, "model": spec.model}));
        json!({
            "protocol": {"name": "RAEP", "version": WIRE_VERSION},
            "server_version": self.config.server_version,
            "wire_versions": [WIRE_VERSION],
            "capabilities": [
                "transport.ws",
                "transport.sse",
                "agent.norte_code",
                "agent.commands",
                "events.replay",
                "events.canonical",
                "auth.principals",
                "replay.sqlite"
            ],
            "extensions": [NorteCodeExtension::Command.as_str()],
            "limits": {
                "max_frame_bytes": self.config.max_frame_bytes,
                "replay_buffer_events": self.config.replay_capacity,
                "max_artifact_bytes": self.config.max_artifact_bytes
            },
            "agents": self.services.keys().collect::<Vec<_>>(),
            "default_agent": self.config.agent_id,
            "default_model": default_model
        })
    }

    /// Returns readiness and basic operational counters for health probes.
    #[must_use]
    pub fn readiness(&self) -> Value {
        json!({
            "status": "ready",
            "agent_id": self.config.agent_id,
            "agents": self.services.len(),
            "replay": self.replay_store.is_some(),
            "artifacts": self.artifact_store.is_some(),
            "limits": {
                "max_connections": self.config.max_connections,
                "command_timeout_ms": self.config.command_timeout_ms
            }
        })
    }

    /// Returns a structured telemetry report for operational dashboards.
    ///
    /// The command remains the source of truth for the records. This method
    /// adds stable aggregate counters without changing the streaming wire
    /// event consumed by existing clients.
    ///
    /// # Errors
    ///
    /// Returns an error when the telemetry command cannot be routed or its
    /// response cannot be serialized.
    pub async fn telemetry_report(
        &self,
        session: Option<&str>,
        principal: &str,
    ) -> Result<Value, HostError> {
        let session_key = session.map(SessionKey::new);
        let request = ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::ListTelemetry {
                session: session_key,
            },
        };
        let frame = Envelope::new(
            FrameType::Extension(NorteCodeExtension::Command.as_str().to_owned()),
            serde_json::to_value(request)?,
            now_millis(),
        );
        let frames = self.handle_as(frame, Some(principal)).await?;
        let records = frames
            .iter()
            .find_map(|frame| match &frame.payload {
                Value::Object(payload)
                    if payload.get("event").and_then(Value::as_str) == Some("service_data")
                        && payload.get("kind").and_then(Value::as_str) == Some("telemetry") =>
                {
                    payload.get("data").cloned()
                }
                _ => None,
            })
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let rows = records.as_array().cloned().unwrap_or_default();
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut cache_read_tokens = 0_u64;
        let mut cache_write_tokens = 0_u64;
        let mut errors = 0_u64;
        let mut cancelled = 0_u64;
        let mut duration_ms = 0_u64;
        for row in &rows {
            input_tokens += row.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
            output_tokens += row
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            cache_read_tokens += row
                .get("cache_read_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            cache_write_tokens += row
                .get("cache_write_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            duration_ms += row.get("duration_ms").and_then(Value::as_u64).unwrap_or(0);
            match row.get("status").and_then(Value::as_str) {
                Some("error") => errors += 1,
                Some("cancelled") => cancelled += 1,
                _ => {}
            }
        }
        Ok(json!({
            "session": session,
            "records": records,
            "summary": {
                "requests": rows.len(),
                "errors": errors,
                "cancelled": cancelled,
                "duration_ms": duration_ms,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cache_read_tokens": cache_read_tokens,
                "cache_write_tokens": cache_write_tokens
            }
        }))
    }

    /// Handles one decoded RAEP frame.
    ///
    /// Service events are published as soon as they arrive, while the
    /// returned vector provides the same frames to a request/response
    /// transport. This keeps transports independent from the agent loop.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame is malformed, unsupported, unauthenticated
    /// for the current attachment, or rejected by the Norte Code service.
    ///
    pub async fn handle(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.handle_as(frame, None).await
    }

    /// Handles one frame while enforcing ownership for `principal`.
    ///
    /// `None` is reserved for in-process callers and keeps the existing local
    /// behavior. HTTP and WebSocket adapters pass the authenticated principal.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame is invalid or the principal does not
    /// own the requested session.
    pub async fn handle_as(
        &self,
        frame: Envelope<Value>,
        principal: Option<&str>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        if frame.version != WIRE_VERSION {
            return Err(HostError::InvalidFrame(
                "unsupported RAEP wire version".to_owned(),
            ));
        }
        if let Some(principal) = principal
            && !frame.session_id.is_empty()
        {
            self.claim_session(&frame.session_id, principal).await?;
        }
        {
            let state = self.state.lock().await;
            if let Some(previous) = state.idempotency.get(&frame.id) {
                return Ok(previous.clone());
            }
        }
        let frame_id = frame.id.clone();
        let result = match frame.kind {
            FrameType::Standard(StandardFrameType::SessionAttach) => self.attach(frame).await,
            FrameType::Standard(StandardFrameType::UserInput) => self.user_input(frame).await,
            FrameType::Standard(StandardFrameType::CancelTurn) => self.cancel(frame).await,
            FrameType::Standard(StandardFrameType::SessionDetach) => self.detach(frame).await,
            FrameType::Standard(StandardFrameType::SessionClose) => self.close(frame).await,
            FrameType::Standard(StandardFrameType::Ack) => self.ack(frame).await,
            FrameType::Standard(StandardFrameType::SessionConfigure) => {
                self.agent_command(frame).await
            }
            FrameType::Extension(ref name) if name == NorteCodeExtension::Command.as_str() => {
                self.extension_command(frame).await
            }
            _ => Err(HostError::InvalidFrame("unsupported command".to_owned())),
        }?;
        self.state
            .lock()
            .await
            .idempotency
            .insert(frame_id, result.clone());
        Ok(result)
    }

    async fn claim_session(&self, session_id: &str, principal: &str) -> Result<(), HostError> {
        let mut state = self.state.lock().await;
        match state.session_owners.get(session_id) {
            Some(owner) if owner != principal => Err(HostError::InvalidFrame(
                "session belongs to another principal".to_owned(),
            )),
            Some(_) => Ok(()),
            None => {
                state
                    .session_owners
                    .insert(session_id.to_owned(), principal.to_owned());
                Ok(())
            }
        }
    }

    async fn agent_command(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let command: AgentCommand = serde_json::from_value(frame.payload)?;
        let session_id = frame.session_id;
        let session = SessionKey::new(session_id.clone());
        let kind = match command {
            AgentCommand::ListJobs => ClientRequestKind::ListJobs { session },
            AgentCommand::StopJobs { id } => ClientRequestKind::StopJobs { session, id },
            AgentCommand::SetModel { provider, model } => ClientRequestKind::SetModel {
                session,
                provider,
                model,
            },
            AgentCommand::SetReasoning { scope, reasoning } => ClientRequestKind::SetReasoning {
                session,
                scope,
                reasoning,
            },
            AgentCommand::SetBudget { budget } => ClientRequestKind::SetBudget { session, budget },
            AgentCommand::AnswerQuestion { question, answer } => {
                ClientRequestKind::AnswerQuestion {
                    session,
                    question,
                    answer,
                }
            }
            AgentCommand::Resume => ClientRequestKind::Resume {
                session,
                replay: true,
            },
            AgentCommand::Compact => ClientRequestKind::Compact { session },
        };
        self.submit_service(&session_id, kind).await
    }

    /// Returns replay frames strictly after `last_seq`.
    ///
    /// # Errors
    ///
    /// Returns an error when the session has no retained replay window or the
    /// requested sequence predates the retained window.
    pub async fn replay_after(
        &self,
        session_id: &str,
        last_seq: u64,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let state = self.state.lock().await;
        let frames = state
            .replay
            .get(session_id)
            .ok_or_else(|| HostError::Replay("session replay window is missing".to_owned()))?;
        if let Some(first) = frames.front().and_then(|frame| frame.seq)
            && last_seq.saturating_add(1) < first
        {
            return Err(HostError::Replay(
                "requested sequence is older than the retained window".to_owned(),
            ));
        }
        Ok(frames
            .iter()
            .filter(|frame| frame.seq.is_some_and(|seq| seq > last_seq))
            .cloned()
            .collect())
    }

    async fn attach(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        let attach: SessionAttach = serde_json::from_value(frame.payload)?;
        let agent_id = if attach.agent_id.is_empty() {
            self.config.agent_id.clone()
        } else {
            attach.agent_id.clone()
        };
        let service = self.service_for_agent(&agent_id)?;
        let (session_id, created) = if frame.session_id.is_empty() {
            let key = service
                .start_session(None)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?;
            (key.to_string(), true)
        } else {
            (frame.session_id.clone(), false)
        };
        let exists = created
            || self
                .service_for_session(&session_id)
                .await
                .ok()
                .and_then(|service| service.session_title(&SessionKey::new(session_id.clone())))
                .is_some()
            || service
                .session_title(&SessionKey::new(session_id.clone()))
                .is_some()
            || service
                .session_spec(&SessionKey::new(session_id.clone()))
                .is_some()
            || service
                .sessions()
                .iter()
                .any(|session| session.session.as_str() == session_id);
        if !exists && attach.create_if_missing == Some(false) {
            return Err(HostError::InvalidFrame("session not found".to_owned()));
        }
        self.state
            .lock()
            .await
            .session_agents
            .insert(session_id.clone(), agent_id);
        self.load_persisted_replay(&session_id).await?;
        {
            let mut state = self.state.lock().await;
            state.attached.insert(session_id.clone());
            state.sequences.entry(session_id.clone()).or_insert(0);
            state.replay.entry(session_id.clone()).or_default();
        }
        let (replayed, replay_meta) = match attach.resume_from_seq {
            Some(after) => match self.replay_after(&session_id, after).await {
                Ok(frames) => {
                    let to_seq = frames
                        .iter()
                        .filter_map(|frame| frame.seq)
                        .max()
                        .unwrap_or(after);
                    (
                        frames,
                        json!({"from_seq": after.saturating_add(1), "to_seq": to_seq, "truncated": false}),
                    )
                }
                Err(HostError::Replay(_)) => (
                    Vec::new(),
                    json!({"from_seq": after.saturating_add(1), "to_seq": after, "truncated": true}),
                ),
                Err(error) => return Err(error),
            },
            None => (Vec::new(), json!({"truncated": false})),
        };
        let last_seq = self.current_seq(&session_id).await;
        let mode = attach.mode.unwrap_or(AttachMode::Controller);
        let attached = self
            .emit(
                &session_id,
                StandardFrameType::SessionAttached,
                json!({
                    "created": created,
                    "mode": mode,
                    "last_seq": last_seq,
                    "replay": replay_meta,
                    "negotiated_capabilities": [
                        "agent.norte_code",
                        "events.replay"
                    ]
                }),
            )
            .await?;
        let mut state_payload = json!({"status": "ACTIVE", "history_summary": []});
        if let Some(title) = service.session_title(&SessionKey::new(session_id.clone())) {
            state_payload["title"] = json!(title);
        }
        let state = self
            .emit(&session_id, StandardFrameType::SessionState, state_payload)
            .await?;
        let mut output = vec![attached, state];
        output.extend(replayed);
        Ok(output)
    }

    async fn load_persisted_replay(&self, session_id: &str) -> Result<(), HostError> {
        let Some(store) = &self.replay_store else {
            return Ok(());
        };
        if self.state.lock().await.loaded_replay.contains(session_id) {
            return Ok(());
        }
        let persisted = store.load(session_id).await?;
        let mut state = self.state.lock().await;
        let mut max_seq = 0;
        let replayed = persisted
            .into_iter()
            .filter(|frame| {
                if let Some(seq) = frame.seq {
                    max_seq = max_seq.max(seq);
                    true
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        if max_seq > 0 {
            state
                .sequences
                .entry(session_id.to_owned())
                .and_modify(|current| *current = (*current).max(max_seq))
                .or_insert(max_seq);
        }
        let replay = state.replay.entry(session_id.to_owned()).or_default();
        replay.extend(replayed);
        while replay.len() > self.config.replay_capacity {
            replay.pop_front();
        }
        state.loaded_replay.insert(session_id.to_owned());
        Ok(())
    }

    async fn user_input(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let input: UserInput = serde_json::from_value(frame.payload)?;
        input
            .validate()
            .map_err(|error| HostError::InvalidFrame(error.to_string()))?;
        let (text, attachments) = match (input.prompt, input.content) {
            (Some(text), None) => (text, Vec::new()),
            (None, Some(parts)) => {
                let mut text = String::new();
                let mut attachments = Vec::new();
                for part in parts {
                    if part.kind == raep_types::Modality::Text {
                        text.push_str(part.text.as_deref().ok_or_else(|| {
                            HostError::InvalidFrame("text content part is missing text".to_owned())
                        })?);
                        continue;
                    }
                    let source =
                        part.source
                            .as_ref()
                            .and_then(Value::as_object)
                            .ok_or_else(|| {
                                HostError::InvalidFrame(
                                    "non-text content requires an artifact source".to_owned(),
                                )
                            })?;
                    let artifact_id = source
                        .get("artifact_id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            HostError::InvalidFrame(
                                "artifact source requires artifact_id".to_owned(),
                            )
                        })?;
                    let blob = artifact_id.parse().map_err(|_| {
                        HostError::InvalidFrame("artifact_id is not a valid blob id".to_owned())
                    })?;
                    let mime = part.mime_type.clone().ok_or_else(|| {
                        HostError::InvalidFrame("artifact content requires mime_type".to_owned())
                    })?;
                    let name = source
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(artifact_id)
                        .to_owned();
                    attachments.push(norte_code_protocol::Attachment {
                        path: String::new(),
                        blob: Some(blob),
                        mime,
                        name,
                    });
                }
                (text, attachments)
            }
            _ => unreachable!("UserInput::validate enforces exactly one form"),
        };
        let mut output = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::SendMessage {
                    session: Some(SessionKey::new(frame.session_id.clone())),
                    text,
                    attachments,
                    budget: None,
                    reasoning: None,
                    replay: false,
                },
            )
            .await?;
        if output.is_empty() {
            output.push(
                self.emit(
                    &frame.session_id,
                    StandardFrameType::UserInputAccepted,
                    json!({"accepted": true}),
                )
                .await?,
            );
        }
        Ok(output)
    }

    async fn cancel(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request = ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::CancelTurn {
                session: SessionKey::new(frame.session_id.clone()),
            },
        };
        let events = self
            .service
            .submit(request)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.collect_service_events(&frame.session_id, events).await
    }

    async fn detach(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        let mut state = self.state.lock().await;
        state.attached.remove(&frame.session_id);
        Ok(Vec::new())
    }

    async fn close(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let events = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::DeleteSession {
                    session: SessionKey::new(frame.session_id.clone()),
                },
            )
            .await?;
        self.state.lock().await.attached.remove(&frame.session_id);
        Ok(events)
    }

    async fn ack(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let last_seq = frame
            .payload
            .get("last_seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| HostError::InvalidFrame("ACK.last_seq is required".to_owned()))?;
        let mut state = self.state.lock().await;
        if let Some(frames) = state.replay.get_mut(&frame.session_id) {
            frames.retain(|stored| stored.seq.is_none_or(|seq| seq > last_seq));
        }
        Ok(Vec::new())
    }

    async fn extension_command(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let request: ClientRequest = serde_json::from_value(frame.payload)?;
        if is_plugin_request(&request.kind) {
            return self.plugin_command(&frame.session_id, request).await;
        }
        // Connection-scoped commands deliberately ignore the route's
        // placeholder session id. The HTTP adapter has a session-shaped URL
        // for every command, but listing sessions, creating one, and querying
        // host-wide catalogues must work before any session is attached.
        let session_id = session_for_request(&request.kind)
            .map(std::string::ToString::to_string)
            .unwrap_or_default();
        if !session_id.is_empty() {
            self.require_attached(&session_id).await?;
        }
        let service = if session_id.is_empty() {
            Arc::clone(&self.service)
        } else {
            self.service_for_session(&session_id).await?
        };
        self.collect_service_events(
            &session_id,
            service
                .submit(request)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
        )
        .await
    }

    async fn plugin_command(
        &self,
        session_id: &str,
        request: ClientRequest,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let event = match request.kind {
            ClientRequestKind::ListPlugins => ServerEventKind::PluginList {
                plugins: crate::plugin_manager::plugin_info(),
            },
            ClientRequestKind::InstallPlugin { url, package, all } => {
                let result = tokio::task::spawn_blocking(move || {
                    if all {
                        crate::plugin_manager::install_all_plugins(&url, &|_, _, _| {})
                            .map(|names| names.join(", "))
                    } else {
                        crate::plugin_manager::install_plugin(
                            &url,
                            package.as_deref(),
                            &|_, _, _| {},
                        )
                    }
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: "installed".to_owned(),
                    name: result,
                }
            }
            ClientRequestKind::UpdatePlugins { name } => {
                let result = tokio::task::spawn_blocking(move || {
                    crate::plugin_manager::update_plugins(name.as_deref())
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                let name = result
                    .into_iter()
                    .filter_map(|(_, result)| result.ok())
                    .collect::<Vec<_>>()
                    .join(", ");
                ServerEventKind::PluginChanged {
                    operation: "updated".to_owned(),
                    name,
                }
            }
            ClientRequestKind::RemovePlugin { name } => {
                tokio::task::spawn_blocking({
                    let name = name.clone();
                    move || crate::plugin_manager::remove_plugin(&name)
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: "removed".to_owned(),
                    name,
                }
            }
            ClientRequestKind::SetPluginEnabled { name, enabled } => {
                tokio::task::spawn_blocking({
                    let name = name.clone();
                    move || crate::plugin_manager::set_plugin_enabled(&name, enabled)
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: if enabled { "enabled" } else { "disabled" }.to_owned(),
                    name,
                }
            }
            _ => unreachable!("plugin request was checked before dispatch"),
        };
        self.emit_event(
            session_id,
            ServerEvent {
                request_id: request.request_id,
                session: None,
                kind: event,
            },
        )
        .await
        .map(|frame| vec![frame])
    }

    async fn submit_service(
        &self,
        session_id: &str,
        kind: ClientRequestKind,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind,
            })
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.collect_service_events(session_id, stream).await
    }

    async fn collect_service_events(
        &self,
        session_id: &str,
        mut stream: norte_code_agent::EventStream,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        use futures_util::StreamExt;
        let mut output = Vec::new();
        while let Some(event) = stream.next().await {
            output.push(self.emit_event(session_id, event).await?);
        }
        Ok(output)
    }

    async fn emit_event(
        &self,
        session_id: &str,
        event: ServerEvent,
    ) -> Result<Envelope<Value>, HostError> {
        let kind = frame_type_for_event(&event.kind);
        let session_id = event
            .session
            .as_ref()
            .map_or_else(|| session_id.to_owned(), |key| key.as_str().to_owned());
        let turn_id = turn_id_for_event(&event.kind);
        let payload = standard_event_payload(&event.kind)
            .unwrap_or_else(|| canonical_event_payload(&event.kind));
        self.emit_with_extension(
            &session_id,
            kind,
            payload,
            Some(json!({"norte_code": serde_json::to_value(&event)?})),
            turn_id,
        )
        .await
    }

    async fn emit(
        &self,
        session_id: &str,
        kind: StandardFrameType,
        payload: Value,
    ) -> Result<Envelope<Value>, HostError> {
        self.emit_with_extension(session_id, standard(kind), payload, None, None)
            .await
    }

    async fn emit_with_extension(
        &self,
        session_id: &str,
        kind: FrameType,
        payload: Value,
        ext: Option<Value>,
        turn_id: Option<u64>,
    ) -> Result<Envelope<Value>, HostError> {
        let mut frame = Envelope::new(kind, payload, now_millis());
        frame.session_id = session_id.to_owned();
        frame.ext = ext;
        if let Some(turn) = turn_id {
            frame.turn_id = Some(
                raep_types::TurnId::new(turn.to_string())
                    .map_err(|error| HostError::InvalidFrame(error.to_string()))?,
            );
        }
        let mut state = self.state.lock().await;
        let sequence = state.sequences.entry(session_id.to_owned()).or_insert(0);
        *sequence = sequence.saturating_add(1);
        frame.seq = Some(*sequence);
        let replay = state.replay.entry(session_id.to_owned()).or_default();
        replay.push_back(frame.clone());
        while replay.len() > self.config.replay_capacity {
            replay.pop_front();
        }
        if let Some(store) = &self.replay_store {
            store.append(&frame).await?;
        }
        drop(state);
        let _ = self.events.send(frame.clone());
        Ok(frame)
    }

    async fn require_attached(&self, session_id: &str) -> Result<(), HostError> {
        if self.state.lock().await.attached.contains(session_id) {
            Ok(())
        } else {
            Err(HostError::InvalidFrame(
                "session is not attached".to_owned(),
            ))
        }
    }
}

fn turn_id_for_event(kind: &ServerEventKind) -> Option<u64> {
    match kind {
        ServerEventKind::TurnStarted { turn }
        | ServerEventKind::Delta { turn, .. }
        | ServerEventKind::ToolStarted { turn, .. }
        | ServerEventKind::ToolOutput { turn, .. }
        | ServerEventKind::ToolFinished { turn, .. }
        | ServerEventKind::RequestReconstructed { turn, .. }
        | ServerEventKind::TurnFinished { turn, .. }
        | ServerEventKind::QuestionAsked { turn, .. }
        | ServerEventKind::RetryScheduled { turn, .. }
        | ServerEventKind::RetryStarted { turn, .. } => Some(*turn),
        _ => None,
    }
}

fn frame_type_for_event(kind: &ServerEventKind) -> FrameType {
    if matches!(
        kind,
        ServerEventKind::QuestionAsked { .. }
            | ServerEventKind::PluginList { .. }
            | ServerEventKind::PluginChanged { .. }
    ) {
        if matches!(kind, ServerEventKind::QuestionAsked { .. }) {
            FrameType::Extension("norte_code.question_asked".to_owned())
        } else {
            FrameType::Extension(NorteCodeExtension::Plugin.as_str().to_owned())
        }
    } else if matches!(kind, ServerEventKind::Telemetry { .. }) {
        FrameType::Extension(NorteCodeExtension::Telemetry.as_str().to_owned())
    } else if matches!(kind, ServerEventKind::ToolList { .. }) {
        FrameType::Extension(NorteCodeExtension::ToolList.as_str().to_owned())
    } else {
        standard(standard_frame_type_for_event(kind))
    }
}

fn standard_frame_type_for_event(kind: &ServerEventKind) -> StandardFrameType {
    match kind {
        ServerEventKind::Delta { .. } => StandardFrameType::TokenDelta,
        ServerEventKind::ToolStarted { .. } => StandardFrameType::ToolCallStarted,
        ServerEventKind::ToolOutput { .. } => StandardFrameType::JobProgressDelta,
        ServerEventKind::ToolFinished { .. } => StandardFrameType::ToolCallFinished,
        ServerEventKind::QuestionAsked { .. } => StandardFrameType::ToolApprovalResponse,
        ServerEventKind::BudgetWarning { .. } => StandardFrameType::BudgetWarning,
        ServerEventKind::CompactionStarted { .. } => StandardFrameType::ContextUpdated,
        ServerEventKind::CompactionFinished { .. } => StandardFrameType::HistoryCompacted,
        ServerEventKind::SessionCreated { .. }
        | ServerEventKind::SessionList { .. }
        | ServerEventKind::SessionDeleted
        | ServerEventKind::SessionRenamed { .. }
        | ServerEventKind::SessionForked { .. }
        | ServerEventKind::SessionSwitched
        | ServerEventKind::SessionStats { .. }
        | ServerEventKind::Committed { .. }
        | ServerEventKind::SessionTodos { .. } => StandardFrameType::SessionUpdated,
        ServerEventKind::TurnStarted { .. }
        | ServerEventKind::TurnFinished { .. }
        | ServerEventKind::TurnStopped { .. } => StandardFrameType::TurnStateChanged,
        ServerEventKind::Error { .. } => StandardFrameType::Error,
        ServerEventKind::JobList { .. } | ServerEventKind::JobsStopped { .. } => {
            StandardFrameType::JobProgressDelta
        }
        ServerEventKind::SteerApplied { .. } => StandardFrameType::UserInputAccepted,
        ServerEventKind::ModelChanged { .. }
        | ServerEventKind::ModelQueued { .. }
        | ServerEventKind::ReasoningDowngraded { .. }
        | ServerEventKind::ModelList { .. }
        | ServerEventKind::CatalogUpdated { .. }
        | ServerEventKind::SkillList { .. }
        | ServerEventKind::SkillsUpdated { .. }
        | ServerEventKind::PluginList { .. }
        | ServerEventKind::PluginChanged { .. }
        | ServerEventKind::Telemetry { .. }
        | ServerEventKind::ToolList { .. }
        | ServerEventKind::McpList { .. } => StandardFrameType::AgentChanged,
        ServerEventKind::QuestionAnswered { .. } => StandardFrameType::ApprovalResolved,
        ServerEventKind::RetryScheduled { .. } | ServerEventKind::RetryStarted { .. } => {
            StandardFrameType::ProviderRetry
        }
        ServerEventKind::RequestReconstructed { .. }
        | ServerEventKind::CostUpdated { .. }
        | ServerEventKind::CostNotReported { .. } => StandardFrameType::OutputPartFinished,
        ServerEventKind::MessageReplayed { .. } => StandardFrameType::HistoryGet,
        ServerEventKind::BudgetExceeded { .. } | ServerEventKind::BudgetChanged { .. } => {
            StandardFrameType::BudgetWarning
        }
    }
}

fn session_for_request(kind: &ClientRequestKind) -> Option<&SessionKey> {
    match kind {
        ClientRequestKind::SwitchSession { session }
        | ClientRequestKind::RenameSession { session, .. }
        | ClientRequestKind::DeleteSession { session }
        | ClientRequestKind::Steer { session, .. }
        | ClientRequestKind::CancelTurn { session }
        | ClientRequestKind::StopJobs { session, .. }
        | ClientRequestKind::DetachTurn { session }
        | ClientRequestKind::ListJobs { session }
        | ClientRequestKind::Resume { session, .. }
        | ClientRequestKind::AnswerQuestion { session, .. }
        | ClientRequestKind::SetModel { session, .. }
        | ClientRequestKind::SetReasoning { session, .. }
        | ClientRequestKind::SetBudget { session, .. }
        | ClientRequestKind::Compact { session }
        | ClientRequestKind::ForkSession { session, .. }
        | ClientRequestKind::ListTools {
            session: Some(session),
        }
        | ClientRequestKind::SetApprovalRule {
            session: Some(session),
            ..
        }
        | ClientRequestKind::ForgetApprovalRule {
            session: Some(session),
            ..
        } => Some(session),
        ClientRequestKind::SendMessage { session, .. }
        | ClientRequestKind::CostReport { session }
        | ClientRequestKind::ListTelemetry { session } => session.as_ref(),
        ClientRequestKind::CreateSession { .. }
        | ClientRequestKind::ListSessions
        | ClientRequestKind::ListModels
        | ClientRequestKind::ListTools { session: None }
        | ClientRequestKind::SetApprovalRule { session: None, .. }
        | ClientRequestKind::ForgetApprovalRule { session: None, .. }
        | ClientRequestKind::RefreshCatalog
        | ClientRequestKind::ListSkills
        | ClientRequestKind::RefreshSkills
        | ClientRequestKind::ListPlugins
        | ClientRequestKind::InstallPlugin { .. }
        | ClientRequestKind::UpdatePlugins { .. }
        | ClientRequestKind::RemovePlugin { .. }
        | ClientRequestKind::SetPluginEnabled { .. }
        | ClientRequestKind::ListMcp
        | ClientRequestKind::RefreshMcp
        | ClientRequestKind::SetMcpActive { .. } => None,
    }
}

fn is_plugin_request(kind: &ClientRequestKind) -> bool {
    matches!(
        kind,
        ClientRequestKind::ListPlugins
            | ClientRequestKind::InstallPlugin { .. }
            | ClientRequestKind::UpdatePlugins { .. }
            | ClientRequestKind::RemovePlugin { .. }
            | ClientRequestKind::SetPluginEnabled { .. }
    )
}

fn standard(kind: StandardFrameType) -> FrameType {
    FrameType::Standard(kind)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte::BlobStore;

    #[tokio::test]
    async fn file_replay_store_round_trips_frames_by_session() {
        let path = std::env::temp_dir().join(format!(
            "norte-code-raep-replay-{}.jsonl",
            FrameId::generate()
        ));
        let store = FileReplayStore::open(&path).expect("replay store");
        let mut frame = Envelope::new(
            standard(StandardFrameType::TokenDelta),
            json!({"delta": "hello"}),
            1,
        );
        frame.session_id = "session-a".to_owned();
        frame.seq = Some(7);
        store.append(&frame).await.expect("append");

        let frames = store.load("session-a").await.expect("load");
        assert_eq!(frames, vec![frame]);
        assert_eq!(store.load("session-b").await.expect("load"), Vec::new());
        std::fs::remove_file(path).expect("remove replay journal");
    }

    #[tokio::test]
    async fn file_replay_store_rotates_each_session_window() {
        let path = std::env::temp_dir().join(format!(
            "norte-code-raep-rotate-{}.jsonl",
            FrameId::generate()
        ));
        let store = FileReplayStore::open_with_capacity(&path, 2).expect("replay store");
        for seq in 1..=3 {
            let mut frame = Envelope::new(
                standard(StandardFrameType::TokenDelta),
                json!({"seq": seq}),
                i64::try_from(seq).expect("test sequence fits in i64"),
            );
            frame.session_id = "session-a".to_owned();
            frame.seq = Some(seq);
            store.append(&frame).await.expect("append");
        }
        let frames = store.load("session-a").await.expect("load");
        assert_eq!(
            frames
                .iter()
                .filter_map(|frame| frame.seq)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        std::fs::remove_file(path).expect("remove replay journal");
    }

    #[tokio::test]
    async fn sqlite_replay_store_round_trips_and_rotates() {
        let path =
            std::env::temp_dir().join(format!("norte-code-raep-replay-{}.db", FrameId::generate()));
        let store = SqliteReplayStore::open(&path, 2).expect("replay store");
        for seq in 1..=3 {
            let mut frame = Envelope::new(
                standard(StandardFrameType::TokenDelta),
                json!({"seq": seq}),
                i64::try_from(seq).expect("test sequence fits in i64"),
            );
            frame.session_id = "session-a".to_owned();
            frame.seq = Some(seq);
            store.append(&frame).await.expect("append");
        }
        let frames = store.load("session-a").await.expect("load");
        assert_eq!(
            frames
                .iter()
                .filter_map(|frame| frame.seq)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        std::fs::remove_file(path).expect("remove replay database");
    }

    #[tokio::test]
    async fn blob_artifacts_return_content_addressed_ids() {
        let blobs = Arc::new(norte::InMemoryBlobStore::new());
        let store = BlobArtifactStore::new(Arc::clone(&blobs) as Arc<dyn BlobStore>);
        let reference = store.put("image/png", b"png").await.expect("put");
        let id = reference.id.parse().expect("blob id");
        assert_eq!(blobs.get(id).await.expect("get"), b"png");
        assert_eq!(reference.mime_type, "image/png");
        assert_eq!(store.get(&reference.id).await.expect("get"), b"png");
    }

    #[tokio::test]
    async fn artifact_uploads_obey_host_limit() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let config = HostConfig {
            max_artifact_bytes: 2,
            ..HostConfig::default()
        };
        let host = NorteCodeRaepHost::new(service, config).expect("host");
        let error = host.put_artifact("text/plain", b"123").await;
        assert!(matches!(error, Err(HostError::Artifact(message)) if message.contains("limit")));
    }

    #[tokio::test]
    async fn command_frames_are_idempotent_and_reject_unknown_wire_versions() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
        let mut frame = Envelope::new(
            FrameType::Extension(NorteCodeExtension::Command.as_str().to_owned()),
            serde_json::to_value(ClientRequest {
                request_id: 1,
                kind: ClientRequestKind::ListSessions,
            })
            .expect("request"),
            1,
        );
        let first = host.handle(frame.clone()).await.expect("first request");
        let second = host.handle(frame.clone()).await.expect("duplicate request");
        assert_eq!(first, second);
        frame.version = "unsupported".to_owned();
        assert!(matches!(
            host.handle(frame).await,
            Err(HostError::InvalidFrame(message)) if message.contains("wire version")
        ));
    }

    #[tokio::test]
    async fn connection_limit_is_enforced_without_waiting() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(
            service,
            HostConfig {
                max_connections: 1,
                ..HostConfig::default()
            },
        )
        .expect("host");
        let permit = host.try_connection().expect("first connection");
        assert!(host.try_connection().is_none());
        drop(permit);
        assert!(host.try_connection().is_some());
    }

    #[tokio::test]
    async fn attaching_emits_schema_shaped_state_without_replaying_history() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
        let frame = Envelope::new(
            standard(StandardFrameType::SessionAttach),
            json!({"agent_id": "norte-code", "create_if_missing": true}),
            1,
        );
        let frames = host.handle(frame).await.expect("attach");
        assert_eq!(frames.len(), 2, "attach must not replay history");
        assert_eq!(frames[0].kind, standard(StandardFrameType::SessionAttached));
        assert_eq!(frames[0].payload["created"], true);
        assert_eq!(frames[0].payload["mode"], "controller");
        assert_eq!(frames[0].payload["last_seq"], 0);
        assert_eq!(frames[1].kind, standard(StandardFrameType::SessionState));
        assert_eq!(frames[1].payload["status"], "ACTIVE");
        assert!(
            frames[1].payload["history_summary"].is_array(),
            "SESSION_STATE must carry history_summary"
        );
    }

    #[test]
    fn questions_use_a_namespaced_server_frame() {
        let frame = frame_type_for_event(&ServerEventKind::QuestionAsked {
            turn: 1,
            question: "approval".to_owned(),
            prompt: "Continue?".to_owned(),
            options: Vec::new(),
            multiple: false,
        });
        assert_eq!(
            frame,
            FrameType::Extension("norte_code.question_asked".to_owned())
        );
    }
}
