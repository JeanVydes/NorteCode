//! Sent-message history and composer paste, copy and cut behavior.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::{InputEvent, Tree, node};

use super::{Composer, History};
use crate::attachment::ComposerAction;
use crate::clipboard::{Clipboard, ClipboardContent, ClipboardFile, ClipboardImage, FakeClipboard};
use terminalui::Theme;

#[test]
fn older_is_none_when_history_is_empty() {
    let mut history = History::default();
    assert_eq!(history.older("draft"), None);
}

#[test]
fn older_recalls_newest_first_then_older() {
    let mut history = History::default();
    history.push("first");
    history.push("second");
    assert_eq!(history.older("draft").as_deref(), Some("second"));
    assert_eq!(history.older("second").as_deref(), Some("first"));
}

#[test]
fn older_at_the_oldest_entry_stops() {
    let mut history = History::default();
    history.push("only");
    assert_eq!(history.older("draft").as_deref(), Some("only"));
    assert_eq!(history.older("only"), None);
}

#[test]
fn newer_walks_back_to_the_stashed_draft() {
    let mut history = History::default();
    history.push("first");
    history.push("second");
    assert_eq!(history.older("unsent draft").as_deref(), Some("second"));
    assert_eq!(history.older("second").as_deref(), Some("first"));
    assert_eq!(history.newer().as_deref(), Some("second"));
    assert_eq!(history.newer().as_deref(), Some("unsent draft"));
    assert_eq!(history.newer(), None);
}

#[test]
fn browsing_resets_once_the_draft_is_restored() {
    let mut history = History::default();
    history.push("entry");
    assert_eq!(history.older("draft").as_deref(), Some("entry"));
    assert_eq!(history.newer().as_deref(), Some("draft"));
    assert_eq!(history.older("draft again").as_deref(), Some("entry"));
}

#[test]
fn consecutive_duplicates_are_collapsed() {
    let mut history = History::default();
    history.push("same");
    history.push("same");
    assert_eq!(history.entries.len(), 1);
}

#[test]
fn push_resets_an_active_browse() {
    let mut history = History::default();
    history.push("one");
    history.push("two");
    assert_eq!(history.older("draft").as_deref(), Some("two"));
    history.push("three");
    assert_eq!(history.browse, None);
    assert_eq!(history.older("x").as_deref(), Some("three"));
}

#[test]
fn the_composer_repaints_with_the_current_theme() {
    let dark = Theme::default();
    let other = Theme::for_ansi16();
    let clipboard: Arc<dyn Clipboard> = Arc::new(FakeClipboard::default());
    let mut tree = Tree::new(node("composer", Composer::new(&dark, clipboard)));
    let area = Rect::new(0, 0, 40, 6);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &other);
    let composer = tree
        .find_mut::<Composer>("composer")
        .expect("composer in tree");
    assert_eq!(composer.input.style.background, Some(other.secondary));
    assert_eq!(composer.input.style.foreground, Some(other.foreground));
    assert_eq!(composer.input.style.cursor, Some(other.accent));
}

fn composer_tree(clipboard: Arc<dyn Clipboard>) -> Tree {
    let mut tree = Tree::new(node(
        "composer",
        Composer::new(&Theme::default(), clipboard),
    ));
    assert!(tree.focus("composer"));
    tree
}

fn press(tree: &mut Tree, code: KeyCode, modifiers: KeyModifiers) {
    tree.handle_event(&InputEvent::Key(KeyEvent::new(code, modifiers)));
}

fn type_text(tree: &mut Tree, text: &str) {
    for ch in text.chars() {
        press(tree, KeyCode::Char(ch), KeyModifiers::NONE);
    }
}

#[test]
fn text_of_thirty_two_chars_stays_inline() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let text = "a".repeat(32);
    tree.handle_event(&InputEvent::Paste(text.clone()));
    let composer = tree.find::<Composer>("composer").expect("composer");
    assert_eq!(composer.text(), text);
    assert_eq!(composer.draft.spans(), []);
}

#[test]
fn text_of_thirty_three_chars_becomes_one_chip() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let text = "Lorem ipsum dolor sit amet consectetur";
    tree.handle_event(&InputEvent::Paste(text.to_owned()));
    let composer = tree.find::<Composer>("composer").expect("composer");
    assert_eq!(composer.text(), "[Copied: Lorem i...]");
    assert_eq!(composer.expanded_text(), text);
    assert_eq!(composer.draft.spans().len(), 1);
}

