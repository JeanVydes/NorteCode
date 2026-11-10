//! The stores the host keeps: pins, artifacts, uploads and replay.

use async_trait::async_trait;

use raep_types::{AttachMode, Envelope, FrameId, TurnId};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[allow(clippy::wildcard_imports)]
use super::*;

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
