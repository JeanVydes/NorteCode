//! The sessions page: attach, rename and delete from the keyboard.

use crossterm::event::Event;
use ratatui::layout::Rect;
use terminalui::components::button::Button;
use terminalui::components::input::Input;
use terminalui::{InputEvent, Theme, Tree, node};

use super::{Mode, SessionsAction, SessionsPage};
use crate::ui::detail_panel::DetailPanel;

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
    page.set_data(
        sessions,
        Some("active-one".to_string()),
        crate::session_stats::SharedUsage::default(),
    );
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

fn key_input(character: char) -> InputEvent {
    InputEvent::from(Event::Key(key(character)))
}

fn enter_input() -> InputEvent {
    InputEvent::from(Event::Key(enter()))
}

#[test]
fn the_detail_panel_tracks_the_selected_session() {
    let mut page = page_with(vec![session("a", None), session("b", Some("Second"))]);
    page.handle_listing_key(&key('j'));
    page.sync_detail();
    let panel = page
        .children
        .get(1)
        .and_then(|node| node.view.as_any_ref().downcast_ref::<DetailPanel>())
        .expect("detail panel");
    assert_eq!(panel.title, "session · 2");
    assert!(
        panel
            .fields
            .iter()
            .any(|field| field.label == "title" && field.value == "Second")
    );
}

#[test]
fn new_key_asks_to_create_a_session() {
    let mut page = page_with(vec![session("a", None)]);
    page.handle_listing_key(&key('n'));
    let actions = page.take_pending();
    assert!(matches!(&actions[..], [SessionsAction::Create]));
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
    let mut tree = Tree::new(node(
        "sessions",
        page_with(vec![session("a", Some("old title"))]),
    ));
    tree.focus("sessions");
    tree.handle_event(&key_input('r'));
    assert!(tree.focus("title:input"));
    assert_eq!(tree.focused_id(), Some("title:input"));
    for character in "fresh".chars() {
        tree.handle_event(&key_input(character));
    }
    tree.handle_event(&enter_input());
    let actions = tree
        .find_mut::<SessionsPage>("sessions")
        .expect("page")
        .take_pending();
    assert!(matches!(
        &actions[..],
        [SessionsAction::Rename { title, .. }] if title == "fresh"
    ));
}

#[test]
fn rename_shows_the_original_title_in_the_dialog() {
    let mut tree = Tree::new(node(
        "sessions",
        page_with(vec![session("a", Some("original name"))]),
    ));
    tree.focus("sessions");
    tree.handle_event(&key_input('r'));

    let area = Rect::new(0, 0, 60, 20);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    tree.render(area, &mut buf, &Theme::default());
    let sheet = &buf;
    let painted: String = (0..area.height)
        .flat_map(|y| (0..area.width).map(move |x| sheet[(x, y)].symbol().to_string()))
        .collect();
    assert!(painted.contains("original name"), "{painted}");
}

#[test]
fn reopening_rename_does_not_type_the_opening_key() {
    let mut tree = Tree::new(node(
        "sessions",
        page_with(vec![session("a", Some("first"))]),
    ));
    tree.focus("sessions");
    tree.handle_event(&key_input('r'));
    tree.focus("title:input");
    tree.handle_event(&enter_input());
    tree.find_mut::<SessionsPage>("sessions")
        .expect("page")
        .take_pending();

    tree.focus("sessions");
    tree.handle_event(&key_input('r'));
    let actions = tree
        .find_mut::<SessionsPage>("sessions")
        .expect("page")
        .take_pending();
    assert!(actions.is_empty());
    let input = tree.find::<Input>("title:input").expect("input");
    assert_eq!(input.value(), "first");
}

#[test]
fn delete_requires_confirmation_and_carries_the_session() {
    let mut tree = Tree::new(node("sessions", page_with(vec![session("doomed", None)])));
    tree.focus("sessions");
    tree.handle_event(&key_input('d'));
    assert!(tree.focus("delete_confirm"));
    assert_eq!(tree.focused_id(), Some("delete_confirm"));
    assert!(
        tree.find_mut::<SessionsPage>("sessions")
            .expect("page")
            .take_pending()
            .is_empty()
    );
    tree.handle_event(&enter_input());
    let actions = tree
        .find_mut::<SessionsPage>("sessions")
        .expect("page")
        .take_pending();
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

#[test]
fn cancelling_delete_closes_the_overlay_without_deleting() {
    let mut tree = Tree::new(node("sessions", page_with(vec![session("doomed", None)])));
    tree.focus("sessions");
    tree.handle_event(&key_input('d'));
    assert_eq!(
        tree.find::<SessionsPage>("sessions").expect("page").mode,
        Mode::ConfirmDelete {
            session: norte_code_wire::SessionKey::new("doomed")
        }
    );

    if let Some(cancel) = tree.find_mut::<Button>("delete_cancel") {
        cancel.state.pressed = true;
    }
    let actions = tree
        .find_mut::<SessionsPage>("sessions")
        .expect("page")
        .take_pending();
    assert!(actions.is_empty(), "cancel must not delete the session");
    assert_eq!(
        tree.find::<SessionsPage>("sessions").expect("page").mode,
        Mode::Listing,
        "cancel must close the delete overlay"
    );
}
