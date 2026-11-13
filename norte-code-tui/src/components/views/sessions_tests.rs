use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::SessionsView;
use crate::{
    components::{Component, ComponentAction},
    ui_model::UiModel,
};

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
