//! Clipboard double behavior.

use std::sync::Arc;

use super::{Clipboard, ClipboardContent, FakeClipboard};

#[test]
fn the_fake_clipboard_replays_one_payload() {
    let clipboard = FakeClipboard::with(ClipboardContent::Text("text".to_owned()));
    let shared: Arc<dyn Clipboard> = Arc::new(clipboard);
    assert_eq!(
        shared.read().unwrap(),
        ClipboardContent::Text("text".to_owned())
    );
}

#[test]
fn the_fake_clipboard_records_every_write() {
    let clipboard = Arc::new(FakeClipboard::default());
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    shared.write("first").unwrap();
    shared.write("second").unwrap();
    assert_eq!(clipboard.written(), vec!["first", "second"]);
}
