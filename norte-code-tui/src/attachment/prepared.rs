//! A validated attachment waiting for the host to store its bytes.

use std::path::PathBuf;

use super::kind::AttachmentKind;
use super::mime::MimeType;
use super::notice::AttachmentNotice;

/// The bytes or path the client will upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AttachmentSource {
    /// PNG bytes read from the clipboard image.
    Image(Vec<u8>),
    /// A file the platform clipboard pointed at.
    File(PathBuf),
}

/// An attachment whose media type and kind are already known.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedAttachment {
    /// The kind used for the chip and the model gate.
    pub(crate) kind: AttachmentKind,
    /// The validated media type.
    pub(crate) mime: MimeType,
    /// Where the bytes come from.
    pub(crate) source: AttachmentSource,
    /// Where the composer wants the chip inserted.
    pub(crate) cursor: usize,
}

/// What the composer asks the app to do after a paste.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ComposerAction {
    /// Store these bytes and add a chip once the host confirms.
    Attach(PreparedAttachment),
    /// Show a reader-facing notice.
    Notice(AttachmentNotice),
}
