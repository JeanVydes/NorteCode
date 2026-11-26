//! The stores the host keeps: artifacts, uploads and replay.

use async_trait::async_trait;

use raep_types::{AttachMode, Envelope, FrameId, TurnId};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

#[allow(clippy::wildcard_imports)]
use super::*;

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

#[derive(Default)]
pub(super) struct HostState {
    pub(super) attached: HashSet<String>,
    pub(super) attachment_ids: HashMap<String, FrameId>,
    pub(super) attachment_modes: HashMap<String, AttachMode>,
    pub(super) sequences: super::bounded_map::BoundedMap<String, u64>,
    pub(super) idempotency: HashMap<(String, FrameId), CachedRequest>,
    pub(super) idempotency_order: VecDeque<(String, FrameId)>,
    pub(super) session_owners: HashMap<String, String>,
    pub(super) artifact_owners: HashSet<(String, String)>,
    pub(super) exports: HashMap<String, StoredExport>,
    pub(super) imports: HashMap<String, StoredImport>,
    pub(super) session_agents: super::bounded_map::BoundedMap<String, String>,
    pub(super) session_create_keys: HashMap<String, String>,
    pub(super) session_create_order: VecDeque<String>,
    pub(super) client_capabilities:
        super::bounded_map::BoundedMap<String, raep_types::ClientCapabilities>,
    /// Everything about the turns themselves: running, queued and named.
    pub(super) turns: super::turn_book::TurnBook,
    pub(super) turn_request_fingerprints: HashMap<(String, TurnId), String>,
    pub(super) turn_request_frames: HashMap<(String, TurnId), Envelope<Value>>,
    pub(super) turn_request_order: VecDeque<(String, TurnId)>,
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
    pub(super) path: PathBuf,
    pub(super) owner: Option<String>,
    pub(super) expires_at: i64,
}

pub(super) struct StoredImport {
    pub(super) path: PathBuf,
    pub(super) owner: String,
    pub(super) expires_at: i64,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct StoredImportPayload {
    pub(super) manifest: crate::bundle::BundleManifest,
    pub(super) history: Vec<norte_code_agent::service::ImportedHistoryItem>,
    pub(super) title: Option<String>,
}
