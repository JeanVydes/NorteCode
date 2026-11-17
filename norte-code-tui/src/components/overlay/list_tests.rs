//! The modal list: filtering, selection and the scroll that follows it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{ListOutcome, ModalList};
use crate::components::MenuEntry;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn list(labels: &[&str]) -> ModalList {
    ModalList::new(
        labels
            .iter()
            .map(|label| MenuEntry::new(*label, None))
            .collect(),
    )
}

#[test]
fn typing_filters_the_rows() {
    let mut list = list(&["New session", "Theme", "Quit"]);

    for character in "the".chars() {
        list.handle_key(key(KeyCode::Char(character)));
    }

    assert_eq!(list.visible_labels(), vec!["Theme".to_owned()]);
}

#[test]
fn backspace_widens_the_filter_again() {
    let mut list = list(&["Theme", "Quit"]);
    list.handle_key(key(KeyCode::Char('q')));

    list.handle_key(key(KeyCode::Backspace));

    assert_eq!(list.visible_labels().len(), 2);
}

#[test]
fn escape_reports_closed_and_the_menu_drops_the_filter_first() {
    let mut list = list(&["Theme", "Quit"]);
    list.handle_key(key(KeyCode::Char('t')));

    assert_eq!(list.handle_key(key(KeyCode::Esc)), ListOutcome::Closed);
    assert!(list.clear_filter());
    assert_eq!(list.handle_key(key(KeyCode::Esc)), ListOutcome::Closed);
    assert!(!list.clear_filter());
}

#[test]
fn the_selection_stops_at_the_last_row() {
    let mut list = list(&["a", "b"]);
    for _ in 0..10 {
        list.handle_key(key(KeyCode::Char('j')));
    }

    assert_eq!(list.selected_index(), Some(1));
}

#[test]
fn enter_reports_the_index_in_the_whole_list() {
    let mut list = list(&["Alpha", "Beta", "Gamma"]);
    list.handle_key(key(KeyCode::Char('m')));
    list.handle_key(key(KeyCode::Char('a')));

    assert_eq!(
        list.handle_key(key(KeyCode::Enter)),
        ListOutcome::Submitted(2)
    );
}
