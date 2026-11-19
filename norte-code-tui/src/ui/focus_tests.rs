//! Focus wiring: the composer must hold the focus slot at startup, and the
//! Input inside it must see itself as focused when the tree dispatches a key.

use terminalui::InputEvent;

use crate::ui::build_tree;
use crate::ui_model::UiModel;

#[test]
fn composer_holds_the_focus_slot_and_accepts_input() {
    let model = UiModel::detached();
    let mut tree = build_tree(&model);
    eprintln!("focusable order: {:?}", tree.focusable_ids());
    assert_eq!(tree.focused_id(), Some("composer"));

    let before = tree.to_json().expect("json");
    let input = InputEvent::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('h'),
        crossterm::event::KeyModifiers::NONE,
    ));
    let _ = tree.handle_event(&input);
    let after = tree.to_json().expect("json");
    eprintln!("changed: {}", before != after);
    assert_ne!(before, after, "the composer must record the typed key");
}
