//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use crate::payloads::payload;
use raep_types::{Envelope, StandardFrameType};
use serde_json::Value;
use std::collections::BTreeMap;

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::capabilities::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::payloads::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::stores::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;

impl NorteCodeRaepHost {
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
    /// Stores an authenticated upload scoped to one session.
    ///
    /// # Errors
    ///
    /// Returns an error when uploads are disabled, the MIME type is not
    /// supported, the upload exceeds the configured limit, or storage fails.
    pub async fn put_session_upload_as(
        &self,
        session_id: &str,
        mime_type: &str,
        bytes: &[u8],
        owner: Option<&str>,
    ) -> Result<UploadReceipt, HostError> {
        if self
            .session_summary(session_id, owner.unwrap_or_default())
            .await
            .is_none()
        {
            return Err(HostError::Artifact("session not found".to_owned()));
        }
        if bytes.is_empty() {
            return Err(HostError::Artifact("upload must not be empty".to_owned()));
        }
        if bytes.len() > self.config.max_artifact_bytes {
            return Err(HostError::Artifact(format!(
                "upload exceeds {} byte limit",
                self.config.max_artifact_bytes
            )));
        }
        let kind = if mime_type.starts_with("image/") {
            AcceptedPartKind::Image
        } else if mime_type.starts_with("audio/") {
            AcceptedPartKind::Audio
        } else if mime_type.starts_with("video/") {
            AcceptedPartKind::Video
        } else {
            AcceptedPartKind::Document
        };
        validate_media_mime(kind, mime_type).map_err(HostError::Artifact)?;
        let artifact_store = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("artifact storage is disabled".to_owned()))?;
        let upload_index = self
            .upload_index
            .as_ref()
            .ok_or_else(|| HostError::Artifact("upload metadata storage is disabled".to_owned()))?;
        let artifact = artifact_store.put(mime_type, bytes).await?;
        let upload_id = format!("upl_{}", uuid::Uuid::new_v4());
        let expires_at = now_millis().saturating_add(60 * 60 * 1000);
        let sha256 = crate::bundle::sha256(bytes);
        upload_index
            .insert(UploadRecord {
                id: upload_id.clone(),
                session_id: session_id.to_owned(),
                owner: owner.map(str::to_owned),
                blob_id: artifact.id,
                mime_type: mime_type.to_owned(),
                bytes: bytes.len() as u64,
                sha256: sha256.clone(),
                expires_at,
                referenced: false,
            })
            .await?;
        Ok(UploadReceipt {
            upload_id,
            mime_type: mime_type.to_owned(),
            bytes: bytes.len() as u64,
            sha256,
            expires_at,
        })
    }
    pub(crate) async fn resolve_session_asset(
        &self,
        session_id: &str,
        asset_id: &str,
        owner: Option<&str>,
        output: bool,
    ) -> Result<UploadRecord, HostError> {
        let index = self
            .upload_index
            .as_ref()
            .ok_or_else(|| HostError::Artifact("upload metadata storage is disabled".to_owned()))?;
        let expected_prefix = if output { "out_" } else { "upl_" };
        let record = index
            .get(asset_id)
            .await?
            .filter(|record| {
                record.session_id == session_id
                    && record.owner.as_deref() == owner
                    && (record.referenced || record.expires_at > now_millis())
                    && record.id.starts_with(expected_prefix)
                    && (!output || record.referenced)
            })
            .ok_or_else(|| HostError::Artifact("upload not found".to_owned()))?;
        Ok(record)
    }
    /// Fetches one generated output after checking its session and owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the output is unknown, expired, foreign, or the
    /// backing blob cannot be read.
    pub async fn get_session_output_as(
        &self,
        session_id: &str,
        output_id: &str,
        owner: Option<&str>,
    ) -> Result<(Vec<u8>, String), HostError> {
        let record = self
            .resolve_session_asset(session_id, output_id, owner, true)
            .await?;
        let bytes = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("artifact storage is disabled".to_owned()))?
            .get(&record.blob_id)
            .await?;
        Ok((bytes, record.mime_type))
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
    /// Reads an unexpired portable bundle after checking its owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the export is missing, expired, or owned by a
    /// different principal.
    pub async fn get_export_as(
        &self,
        id: &str,
        principal: &str,
    ) -> Result<(Vec<u8>, i64), HostError> {
        let mut state = self.state.lock().await;
        if state
            .exports
            .get(id)
            .is_some_and(|export| export.expires_at <= now_millis())
        {
            state.exports.remove(id);
        }
        let export = state
            .exports
            .get(id)
            .ok_or_else(|| HostError::Artifact("export not found or expired".to_owned()))?;
        if export
            .owner
            .as_deref()
            .is_some_and(|owner| owner != principal)
        {
            return Err(HostError::Artifact(
                "export not found or expired".to_owned(),
            ));
        }
        Ok((export.bytes.clone(), export.expires_at))
    }
    /// Validates a portable bundle and stores a one-hour import preview.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid/oversized bundle, unsupported history
    /// content, or a preview store failure.
    pub async fn preview_import_as(
        &self,
        bytes: &[u8],
        principal: &str,
    ) -> Result<Value, HostError> {
        if bytes.len() > self.config.max_export_bytes {
            return Err(HostError::InvalidFrame(
                "PAYLOAD_TOO_LARGE: import exceeds max_export_bytes".to_owned(),
            ));
        }
        let (manifest, files) = crate::bundle::read_bundle(bytes, self.config.max_export_bytes)
            .map_err(|error| HostError::InvalidFrame(format!("BUNDLE_INVALID: {error}")))?;
        if manifest.sessions.len() != 1
            || manifest.sessions[0].session_id != manifest.root_session_id
            || manifest.sessions[0].parent_session_id.is_some()
            || files.len() != 2
        {
            return Err(HostError::InvalidFrame(
                "BUNDLE_UNSUPPORTED: this host imports a single text-only root session".to_owned(),
            ));
        }
        let session = &manifest.sessions[0];
        if let Some(usage) = session.usage_total.as_ref() {
            validate_bundle_usage(usage)?;
        }
        let items = parse_portable_history(files.get(&session.history_file).ok_or_else(|| {
            HostError::InvalidFrame("BUNDLE_INVALID: history file is missing".to_owned())
        })?)?;
        let session_json_path = format!("sessions/{}/session.json", session.session_id);
        let session_json: Value =
            serde_json::from_slice(files.get(&session_json_path).ok_or_else(|| {
                HostError::InvalidFrame("BUNDLE_INVALID: session.json is missing".to_owned())
            })?)?;
        let title = session_json
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if title.as_ref().is_some_and(|title| title.len() > 512) {
            return Err(HostError::InvalidFrame(
                "BUNDLE_INVALID: session title exceeds 512 bytes".to_owned(),
            ));
        }
        let agent_id = session
            .agent
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                HostError::InvalidFrame("BUNDLE_INVALID: session agent id is missing".to_owned())
            })?;
        let version = session
            .agent
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let available = self.services.contains_key(agent_id)
            && (version.is_empty() || version == self.config.server_version);
        let expires_at = now_millis().saturating_add(60 * 60 * 1000);
        let import_id = format!("imp_{}", uuid::Uuid::new_v4().simple());
        {
            let mut state = self.state.lock().await;
            state
                .imports
                .retain(|_, import| import.expires_at > now_millis());
            if state.imports.len() >= 4 {
                return Err(HostError::InvalidFrame(
                    "IMPORT_STORE_FULL: active preview limit reached".to_owned(),
                ));
            }
            state.imports.insert(
                import_id.clone(),
                StoredImport {
                    manifest: manifest.clone(),
                    history: items,
                    title,
                    owner: principal.to_owned(),
                    expires_at,
                },
            );
        }
        Ok(payload(ImportPreviewPayload {
            import_id: &import_id,
            bundle_version: &manifest.bundle_version,
            sessions: manifest.sessions.len(),
            signature: ImportSignatureStatus::Absent,
            agents: vec![ImportAgentAvailability {
                agent_id,
                version,
                available,
            }],
            warnings: [
                "Only plain-text user and assistant history is restored.",
                "Children, uploads, job output, generated outputs, and workspace data are not imported.",
            ],
            expires_at,
        }))
    }
    /// Commits a previously validated portable bundle preview.
    ///
    /// # Errors
    ///
    /// Returns an error for an expired/foreign preview, unavailable agent,
    /// unsupported usage carry-over, or a failed session creation.
    pub(crate) async fn commit_import_as(
        &self,
        import_id: &str,
        request: ImportCommitRequest,
        principal: &str,
    ) -> Result<Value, HostError> {
        if request.decryption_jwk.is_some() {
            return Err(HostError::InvalidFrame(
                "encrypted imports are not supported".to_owned(),
            ));
        }
        let imported = {
            let mut state = self.state.lock().await;
            if state
                .imports
                .get(import_id)
                .is_some_and(|import| import.expires_at <= now_millis())
            {
                state.imports.remove(import_id);
            }
            let imported = state
                .imports
                .get(import_id)
                .ok_or_else(|| HostError::InvalidFrame("IMPORT_NOT_FOUND".to_owned()))?;
            if imported.owner != principal {
                return Err(HostError::InvalidFrame("IMPORT_NOT_FOUND".to_owned()));
            }
            imported.clone()
        };
        let source = &imported.manifest.sessions[0];
        let usage_total = request
            .carry_usage
            .unwrap_or(true)
            .then(|| source.usage_total.clone())
            .flatten();
        let agent_id = source
            .agent
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                HostError::InvalidFrame("BUNDLE_INVALID: missing agent id".to_owned())
            })?;
        let version = source
            .agent
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mapped = request
            .agent_map
            .get(&format!("{agent_id}@{version}"))
            .or_else(|| request.agent_map.get(agent_id))
            .map_or(agent_id, String::as_str);
        let service = self.services.get(mapped).cloned().ok_or_else(|| {
            HostError::InvalidFrame(format!("AGENT_NOT_FOUND: `{mapped}` is unavailable"))
        })?;
        {
            let mut state = self.state.lock().await;
            let can_consume = state.imports.get(import_id).is_some_and(|import| {
                import.owner == principal && import.expires_at > now_millis()
            });
            if !can_consume {
                return Err(HostError::InvalidFrame("IMPORT_NOT_FOUND".to_owned()));
            }
            state.imports.remove(import_id);
        }
        let imported_from = payload(ImportedFromPayload {
            exporter: &imported.manifest.exporter,
            session_id: &source.session_id,
            exported_at: imported.manifest.exported_at,
        });
        let key = service
            .import_plain_history(imported.title, imported.history, imported_from, usage_total)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        {
            let mut state = self.state.lock().await;
            state
                .session_agents
                .insert(key.to_string(), mapped.to_owned());
            state
                .session_owners
                .insert(key.to_string(), principal.to_owned());
        }
        let id_map = BTreeMap::from([(source.session_id.clone(), key.to_string())]);
        Ok(payload(ImportCommittedPayload {
            session_id: key.as_str(),
            id_map,
            warnings: [
                "Only plain-text user and assistant history was restored.",
                "The imported session is suspended until its first controller attachment.",
            ],
        }))
    }
    /// Removes an import preview owned by `principal`.
    pub async fn delete_import_as(&self, import_id: &str, principal: &str) -> bool {
        let mut state = self.state.lock().await;
        if state
            .imports
            .get(import_id)
            .is_some_and(|import| import.owner == principal)
        {
            state.imports.remove(import_id);
            true
        } else {
            false
        }
    }
    pub(crate) async fn load_generated_media(
        &self,
        media: &norte_code_wire::MediaPart,
    ) -> Result<(OutputPartKind, String, Vec<u8>), HostError> {
        let (kind, input_kind) = match media.kind {
            norte_code_wire::MediaKind::Image => (OutputPartKind::Image, AcceptedPartKind::Image),
            norte_code_wire::MediaKind::Document => {
                (OutputPartKind::Document, AcceptedPartKind::Document)
            }
            norte_code_wire::MediaKind::Audio => (OutputPartKind::Audio, AcceptedPartKind::Audio),
            norte_code_wire::MediaKind::Video => (OutputPartKind::Video, AcceptedPartKind::Video),
        };
        let mime_type = media.mime.as_str();
        validate_media_mime(input_kind, mime_type).map_err(HostError::Artifact)?;
        let artifact_store = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("output storage is disabled".to_owned()))?;
        let bytes = match &media.source {
            norte_code_wire::MediaSource::Inline(bytes) => bytes.clone(),
            norte_code_wire::MediaSource::Blob(id) => artifact_store.get(id).await?,
            norte_code_wire::MediaSource::ProviderFile(reference) => {
                let blob = reference.blob.as_deref().ok_or_else(|| {
                    HostError::Artifact(
                        "generated provider file has no durable blob reference".to_owned(),
                    )
                })?;
                artifact_store.get(blob).await?
            }
            norte_code_wire::MediaSource::Url(_) => {
                return Err(HostError::Artifact(
                    "generated media URL was not materialized into the blob store".to_owned(),
                ));
            }
        };
        let output_limit = self.output_part_bytes_limit();
        if bytes.is_empty() || bytes.len() > output_limit {
            return Err(HostError::Artifact(format!(
                "generated media must be between 1 and {output_limit} bytes"
            )));
        }
        Ok((kind, mime_type.to_owned(), bytes))
    }
    pub(crate) async fn emit_media_output(
        &self,
        session_id: &str,
        turn: u64,
        index: u32,
        media: &norte_code_wire::MediaPart,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let (kind, mime_type, bytes) = self.load_generated_media(media).await?;
        let artifact_store = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("output storage is disabled".to_owned()))?;
        let upload_index = self
            .upload_index
            .as_ref()
            .ok_or_else(|| HostError::Artifact("output metadata storage is disabled".to_owned()))?;
        let blob = artifact_store.put(&mime_type, &bytes).await?;
        let output_id = format!("out_{}", uuid::Uuid::new_v4());
        let sha256 = crate::bundle::sha256(&bytes);
        let owner = self
            .state
            .lock()
            .await
            .session_owners
            .get(session_id)
            .cloned();
        upload_index
            .insert(UploadRecord {
                id: output_id.clone(),
                session_id: session_id.to_owned(),
                owner,
                blob_id: blob.id,
                mime_type: mime_type.clone(),
                bytes: bytes.len() as u64,
                sha256: sha256.clone(),
                expires_at: i64::MAX,
                referenced: true,
            })
            .await?;
        let name = media.metadata.extension.get("name").and_then(Value::as_str);
        let alt = media.metadata.extension.get("alt").and_then(Value::as_str);
        let started = self
            .emit_with_turn(
                session_id,
                standard(StandardFrameType::OutputPartStarted),
                payload(OutputPartStartedPayload {
                    part_id: &output_id,
                    index,
                    kind,
                    mime_type: &mime_type,
                    streamed: false,
                    name,
                    alt,
                    duration_ms: media.metadata.duration_ms,
                    width: media.metadata.width,
                    height: media.metadata.height,
                    bytes_expected: bytes.len(),
                }),
                Some(turn),
                None,
                None,
            )
            .await?;
        let finished = self
            .emit_with_turn(
                session_id,
                standard(StandardFrameType::OutputPartFinished),
                payload(OutputPartFinishedPayload {
                    part_id: &output_id,
                    index,
                    status: OutputPartStatus::Ok,
                    source: OutputPartSource::Output {
                        output_id: &output_id,
                    },
                    bytes: bytes.len(),
                    sha256: &sha256,
                    transcript: media.metadata.transcript.as_deref(),
                }),
                Some(turn),
                None,
                None,
            )
            .await?;
        Ok(vec![started, finished])
    }
}
