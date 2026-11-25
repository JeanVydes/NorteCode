//! Runtime-independent RAEP host adapter.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use norte_code_agent::Service;
use norte_code_protocol::config::RaepLimitsConfig;
use norte_code_wire::{ClientRequestKind, JobStatus, SessionKey};
use raep_types::{AttachMode, Envelope, StandardFrameType};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore, broadcast};

use crate::payloads::payload;
use crate::plugin_host::PluginHostAdapter;
use norte_code_wire::ServiceDataKind;

const DEFAULT_REPLAY_CAPACITY: usize = 2_000;
const DEFAULT_MAX_CONNECTIONS: usize = 64;

fn env_limit(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn usize_limit(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

mod agents;
mod artifacts;
mod bounded_map;
mod bundle_store;
mod bundle_validation;
pub(crate) mod capabilities;
mod config_accessors;
mod configuration;
mod emit;
mod event_mapping;
mod exports;
mod frames;
mod history;
mod input;
mod input_validation;
mod modalities;
#[cfg(test)]
pub(crate) use history::{HistoryEntry, order_history};
mod jobs;
mod overrides;
pub(crate) mod payloads;
mod queue;
pub(crate) mod replay;
mod replay_access;
mod requests;
mod service_events;
mod sessions;
pub(crate) mod stores;
mod turn_book;
mod turn_control;
pub(crate) mod turn_handles;
mod turns;
pub(crate) mod uploads;
mod user_input;

pub(crate) use bundle_validation::*;
pub(crate) use event_mapping::*;
pub(crate) use input::*;
pub(crate) use input_validation::*;
pub(crate) use jobs::job_finished_payload;
#[cfg(test)]
pub(crate) use jobs::job_output_from_index;
pub(crate) use modalities::*;
pub(crate) use overrides::*;
pub(crate) use requests::*;

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use capabilities::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use payloads::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use replay::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use stores::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use uploads::*;

/// A canonical `service_data` payload, decoded to find the telemetry rows.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalServiceData {
    kind: ServiceDataKind,
    data: Value,
}

/// A replay descriptor in an attach payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ReplayMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    from_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_seq: Option<u64>,
    truncated: bool,
}

/// The session-attached payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SessionAttachedPayload {
    created: bool,
    mode: AttachMode,
    last_seq: u64,
    replay: ReplayMeta,
    negotiated_capabilities: Vec<Capability>,
    /// The non-secret summary of the effective agent (RFC 7). This host
    /// only runs one configured agent, so it is that agent.
    agent: norte_code_wire::AgentSummary,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct UserInputOrigin {
    kind: UserInputOriginKind,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct QueuedTurnStatePayload {
    state: raep_types::TurnState,
}

#[derive(Debug, serde::Serialize)]
pub(crate) enum UserInputOriginKind {
    #[serde(rename = "user")]
    User,
}

/// Host configuration independent of the HTTP listener.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Agent identifier accepted during attachment.
    pub agent_id: String,
    /// Advertised server version.
    pub server_version: String,
    /// Broadcast channel capacity for live frames.
    pub replay_capacity: usize,
    /// Directory holding one replay file per session.
    pub replay_dir: std::path::PathBuf,
    /// Maximum bytes retained per session's replay file.
    pub replay_bytes: u64,
    /// Maximum number of concurrently running jobs per session.
    pub max_concurrent_jobs: usize,
    /// Duration for which terminal job output remains queryable.
    pub job_output_retention_s: u64,
    /// Maximum number of command results retained for idempotent retries.
    pub idempotency_capacity: usize,
    /// Maximum user inputs queued behind an active turn.
    pub max_queued_inputs: usize,
    /// Maximum frame size advertised to clients.
    pub max_frame_bytes: u64,
    /// Optional bearer token required by operational HTTP routes.
    pub auth_token: Option<String>,
    /// Optional bearer-token to principal mappings for multi-user ownership.
    pub auth_principals: HashMap<String, String>,
    /// Browser origins allowed to call the HTTP API.
    ///
    /// A request that carries `Origin` is rejected unless the value is listed
    /// here. A non-browser client sends no `Origin` and is unaffected.
    pub allowed_origins: Vec<String>,
    /// Maximum size accepted for one uploaded artifact.
    pub max_artifact_bytes: usize,
    /// Maximum size of one portable session bundle.
    pub max_export_bytes: usize,
    /// Lifetime of generated portable bundles.
    pub export_ttl_s: u64,
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
            replay_capacity: env_limit("NORTE_CODE_REPLAY_CAPACITY", DEFAULT_REPLAY_CAPACITY),
            // A host embedded without a database (tests, tooling) must not
            // write into the source tree. The server binary points this at
            // the database's directory.
            replay_dir: std::env::temp_dir().join("norte-code-replay"),
            replay_bytes: 8 * 1024 * 1024,
            max_concurrent_jobs: 8,
            job_output_retention_s: 86_400,
            idempotency_capacity: 4_096,
            max_queued_inputs: 8,
            max_frame_bytes: 1024 * 1024,
            auth_token: None,
            auth_principals: HashMap::new(),
            allowed_origins: vec![
                "http://localhost:8080".to_owned(),
                "http://127.0.0.1:8080".to_owned(),
            ],
            max_artifact_bytes: 16 * 1024 * 1024,
            max_export_bytes: 64 * 1024 * 1024,
            export_ttl_s: 3600,
            max_connections: env_limit("NORTE_CODE_MAX_CONNECTIONS", DEFAULT_MAX_CONNECTIONS),
            command_timeout_ms: 120_000,
        }
    }
}

