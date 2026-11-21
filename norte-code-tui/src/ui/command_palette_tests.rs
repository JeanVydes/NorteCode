//! The palette filters commands and reports a typed action to the host.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use terminalui::{InputEvent, Tree, node};

use super::command_palette::{CommandPalette, PaletteAction};
use crate::ui_model::Tab;

fn key(character: char) -> InputEvent {
    InputEvent::from(Event::Key(KeyEvent {
        code: KeyCode::Char(character),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    }))
}

fn enter() -> InputEvent {
    InputEvent::from(Event::Key(KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    }))
}

#[test]
fn filtering_a_command_reports_its_action_and_restores_focus() {
    let mut tree = Tree::new(node("palette", CommandPalette::new()));
    tree.find_mut::<CommandPalette>("palette")
        .expect("palette")
        .open(Some("composer"));
    assert!(tree.focus("palette"));

    for character in "Compact".chars() {
        tree.handle_event(&key(character));
    }
    tree.handle_event(&enter());

    let palette = tree.find_mut::<CommandPalette>("palette").expect("palette");
    assert!(!palette.is_open());
    assert_eq!(
        palette.take_action(),
        Some((PaletteAction::Compact, Some("composer".to_owned())))
    );
}

#[test]
fn the_first_command_switches_to_chat() {
    let mut tree = Tree::new(node("palette", CommandPalette::new()));
    tree.find_mut::<CommandPalette>("palette")
        .expect("palette")
        .open(None);
    tree.focus("palette");
    tree.handle_event(&enter());

    let action = tree
        .find_mut::<CommandPalette>("palette")
        .expect("palette")
        .take_action();
    assert_eq!(action, Some((PaletteAction::SwitchTab(Tab::Chat), None)));
}
