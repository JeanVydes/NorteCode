use norte::domain::part::{ExtensionPart, Part};
use norte::{BlobStore, MediaPart, MediaSource, Message, ToolOutput};
use norte_code_wire::PluginId;
use serde::{Deserialize, Serialize};

const NAMESPACE: &str = "norte-code/plugin-artifact";

#[derive(Deserialize, Serialize)]
pub(crate) struct PluginArtifact {
    pub(crate) plugin_id: PluginId,
    pub(crate) call_id: String,
    pub(crate) media: Part,
}

impl PluginArtifact {
    pub(crate) fn read(extension: &ExtensionPart) -> Option<Self> {
        (extension.namespace == NAMESPACE)
            .then(|| serde_json::from_value(extension.data.clone()).ok())
            .flatten()
    }

    pub(crate) async fn publish(
        output: &mut ToolOutput,
        artifacts: Vec<MediaPart>,
        plugin_id: &PluginId,
        call_id: norte::CallId,
        max_bytes: usize,
        blobs: &dyn BlobStore,
    ) -> norte::Result<()> {
        let mut remaining = max_bytes;
        for media in &artifacts {
            let MediaSource::Inline(bytes) = &media.source else {
                return Err(norte::Error::InvalidInput(
                    "plugin artifacts require inline bytes".to_owned(),
                ));
            };
            if bytes.is_empty() || bytes.len() > remaining {
                return Err(norte::Error::InvalidInput(
                    "plugin artifacts exceed the per-call byte limit or contain an empty file"
                        .to_owned(),
                ));
            }
            remaining -= bytes.len();
        }
        let mut published = Vec::new();
        for mut media in artifacts {
            let MediaSource::Inline(bytes) = &media.source else {
                return Err(norte::Error::InvalidInput(
                    "plugin artifacts require inline bytes".to_owned(),
                ));
            };
            media.source = MediaSource::Blob(blobs.put(bytes).await?);
            published.push(Part::Extension(ExtensionPart {
                namespace: NAMESPACE.to_owned(),
                data: serde_json::to_value(Self {
                    plugin_id: plugin_id.clone(),
                    call_id: call_id.to_string(),
                    media: Part::Media(media),
                })?,
            }));
        }
        if !published.is_empty()
            && output.parts.is_empty()
            && let Some(value) = &output.structured
        {
            output.parts.push(Part::text(value.to_string()));
        }
        output.parts.extend(published);
        Ok(())
    }

    pub(crate) fn omit_from_request(message: &mut Message) {
        fn clean(parts: &mut Vec<Part>) {
            parts.retain(|part| !matches!(part, Part::Extension(extension) if extension.namespace == NAMESPACE));
            for part in parts {
                if let Part::ToolResult(result) = part {
                    clean(&mut result.parts);
                }
            }
        }
        clean(&mut message.parts);
    }
}
