use norte::{Error, MediaPart, Result, ToolOutput};
use serde_json::Value;

use crate::user_message_draft::UserMessageDraft;

/// The reserved key in `ToolOutput.structured` that carries host instructions.
pub const HOST_EFFECTS_KEY: &str = "norte_code.host_effects";

/// Instructions a plugin tool gives the host alongside its normal result.
///
/// The envelope is serialized into the tool result at the plugin boundary and
/// removed by the host before the result is stored or sent to a model.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct HostEffects {
    /// User-role messages the plugin asks the host to add.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_messages: Vec<UserMessageDraft>,
    /// Files to publish in the current session. Each part carries inline bytes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<MediaPart>,
}

impl HostEffects {
    /// Builds an envelope carrying one user message.
    #[must_use]
    pub fn user_message(draft: UserMessageDraft) -> Self {
        Self {
            user_messages: vec![draft],
            artifacts: Vec::new(),
        }
    }

    /// Builds an envelope carrying several user messages.
    #[must_use]
    pub fn user_messages(user_messages: Vec<UserMessageDraft>) -> Self {
        Self {
            user_messages,
            artifacts: Vec::new(),
        }
    }

    /// Builds an effect that publishes files for the client.
    #[must_use]
    pub fn artifacts(artifacts: Vec<MediaPart>) -> Self {
        Self {
            user_messages: Vec::new(),
            artifacts,
        }
    }

    /// Attaches the envelope to a tool result.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the result has no object-shaped
    /// structured value or already carries an envelope.
    pub fn attach(self, mut output: ToolOutput) -> Result<ToolOutput> {
        let Some(structured) = output.structured else {
            return Err(Error::InvalidInput(
                "a host effect needs an object-shaped tool result".to_owned(),
            ));
        };
        let Value::Object(mut fields) = structured else {
            return Err(Error::InvalidInput(
                "a host effect needs an object-shaped tool result".to_owned(),
            ));
        };
        if fields.contains_key(HOST_EFFECTS_KEY) {
            return Err(Error::InvalidInput(
                "a tool result already carries host effects".to_owned(),
            ));
        }
        fields.insert(HOST_EFFECTS_KEY.to_owned(), serde_json::to_value(self)?);
        output.structured = Some(Value::Object(fields));
        Ok(output)
    }

    /// Removes and parses the envelope from a tool result.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the key is present but the envelope
    /// is malformed. The key is removed in that case so the bad payload never
    /// reaches a model.
    pub fn extract(output: &mut ToolOutput) -> Result<Option<Self>> {
        let Some(Value::Object(fields)) = output.structured.as_mut() else {
            return Ok(None);
        };
        let Some(value) = fields.remove(HOST_EFFECTS_KEY) else {
            return Ok(None);
        };
        serde_json::from_value(value)
            .map(Some)
            .map_err(|error| Error::InvalidInput(format!("invalid host effects: {error}")))
    }
}
