//! The session view: keys, prompts and the transcript's click targets.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::SessionView;
use crate::components::{Component, ComponentAction};
use crate::ui_model::UiModel;

const CLICK_LEFT: MouseEventKind = MouseEventKind::Down(MouseButton::Left);

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn transcript_model() -> UiModel {
    let mut model = UiModel::default();
    model.transcript.clear();
    model
}

fn view_with_separators() -> SessionView {
    SessionView {
        layout: super::transcript::TranscriptLayout {
            max_scroll: 0,
            width: 80,
            text_area: Rect::new(0, 0, 80, 20),
            separators: vec![
                (0, super::transcript::TurnKey::Live(1)),
                (5, super::transcript::TurnKey::Live(2)),
            ],
            thoughts: Vec::new(),
        },
        ..SessionView::default()
    }
}

fn view_with_thoughts() -> SessionView {
    SessionView {
        layout: super::transcript::TranscriptLayout {
            max_scroll: 0,
            width: 80,
            text_area: Rect::new(0, 0, 80, 20),
            separators: Vec::new(),
            thoughts: vec![(
                3,
                super::transcript::ThoughtKey {
                    turn: super::transcript::TurnKey::Live(1),
                    ordinal: 0,
                },
            )],
        },
        ..SessionView::default()
    }
}

#[test]
fn escape_in_the_composer_leaves_it() {
    let mut view = SessionView::default();
    let model = UiModel::default();

    view.handle_key(key(KeyCode::Esc), &model);

    assert!(!view.captures_global_keys());
}

#[test]
fn i_focuses_the_composer_from_the_transcript() {
    let mut view = SessionView::default();
    let model = UiModel::default();
    view.handle_key(key(KeyCode::Esc), &model);

    view.handle_key(key(KeyCode::Char('i')), &model);

    assert!(view.captures_global_keys());
}

#[test]
fn control_c_stops_a_running_turn() {
    let mut view = SessionView::default();
    let mut model = UiModel::default();
    view.handle_key(key(KeyCode::Esc), &model);
    model.turn_state = crate::ui_model::TurnState::Thinking;

    view.handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::CancelTurn)
    ));
}

#[test]
fn control_c_types_nothing_when_no_turn_is_running() {
    let mut view = SessionView::default();
    let model = UiModel::default();

    view.handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &model,
    );

    assert!(view.take_action().is_none());
    assert_eq!(view.composer.text, "");
}

#[test]
fn the_budget_approval_asks_and_declines() {
    let mut view = SessionView::default();
    let model = UiModel {
        budget_approval: Some(crate::ui_model::BudgetApprovalRequest {
            scope: norte_code_wire::BudgetScope::Session,
            unit: norte_code_wire::BudgetUnit::Usd,
            spent: 1.0,
            limit: 1.0,
        }),
        ..UiModel::default()
    };

    view.handle_key(key(KeyCode::Char('a')), &model);
    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::ApproveBudget)
    ));

    view.handle_key(key(KeyCode::Char('d')), &model);
    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::DeclineBudget)
    ));
}

#[test]
fn clicking_a_turn_separator_folds_the_turn() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_LEFT, 10, 0), &model);

    assert!(view.folded.contains(&super::transcript::TurnKey::Live(1)));
}

#[test]
fn clicking_outside_a_separator_does_nothing() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_LEFT, 10, 7), &model);

    assert_eq!(view.folded.len(), 0);
}

#[test]
fn clicking_a_thought_summary_toggles_it() {
    let mut view = view_with_thoughts();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_LEFT, 10, 3), &model);
    assert!(view.thoughts.iter().any(|fold| fold.expanded));

    view.handle_mouse(mouse(CLICK_LEFT, 10, 3), &model);
    assert!(view.thoughts.iter().all(|fold| !fold.expanded));
}

#[test]
fn clicking_outside_a_thought_does_not_toggle_it() {
    let mut view = view_with_thoughts();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_LEFT, 10, 7), &model);

    assert_eq!(view.thoughts.len(), 0);
}

#[test]
fn scrolling_moves_the_transcript() {
    let mut view = view_with_separators();
    let model = transcript_model();
    view.layout.max_scroll = 10;

    view.handle_mouse(mouse(MouseEventKind::ScrollUp, 10, 5), &model);

    assert_eq!(view.scroll_from_bottom, 3);
}

#[test]
fn g_and_shift_g_jump_to_the_ends() {
    let mut view = SessionView::default();
    let model = UiModel::default();
    view.handle_key(key(KeyCode::Esc), &model);
    view.layout.max_scroll = 12;

    view.handle_key(key(KeyCode::Char('g')), &model);
    assert_eq!(view.scroll_from_bottom, 12);

    view.handle_key(key(KeyCode::Char('G')), &model);
    assert_eq!(view.scroll_from_bottom, 0);
}

#[test]
fn a_paste_becomes_the_composer_text() {
    let mut view = SessionView::default();
    let model = UiModel::default();
    let burst = "hello there"
        .chars()
        .map(|character| key(KeyCode::Char(character)))
        .collect::<Vec<_>>();

    view.handle_keys(&burst, &model);

    assert_eq!(view.composer.text, "hello there");
}
