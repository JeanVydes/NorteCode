//! One atomic piece of the composer draft.

use norte_code_wire::{UploadReceipt, UserInputPart};
use serde::Serialize;

use super::content::{ChipContent, OriginalText, UploadedArtifact};
use super::label::ChipLabel;
use crate::attachment::AttachmentKind;

/// The client's stable identity for a chip.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ChipId(u64);

impl ChipId {
    /// Wrap a client-generated identifier.
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// What kind of content a chip holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum ChipKind {
    /// A long paste.
    Text,
    /// A picture.
    Image,
    /// A file.
    File,
    /// A recording.
    Audio,
    /// A moving picture.
    Video,
}

impl ChipKind {
    /// Map an attachment kind to a chip kind.
    pub(crate) const fn from_attachment(kind: AttachmentKind) -> Self {
        match kind {
            AttachmentKind::Image => Self::Image,
            AttachmentKind::File => Self::File,
            AttachmentKind::Audio => Self::Audio,
            AttachmentKind::Video => Self::Video,
        }
    }

    /// The attachment kind behind an upload chip.
    pub(crate) const fn attachment(self) -> Option<AttachmentKind> {
        match self {
            Self::Text => None,
            Self::Image => Some(AttachmentKind::Image),
            Self::File => Some(AttachmentKind::File),
            Self::Audio => Some(AttachmentKind::Audio),
            Self::Video => Some(AttachmentKind::Video),
        }
    }
}

/// One atomic content unit in the composer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct Chip {
    id: ChipId,
    label: ChipLabel,
    content: ChipContent,
    kind: ChipKind,
}

impl Chip {
    pub(crate) fn pending(id: ChipId, attachment: crate::attachment::PreparedAttachment) -> Self {
        let kind = attachment.kind;
        Self {
            id,
            label: ChipLabel::uploaded(kind, 1),
            content: ChipContent::Pending(attachment),
            kind: ChipKind::from_attachment(kind),
        }
    }

    pub(crate) fn pending_attachment(&self) -> Option<&crate::attachment::PreparedAttachment> {
        match &self.content {
            ChipContent::Pending(attachment) => Some(attachment),
            _ => None,
        }
    }
    /// A chip that stands for a long paste.
    pub(crate) fn text(id: ChipId, text: OriginalText) -> Self {
        let label = ChipLabel::copied(&text);
        Self {
            id,
            label,
            content: ChipContent::Text(text),
            kind: ChipKind::Text,
        }
    }

    /// A chip that stands for an uploaded artifact.
    pub(crate) fn upload(
        id: ChipId,
        kind: AttachmentKind,
        receipt: UploadReceipt,
        label: ChipLabel,
    ) -> Self {
        Self {
            id,
            label,
            content: ChipContent::Upload(UploadedArtifact::new(kind, receipt)),
            kind: ChipKind::from_attachment(kind),
        }
    }

    /// The chip's identity.
    pub(crate) const fn id(&self) -> ChipId {
        self.id
    }

    /// The chip's drawn label.
    pub(crate) fn label(&self) -> &ChipLabel {
        &self.label
    }

    /// The chip's kind.
    pub(crate) const fn kind(&self) -> ChipKind {
        self.kind
    }

    /// What the real message contains in this chip's place.
    pub(crate) fn expanded_text(&self) -> String {
        match &self.content {
            ChipContent::Text(text) => text.as_str().to_owned(),
            ChipContent::Upload(_) | ChipContent::Pending(_) => self.label.as_str().to_owned(),
        }
    }

    /// The wire part this chip contributes, when it has one.
    pub(crate) fn wire_part(&self) -> Option<UserInputPart> {
        match &self.content {
            ChipContent::Text(_) | ChipContent::Pending(_) => None,
            ChipContent::Upload(artifact) => Some(artifact.part().clone()),
        }
    }

    /// Replace the label after chips have been renumbered.
    pub(crate) fn set_label(&mut self, label: ChipLabel) {
        self.label = label;
    }
}