#[test]
fn ctrl_v_reads_the_system_clipboard() {
    let clipboard = Arc::new(FakeClipboard::with(ClipboardContent::Text("a".repeat(40))));
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    press(&mut tree, KeyCode::Char('v'), KeyModifiers::CONTROL);
    let composer = tree.find::<Composer>("composer").expect("composer");
    assert_eq!(composer.draft.spans().len(), 1);
}

#[test]
fn copying_a_chip_writes_the_original_text() {
    let clipboard = Arc::new(FakeClipboard::default());
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    let text = "Lorem ipsum dolor sit amet consectetur";
    tree.handle_event(&InputEvent::Paste(text.to_owned()));
    let composer = tree.find_mut::<Composer>("composer").expect("composer");
    assert!(composer.copy_selection());
    assert_eq!(clipboard.written(), vec![text]);
}

#[test]
fn copying_a_selection_writes_the_selected_text() {
    let clipboard = Arc::new(FakeClipboard::default());
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    type_text(&mut tree, "hello world");
    for _ in 0..5 {
        press(&mut tree, KeyCode::Left, KeyModifiers::SHIFT);
    }
    let composer = tree.find_mut::<Composer>("composer").expect("composer");
    assert!(composer.copy_selection());
    assert_eq!(clipboard.written(), vec!["world"]);
}

#[test]
fn ctrl_x_cuts_the_whole_chip() {
    let clipboard = Arc::new(FakeClipboard::default());
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    let text = "Lorem ipsum dolor sit amet consectetur";
    tree.handle_event(&InputEvent::Paste(text.to_owned()));
    press(&mut tree, KeyCode::Char('x'), KeyModifiers::CONTROL);
    let composer = tree.find::<Composer>("composer").expect("composer");
    assert_eq!(composer.text(), "");
    assert_eq!(clipboard.written(), vec![text]);
}

#[test]
fn ctrl_x_without_a_segment_does_not_touch_the_input_buffer() {
    let clipboard = Arc::new(FakeClipboard::default());
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    type_text(&mut tree, "hello");
    press(&mut tree, KeyCode::Char('x'), KeyModifiers::CONTROL);
    let composer = tree.find::<Composer>("composer").expect("composer");
    assert_eq!(composer.text(), "hello");
    assert_eq!(clipboard.written(), Vec::<String>::new());
}

#[test]
fn clipboard_image_needs_a_model_gate_before_a_chip() {
    let clipboard = Arc::new(FakeClipboard::with(ClipboardContent::Image(
        ClipboardImage {
            mime: crate::attachment::MimeType::png(),
            bytes: vec![1, 2, 3],
        },
    )));
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    press(&mut tree, KeyCode::Char('v'), KeyModifiers::CONTROL);
    let actions = tree
        .find_mut::<Composer>("composer")
        .expect("composer")
        .take_actions();
    assert!(matches!(actions.as_slice(), [ComposerAction::Attach(_)]));
}

#[test]
fn unsupported_mime_produces_only_a_notice() {
    let clipboard = Arc::new(FakeClipboard::with(ClipboardContent::Image(
        ClipboardImage {
            mime: crate::attachment::MimeType::parse("application/json").expect("valid media type"),
            bytes: vec![1, 2, 3],
        },
    )));
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    press(&mut tree, KeyCode::Char('v'), KeyModifiers::CONTROL);
    let composer = tree.find_mut::<Composer>("composer").expect("composer");
    let actions = composer.take_actions();
    assert!(matches!(actions.as_slice(), [ComposerAction::Notice(_)]));
    assert_eq!(composer.text(), "");
}

#[test]
fn clipboard_file_is_prepared_as_a_file_attachment() {
    let clipboard = Arc::new(FakeClipboard::with(ClipboardContent::Files(vec![
        ClipboardFile {
            path: std::path::PathBuf::from("notes.pdf"),
        },
    ])));
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    press(&mut tree, KeyCode::Char('v'), KeyModifiers::CONTROL);
    let actions = tree
        .find_mut::<Composer>("composer")
        .expect("composer")
        .take_actions();
    assert!(matches!(actions.as_slice(), [ComposerAction::Attach(_)]));
}
