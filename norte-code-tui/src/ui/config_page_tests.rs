//! The preferences tab reports changes; the host applies them.

use super::{ConfigAction, ConfigPage, Setting};

fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new_with_kind(
        code,
        crossterm::event::KeyModifiers::NONE,
        crossterm::event::KeyEventKind::Press,
    )
}

#[test]
fn enter_cycles_the_selected_setting() {
    let mut page = ConfigPage::new();
    page.handle_key(&key(crossterm::event::KeyCode::Enter));
    assert!(matches!(
        &page.take_pending()[..],
        [ConfigAction::Cycle(Setting::Theme)]
    ));
}

#[test]
fn navigation_moves_to_the_diff_row() {
    let mut page = ConfigPage::new();
    for _ in 0..3 {
        page.handle_key(&key(crossterm::event::KeyCode::Down));
    }
    page.handle_key(&key(crossterm::event::KeyCode::Enter));
    assert!(matches!(
        &page.take_pending()[..],
        [ConfigAction::Cycle(Setting::DiffMode)]
    ));
}
