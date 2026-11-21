//! A deterministic clipboard for tests.

use std::sync::Mutex;

use super::content::ClipboardContent;
use super::error::ClipboardError;
use super::sink::ClipboardSink;
use super::source::ClipboardSource;

/// A clipboard that replays one payload and records every write.
#[derive(Debug, Default)]
pub(crate) struct FakeClipboard {
    content: Mutex<Option<ClipboardContent>>,
    written: Mutex<Vec<String>>,
}

impl FakeClipboard {
    /// A clipboard holding one payload.
    pub(crate) fn with(content: ClipboardContent) -> Self {
        Self {
            content: Mutex::new(Some(content)),
            written: Mutex::new(Vec::new()),
        }
    }

    /// Everything written so far, oldest first.
    pub(crate) fn written(&self) -> Vec<String> {
        self.written
            .lock()
            .expect("clipboard log is not poisoned")
            .clone()
    }
}

impl ClipboardSource for FakeClipboard {
    fn read(&self) -> Result<ClipboardContent, ClipboardError> {
        self.content
            .lock()
            .expect("clipboard content is not poisoned")
            .clone()
            .ok_or_else(|| ClipboardError::Unavailable {
                reason: "clipboard is empty".to_owned(),
            })
    }
}

impl ClipboardSink for FakeClipboard {
    fn write(&self, text: &str) -> Result<(), ClipboardError> {
        self.written
            .lock()
            .expect("clipboard log is not poisoned")
            .push(text.to_owned());
        Ok(())
    }
}
