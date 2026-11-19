//! The sessions page: attach, rename and delete from the keyboard.

use super::{Mode, SessionsAction, SessionsPage};

fn session(id: &str, title: Option<&str>) -> norte_code_wire::SessionSummary {
    norte_code_wire::SessionSummary {
        session_id: id.to_owned(),
        title: title.map(str::to_owned),
        provider: Some("openrouter".to_owned()),
        pinned: false,
        ..norte_code_wire::SessionSummary::default()
    }
}

fn page_with(sessions: Vec<norte_code_wire::SessionSummary>) -> SessionsPage {
    let mut page = SessionsPage::new();
    page.set_data(sessions, Some("active-one".to_string()));
    page
}

fn key(character: char) -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    crossterm::event::KeyEvent {
        code: KeyCode::Char(character),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    }
}

fn enter() -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    crossterm::event::KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    }
}

#[test]
fn enter_asks_to_attach_the_selected_session() {
    let mut page = page_with(vec![session("a", None), session("b", None)]);
    page.handle_listing_key(&enter());
    let actions = page.take_pending();
    assert!(
        matches!(&actions[..], [SessionsAction::Attach { session } ] if session.session_id == "a")
    );
}

#[test]
fn rename_collects_the_typed_title() {
    let mut page = page_with(vec![session("a", Some("old title"))]);
    page.handle_listing_key(&key('r'));
    for character in "fresh".chars() {
        page.type_title(&key(character));
    }
    page.type_title(&enter());
    let actions = page.take_pending();
    assert!(matches!(
        &actions[..],
        [SessionsAction::Rename { title, .. }] if title == "old titlefresh"
    ));
}

#[test]
fn delete_requires_confirmation_and_carries_the_session() {
    let mut page = page_with(vec![session("doomed", None)]);
    page.handle_listing_key(&key('d'));
    assert!(page.take_pending().is_empty());
    page.confirm_delete(&enter());
    let actions = page.take_pending();
    assert!(
        matches!(&actions[..], [SessionsAction::Delete { session } ] if session.as_str() == "doomed")
    );
}

#[test]
fn the_attached_session_cannot_be_deleted() {
    let mut page = page_with(vec![session("active-one", None)]);
    page.handle_listing_key(&key('d'));
    assert!(page.take_pending().is_empty());
    assert_eq!(page.mode, Mode::Listing);
}
