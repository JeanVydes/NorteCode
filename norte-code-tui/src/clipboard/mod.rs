//! One system clipboard for text, images and files.

use std::sync::{Arc, OnceLock};

mod content;
mod error;
#[cfg(test)]
mod fake;
mod image;
mod sink;
mod source;
mod system;

pub(crate) use content::{ClipboardContent, ClipboardFile, ClipboardImage};
pub(crate) use sink::ClipboardSink;
pub(crate) use source::ClipboardSource;
pub(crate) use system::SystemClipboard;

/// The single clipboard object shared by the composer and message actions.
pub(crate) trait Clipboard: ClipboardSource + ClipboardSink {}

impl<T: ClipboardSource + ClipboardSink + ?Sized> Clipboard for T {}

static SYSTEM: OnceLock<Arc<dyn Clipboard>> = OnceLock::new();

/// The platform clipboard, created once for the whole application.
pub(crate) fn system() -> Arc<dyn Clipboard> {
    SYSTEM.get_or_init(|| Arc::new(SystemClipboard)).clone()
}

#[cfg(test)]
pub(crate) use fake::FakeClipboard;

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;
