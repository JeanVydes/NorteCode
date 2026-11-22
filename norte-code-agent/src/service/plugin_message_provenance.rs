use norte::domain::message::Message;
use norte::domain::part::{ExtensionPart, Part};
use norte::id::CallId;
use norte_code_wire::PluginId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const NAMESPACE: &str = "norte-code/plugin-message";

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct PluginMessageProvenance {
    pub(super) plugin_id: PluginId,
    pub(super) call_id: String,
}

impl PluginMessageProvenance {
    pub(super) fn part(plugin_id: &PluginId, call_id: CallId) -> Option<Part> {
        let data = serde_json::to_value(Self {
            plugin_id: plugin_id.clone(),
            call_id: call_id.to_string(),
        })
        .ok()?;
        Some(Part::Extension(ExtensionPart {
            namespace: NAMESPACE.to_owned(),
            data,
        }))
    }

    pub(super) fn from_message(message: &Message) -> Option<Value> {
        message.parts.iter().find_map(|part| match part {
            Part::Extension(extension) if extension.namespace == NAMESPACE => {
                serde_json::from_value::<Self>(extension.data.clone())
                    .ok()
                    .and_then(|provenance| serde_json::to_value(provenance).ok())
            }
            _ => None,
        })
    }
}
