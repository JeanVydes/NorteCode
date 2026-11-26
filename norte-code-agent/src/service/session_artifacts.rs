use std::collections::HashSet;

use norte::domain::part::Part;
use norte::{MediaPart, MediaSource, Role};
use norte_code_wire::{ArtifactId, ArtifactOrigin, SessionArtifact, SessionKey};

use super::plugin_message_provenance::PluginMessageProvenance;
use super::turn_history::compaction_ancestors;
use super::{Service, ServiceError};

impl Service {
    /// Returns the stored media references in a session's conversation.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the session history cannot be read.
    pub async fn session_artifacts(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<SessionArtifact>, ServiceError> {
        let mut artifacts = Vec::new();
        for (mut item, inline) in self.artifact_parts(key).await? {
            if inline.is_none() {
                let blob = item.blob_id.parse()?;
                item.bytes = if let Some(bytes) = self.directory.blob_size(blob).await? {
                    bytes
                } else {
                    match self.blobs.get(blob).await {
                        Ok(bytes) => bytes.len() as u64,
                        Err(norte::Error::BlobNotFound(_)) => continue,
                        Err(error) => return Err(error.into()),
                    }
                };
            }
            artifacts.push(item);
        }
        Ok(artifacts)
    }

    /// Reads a file referenced by a session's conversation.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the session history or blob cannot be read.
    pub async fn session_artifact_bytes(
        &self,
        key: &SessionKey,
        id: &str,
    ) -> Result<Option<(Vec<u8>, String)>, ServiceError> {
        for (item, inline) in self.artifact_parts(key).await? {
            if item.id.as_str() == id {
                let bytes = match inline {
                    Some(bytes) => bytes,
                    None => self.blobs.get(item.blob_id.parse()?).await?,
                };
                return Ok(Some((bytes, item.mime_type)));
            }
        }
        Ok(None)
    }

    async fn artifact_parts(&self, key: &SessionKey) -> Result<Vec<StoredPart>, ServiceError> {
        let (current, _, _) = self.require_record(key)?;
        let historical = self
            .lock_sessions()
            .get(key)
            .into_iter()
            .flat_map(|record| {
                record
                    .turn_boundaries
                    .iter()
                    .map(|boundary| boundary.session)
            })
            .collect::<Vec<_>>();
        let mut sessions = vec![current];
        sessions.extend(historical.into_iter().rev());
        sessions.extend(compaction_ancestors(self.directory.as_ref(), current).await);
        let mut visited_sessions = HashSet::new();
        let mut visited_nodes = HashSet::new();
        let mut artifacts = Vec::new();
        for session in sessions
            .into_iter()
            .filter(|id| visited_sessions.insert(*id))
        {
            let state = self.store.load(session).await?;
            if let Some(head) = state.head {
                for id in self.store.ancestors(head).await?.into_iter().rev() {
                    if visited_nodes.insert(id) {
                        let node = self.store.node(id).await?;
                        let digest = node.id.to_string();
                        let origin = match node.message.role {
                            Role::User => ArtifactOrigin::UserUpload,
                            Role::Assistant => ArtifactOrigin::AssistantOutput {
                                turn: None,
                                part_index: None,
                            },
                            _ => ArtifactOrigin::Message,
                        };
                        collect_parts(
                            &node.message.parts,
                            &digest,
                            node.created_at.timestamp_millis(),
                            origin,
                            &mut artifacts,
                        );
                    }
                }
            }
        }
        Ok(artifacts)
    }
}

type StoredPart = (SessionArtifact, Option<Vec<u8>>);

fn collect_parts(
    parts: &[Part],
    parent: &str,
    created_at: i64,
    fallback: ArtifactOrigin,
    artifacts: &mut Vec<StoredPart>,
) {
    let origin = PluginMessageProvenance::from_parts(parts).map_or(fallback, |plugin| {
        ArtifactOrigin::Plugin {
            plugin_id: plugin.plugin_id,
            call_id: plugin.call_id,
        }
    });
    for (index, part) in parts.iter().enumerate() {
        let id = format!("{parent}.{index}");
        match part {
            Part::Media(media) => {
                if let Some(part) = stored_part(media, id, created_at, origin.clone()) {
                    artifacts.push(part);
                }
            }
            Part::ToolResult(result) => collect_parts(
                &result.parts,
                &id,
                created_at,
                ArtifactOrigin::Tool {
                    call_id: result.call_id.to_string(),
                },
                artifacts,
            ),
            Part::Extension(extension) => {
                if let Some(published) = crate::plugin_artifact::PluginArtifact::read(extension)
                    && let Part::Media(media) = &published.media
                    && let Some(part) = stored_part(
                        media,
                        id,
                        created_at,
                        ArtifactOrigin::Plugin {
                            plugin_id: published.plugin_id,
                            call_id: published.call_id,
                        },
                    )
                {
                    artifacts.push(part);
                }
            }
            _ => {}
        }
    }
}

fn stored_part(
    media: &MediaPart,
    id: String,
    created_at: i64,
    origin: ArtifactOrigin,
) -> Option<StoredPart> {
    let (blob, inline) = match &media.source {
        MediaSource::Blob(blob) => (*blob, None),
        MediaSource::ProviderFile(reference) => (reference.blob?, None),
        MediaSource::Inline(bytes) => (norte::BlobId::of(bytes), Some(bytes.clone())),
        _ => return None,
    };
    Some((
        SessionArtifact {
            id: ArtifactId::new(id),
            blob_id: blob.to_string(),
            mime_type: media.mime.to_string(),
            name: media
                .metadata
                .extension
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            bytes: inline.as_ref().map_or(0, |bytes| bytes.len() as u64),
            created_at: Some(created_at),
            origin,
        },
        inline,
    ))
}
