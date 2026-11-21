//! App-level clipboard routing and upload event folding.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use norte_code_wire::UploadReceipt;
use terminalui::{InputEvent, Theme, Tree, node};

use super::{fold_upload_event, handle_key};
use crate::attachment::{AttachmentKind, AttachmentSource, PreparedAttachment};
use crate::client::ClientEvent;
use crate::clipboard::{Clipboard, FakeClipboard};
use crate::ui::Composer;
use crate::ui_model::UiModel;

fn composer_tree(clipboard: Arc<dyn Clipboard>) -> Tree {
    let mut tree = Tree::new(node(
        "composer",
        Composer::new(&Theme::default(), clipboard),
    ));
    assert!(tree.focus("composer"));
    tree
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn receipt() -> UploadReceipt {
    UploadReceipt {
        upload_id: "upl_1".to_owned(),
        mime_type: "image/png".to_owned(),
        bytes: 12,
        sha256: "digest".to_owned(),
        expires_at: 0,
    }
}

fn image_event() -> ClientEvent {
    ClientEvent::Uploaded {
        attachment: PreparedAttachment {
            kind: AttachmentKind::Image,
            mime: crate::attachment::MimeType::png(),
            source: AttachmentSource::Image(vec![1, 2, 3]),
            cursor: 0,
        },
        receipt: receipt(),
    }
}

#[test]
fn ctrl_c_copies_the_composer_selection_without_quitting() {
    let clipboard = Arc::new(FakeClipboard::default());
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    for ch in "hello world".chars() {
        tree.handle_event(&InputEvent::Key(KeyEvent::new(
            KeyCode::Char(ch),
            KeyModifiers::NONE,
        )));
    }
    for _ in 0..5 {
        tree.handle_event(&InputEvent::Key(KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::SHIFT,
        )));
    }
    let mut model = UiModel::detached();
    let quit = handle_key(&mut tree, ctrl_c(), &mut None, &mut None, &mut model, None);
    assert!(!quit);
    assert_eq!(clipboard.written(), vec!["world"]);
}

#[test]
fn ctrl_c_without_a_selection_quits() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let mut model = UiModel::detached();
    assert!(handle_key(
        &mut tree,
        ctrl_c(),
        &mut None,
        &mut None,
        &mut model,
        None
    ));
}

#[test]
fn an_uploaded_image_gets_its_chip_and_wire_part() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    assert!(fold_upload_event(&image_event(), &mut tree));
    let composer = tree.find::<Composer>("composer").expect("composer");
    assert_eq!(composer.expanded_text(), "[Image #1]");
    assert_eq!(composer.wire_parts().len(), 1);
}

#[test]
fn a_failed_upload_leaves_no_chip() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let event = ClientEvent::UploadFailed {
        reason: "host refused the bytes".to_owned(),
    };
    assert!(fold_upload_event(&event, &mut tree));
    let composer = tree.find::<Composer>("composer").expect("composer");
    assert_eq!(composer.expanded_text(), "");
    assert_eq!(composer.wire_parts(), []);
}