impl HostConfig {
    /// Builds a host configuration from the validated service limits.
    ///
    /// The replay capacity and connection count keep their legacy environment
    /// overrides. Storage and auth fields keep their defaults for the caller
    /// to replace.
    #[must_use]
    pub fn from_limits(agent_id: String, limits: &RaepLimitsConfig) -> Self {
        Self {
            agent_id,
            replay_capacity: env_limit("NORTE_CODE_REPLAY_CAPACITY", limits.replay_capacity),
            replay_bytes: limits.replay_bytes,
            max_concurrent_jobs: limits.max_concurrent_jobs,
            job_output_retention_s: limits.job_output_retention_s,
            idempotency_capacity: limits.idempotency_capacity,
            max_queued_inputs: limits.max_queued_inputs,
            max_frame_bytes: limits.max_frame_bytes,
            max_artifact_bytes: usize_limit(limits.max_artifact_bytes),
            max_export_bytes: usize_limit(limits.max_export_bytes),
            export_ttl_s: limits.export_ttl_s,
            max_connections: env_limit("NORTE_CODE_MAX_CONNECTIONS", limits.max_connections),
            command_timeout_ms: limits.command_timeout_ms,
            ..Self::default()
        }
    }
}

/// Errors raised while translating or executing a RAEP frame.
#[derive(Debug, Error)]
pub enum HostError {
    /// The frame is invalid or cannot be used in the current connection state.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    /// The client sent a frame type this host does not implement.
    #[error("unknown frame type: {0}")]
    UnknownType(String),
    /// A frame id or turn id was reused for a different request.
    #[error("idempotency conflict: {0}")]
    IdempotencyConflict(String),
    /// A turn was requested while the session was occupied under reject policy.
    #[error("session is busy")]
    SessionBusy,
    /// A cancellation targeted a turn that is not active.
    #[error("turn is not active")]
    TurnNotActive,
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

/// A Norte Code service exposed through the RAEP wire.
pub struct NorteCodeRaepHost {
    service: Arc<Service>,
    services: HashMap<String, Arc<Service>>,
    config: HostConfig,
    state: Mutex<HostState>,
    events: broadcast::Sender<Envelope<Value>>,
    replay: Arc<ReplayLog>,
    emission_locks: std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    upload_index: Option<Arc<dyn UploadIndex>>,
    connections: Arc<Semaphore>,
    turn_handles: Arc<turn_handles::TurnHandles>,
    plugin_host: Option<Arc<PluginHostAdapter>>,
}

pub(crate) fn summarize_plugin_updates(
    results: Vec<(String, Result<String, String>)>,
) -> Result<String, String> {
    let updated = results
        .iter()
        .filter_map(|(_, result)| result.as_ref().ok())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let failures = results
        .into_iter()
        .filter_map(|(plugin, result)| result.err().map(|error| format!("{plugin}: {error}")))
        .collect::<Vec<_>>();
    if failures.is_empty() {
        return Ok(updated);
    }
    let successful = if updated.is_empty() {
        String::new()
    } else {
        format!(" Updated successfully: {updated}.")
    };
    Err(format!(
        "Plugin update failed: {}.{successful}",
        failures.join("; ")
    ))
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
        if config.max_queued_inputs == 0 {
            return Err(HostError::InvalidFrame(
                "max_queued_inputs must be positive".to_owned(),
            ));
        }
        if config.max_connections == 0 {
            return Err(HostError::InvalidFrame(
                "max_connections must be positive".to_owned(),
            ));
        }
        if config.max_export_bytes == 0 || config.export_ttl_s == 0 {
            return Err(HostError::InvalidFrame(
                "export size and TTL limits must be positive".to_owned(),
            ));
        }
        service.configure_job_limits(config.max_concurrent_jobs, config.job_output_retention_s);
        let max_connections = config.max_connections;
        let (events, _) = broadcast::channel(config.replay_capacity.max(16));
        let replay = ReplayLog::new(config.replay_dir.clone(), config.replay_bytes);
        Ok(Self {
            services: HashMap::from([(config.agent_id.clone(), Arc::clone(&service))]),
            service,
            config,
            state: Mutex::new(HostState::default()),
            events,
            replay: Arc::new(replay),
            emission_locks: std::sync::Mutex::new(HashMap::new()),
            artifact_store: None,
            upload_index: None,
            connections: Arc::new(Semaphore::new(max_connections)),
            turn_handles: Arc::new(turn_handles::TurnHandles::default()),
            plugin_host: None,
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
        for service in services.values() {
            service.configure_job_limits(
                host.config.max_concurrent_jobs,
                host.config.job_output_retention_s,
            );
        }
        host.services = services;
        Ok(host)
    }

    /// Gives plugins a way to ask this host about the current session.
    #[must_use]
    pub(crate) fn with_plugin_host(mut self, plugin_host: Arc<PluginHostAdapter>) -> Self {
        self.plugin_host = Some(plugin_host);
        self
    }

    /// Moves the replay log to another directory.
    #[must_use]
    pub fn with_replay_log(mut self, log: ReplayLog) -> Self {
        self.replay = Arc::new(log);
        self
    }

    /// Enables durable storage for client-uploaded artifacts.
    #[must_use]
    pub fn with_artifact_store(mut self, artifact_store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(artifact_store);
        self
    }

    /// Enables session-scoped uploads with durable metadata.
    #[must_use]
    pub fn with_upload_index(mut self, upload_index: Arc<dyn UploadIndex>) -> Self {
        self.upload_index = Some(upload_index);
        self
    }

    /// Returns a receiver for live frames emitted by any attached connection.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope<Value>> {
        self.events.subscribe()
    }

    pub(crate) fn track_turn(
        &self,
        key: turn_handles::TurnKey,
        handle: tokio::task::JoinHandle<()>,
    ) {
        self.turn_handles.insert(key, handle);
    }

    pub(crate) fn finish_tracked_turn(&self, key: &turn_handles::TurnKey) {
        self.turn_handles.finish(key);
    }

    pub(crate) fn close_stream_turns(&self, session: &str) {
        self.service
            .cancel_turn_and_watch(&norte_code_wire::SessionKey::new(session.to_owned()));
        let handles = Arc::clone(&self.turn_handles);
        let session = session.to_owned();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                for handle in handles.drain_session(&session) {
                    if !handle.is_finished() {
                        handle.abort();
                    }
                }
            });
        }
    }

    /// Spawns the drain from the service's delegation events into this
    /// host's frame broadcast (RFC 8.2.10).
    ///
    /// The service emits `ChildSessionStarted/Updated` and
    /// `DelegatorStateChanged` on its own broadcast when a child is spawned
    /// or its delegator is suspended; the host turns each one into the
    /// standard frame on the parent's stream, with the same `seq` ordering
    /// as every other frame of that session. Without this drain the events
    /// would sit in the service's channel with no reader.
    ///
    /// # Errors
    ///
    /// Returns an error when the delegation receiver has already been taken:
    /// the host takes it exactly once, at startup.
    pub fn spawn_delegation_drain(self: &Arc<Self>) -> Result<(), HostError> {
        let receiver = self.service.delegation_rx_take().ok_or_else(|| {
            HostError::InvalidFrame("delegation receiver already taken".to_owned())
        })?;
        let host = Arc::clone(self);
        tokio::spawn(async move {
            let mut receiver = receiver;
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        let session_id = event
                            .session
                            .as_ref()
                            .map_or_else(String::new, |key| key.as_str().to_owned());
                        if session_id.is_empty() {
                            continue;
                        }
                        if let Err(error) = host.emit_event(&session_id, event).await {
                            tracing::warn!(%error, session = %session_id, "delegation event could not be emitted");
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(lost)) => {
                        tracing::warn!(lost, "delegation event stream lagged; frames were dropped");
                    }
                }
            }
        });
        Ok(())
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
}

pub(super) fn job_status(job: &norte_code_wire::JobInfo) -> JobStatus {
    match job.state {
        norte_code_wire::JobState::Running => JobStatus::Running,
        norte_code_wire::JobState::Cancelled => JobStatus::Killed,
        norte_code_wire::JobState::Finished if job.timed_out => JobStatus::TimedOut,
        norte_code_wire::JobState::Finished
            if job.exit_code.is_some_and(|exit_code| exit_code != 0) =>
        {
            JobStatus::Failed
        }
        norte_code_wire::JobState::Finished => JobStatus::Completed,
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
