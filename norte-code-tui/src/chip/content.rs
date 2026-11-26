//! What a chip carries.

use norte_code_wire::{UploadReceipt, UserInputPart};
use serde::Serialize;

use crate::attachment::AttachmentKind;

/// Text exactly as the reader pasted it or wrote it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct OriginalText(String);

impl OriginalText {
    /// Wrap text without changing a character of it.
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// The original text.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// An artifact the host has already stored.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct UploadedArtifact {
    receipt: UploadReceipt,
    part: UserInputPart,
}

impl UploadedArtifact {
    /// Build an artifact from the host's receipt.
    pub(crate) fn new(kind: AttachmentKind, receipt: UploadReceipt) -> Self {
        let part = UserInputPart::upload(
            kind.part_kind(),
            receipt.upload_id.clone(),
            receipt.mime_type.clone(),
        );
        Self { receipt, part }
    }

    /// The part that travels with the message.
    pub(crate) fn part(&self) -> &UserInputPart {
        &self.part
    }
}

/// The content behind a chip.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum ChipContent {
    /// Text that lives only in the client draft.
    Text(OriginalText),
    /// Bytes the host stored and that travel as a content part.
    Upload(UploadedArtifact),
    #[serde(skip)]
    Pending(crate::attachment::PreparedAttachment),
}
