//! Sent-message history: recall, stash and restore.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::{Tree, node};

use super::{Composer, History};
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
    let mut tree = Tree::new(node("composer", Composer::new(&dark)));
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
