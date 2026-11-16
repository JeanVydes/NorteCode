use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::SessionsView;
use crate::{
    components::{Component, ComponentAction},
    ui_model::UiModel,
};

fn drawn_by(view: &mut SessionsView, model: &UiModel, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), model))
        .expect("render");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>()
}

#[test]
fn the_list_shows_each_session_with_its_meta_line() {
    let model = UiModel {
        sessions: vec![norte_code_wire::SessionSummary {
            session_id: "session-1".to_owned(),
            title: Some("weekly sync".to_owned()),
            created_at: Some(0),
            updated_at: Some(0),
            last_activity_at: Some(0),
            turn_count: Some(12),
            ..norte_code_wire::SessionSummary::default()
        }],
        ..UiModel::default()
    };
    let mut view = SessionsView::default();

    let drawn = drawn_by(&mut view, &model, 96, 4);

    for expected in [
        "weekly sync",
        "12 turns",
        "created 1970-01-01",
        "updated 1970-01-01",
    ] {
        assert!(drawn.contains(expected), "{expected} is drawn:\n{drawn}");
    }
}

#[test]
fn a_pinned_session_carries_the_pin() {
    let model = UiModel {
        sessions: vec![norte_code_wire::SessionSummary {
            session_id: "session-1".to_owned(),
            title: Some("keep me".to_owned()),
            pinned: true,
            ..norte_code_wire::SessionSummary::default()
        }],
        ..UiModel::default()
    };
    let mut view = SessionsView::default();

    let drawn = drawn_by(&mut view, &model, 80, 4);

    assert!(drawn.contains('★'), "the pin is drawn:\n{drawn}");
}

#[test]
fn number_keys_open_the_matching_session() {
    let model = UiModel {
        sessions: (1..=3)
            .map(|index| norte_code_wire::SessionSummary {
                session_id: format!("session-{index}"),
                ..norte_code_wire::SessionSummary::default()
            })
            .collect(),
        ..UiModel::default()
    };
    let mut view = SessionsView::default();

    view.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &model,
    );
    // A number toggles the expansion of the matching session; Enter on one
    // of the action rows opens it.
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::OpenSession(session)) if session.as_str() == "session-2"
    ));
}

#[test]
fn new_session_key_requests_session_creation() {
    let mut view = SessionsView::default();

    view.handle_key(
        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
        &UiModel::default(),
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::CreateSession)
    ));
}

#[test]
fn renames_a_session_after_enter_confirms_the_title() {
    let model = UiModel {
        sessions: vec![norte_code_wire::SessionSummary {
            session_id: "session-1".to_owned(),
            title: Some("Old title".to_owned()),
            ..norte_code_wire::SessionSummary::default()
        }],
        ..UiModel::default()
    };
    let mut view = SessionsView::default();
    view.handle_key(
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(
        KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::RenameSession { session, title })
            if session.as_str() == "session-1" && title == "Old title!"
    ));
}

#[test]
fn requires_confirmation_before_deleting_a_session() {
    let model = UiModel {
        sessions: vec![norte_code_wire::SessionSummary {
            session_id: "session-1".to_owned(),
            ..norte_code_wire::SessionSummary::default()
        }],
        ..UiModel::default()
    };
    let mut view = SessionsView::default();
    view.handle_key(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        &model,
    );

    assert!(view.take_action().is_none());
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::DeleteSession(session)) if session.as_str() == "session-1"
    ));
}

#[test]
fn forks_the_selected_session() {
    let model = UiModel {
        sessions: vec![norte_code_wire::SessionSummary {
            session_id: "session-1".to_owned(),
            ..norte_code_wire::SessionSummary::default()
        }],
        ..UiModel::default()
    };
    let mut view = SessionsView::default();

    view.handle_key(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::ForkSession(session)) if session.as_str() == "session-1"
    ));
}

#[test]
fn pressing_enter_on_a_session_expands_and_collapses_it() {
    let model = UiModel {
        sessions: (1..=2)
            .map(|index| norte_code_wire::SessionSummary {
                session_id: format!("session-{index}"),
                ..norte_code_wire::SessionSummary::default()
            })
            .collect(),
        ..UiModel::default()
    };
    let mut view = SessionsView::default();
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    let drawn = drawn_by(&mut view, &model, 40, 8);
    assert!(drawn.contains("open"), "{drawn}");
    assert!(drawn.contains("rename"), "{drawn}");
    assert!(drawn.contains("delete"), "{drawn}");

    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);
    let drawn = drawn_by(&mut view, &model, 40, 8);
    assert!(!drawn.contains("rename"), "collapsed again:\n{drawn}");
}

#[test]
fn the_open_action_of_an_expanded_session_opens_it() {
    let model = UiModel {
        sessions: (1..=2)
            .map(|index| norte_code_wire::SessionSummary {
                session_id: format!("session-{index}"),
                ..norte_code_wire::SessionSummary::default()
            })
            .collect(),
        ..UiModel::default()
    };
    let mut view = SessionsView::default();
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::OpenSession(session)) if session.as_str() == "session-1"
    ));
}

#[test]
fn the_delete_action_of_an_expanded_session_opens_the_confirm_prompt() {
    let model = UiModel {
        sessions: (1..=1)
            .map(|index| norte_code_wire::SessionSummary {
                session_id: format!("session-{index}"),
                ..norte_code_wire::SessionSummary::default()
            })
            .collect(),
        ..UiModel::default()
    };
    let mut view = SessionsView::default();
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    let shown = drawn_by(&mut view, &model, 60, 6);
    assert!(shown.contains("Delete session?"), "{shown}");
    assert!(view.take_action().is_none(), "not deleted until Enter");
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);
    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::DeleteSession(session)) if session.as_str() == "session-1"
    ));
}
