use serde::Serialize;

#[derive(Serialize)]
pub struct UserInputPart {
    #[serde(rename = "type")]
    pub kind: InputPartKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<UploadSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputPartKind {
    Text,
    Image,
    Document,
    Audio,
    Video,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum UploadSource {
    Upload { upload_id: String },
}

impl UserInputPart {
    /// A text part, used when a prompt travels with attachments.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: InputPartKind::Text,
            text: Some(text.into()),
            source: None,
            mime_type: None,
        }
    }

    pub fn upload(kind: InputPartKind, upload_id: String, mime_type: String) -> Self {
        Self {
            kind,
            text: None,
            source: Some(UploadSource::Upload { upload_id }),
            mime_type: Some(mime_type),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InputPartKind, UserInputPart};

    #[test]
    fn uploaded_media_uses_raep_content_part_shape() {
        let parts = vec![UserInputPart::upload(
            InputPartKind::Audio,
            "upl_123".to_owned(),
            "audio/wav".to_owned(),
        )];
        assert_eq!(
            serde_json::to_value(parts).expect("typed parts serialize"),
            serde_json::json!([{
                    "type": "audio",
                    "source": {"kind": "upload", "upload_id": "upl_123"},
                    "mime_type": "audio/wav"
            }])
        );
    }
}
