//! Attachments: media types, capabilities and reader-facing failures.

mod capability;
mod error;
mod kind;
mod mime;
mod notice;
mod prepared;

pub(crate) use capability::validate;
pub(crate) use error::AttachmentError;
pub(crate) use kind::AttachmentKind;
pub(crate) use mime::MimeType;
pub(crate) use notice::{AttachmentNotice, NoticeTone};
pub(crate) use prepared::{AttachmentSource, ComposerAction, PreparedAttachment};

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;
