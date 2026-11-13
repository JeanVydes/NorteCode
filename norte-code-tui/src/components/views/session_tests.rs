use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use norte_code_wire::{JobInfo, JobState, SessionKey, SessionSummary};

use super::{SessionView, active_session_label, session_regions};
use crate::{
    components::{Component, ComponentAction, ViewAction},
    ui_model::{ReasoningLevel, SessionAttachmentState, TurnPoint, UiModel},
};
use norte_code_wire::{BudgetAction, BudgetScope, BudgetUnit};

#[test]
fn session_title_is_shown_instead_of_its_identifier() {
    let model = UiModel {
        active_session: Some(SessionKey::new("session-1")),
        sessions: vec![SessionSummary {
            session_id: "session-1".to_owned(),
            title: Some("Debugging".to_owned()),
            ..SessionSummary::default()
        }],
        ..UiModel::default()
    };

    assert_eq!(active_session_label(&model), Some("Debugging"));
}

#[test]
fn untitled_session_uses_the_sidebar_fallback_label() {
    let model = UiModel {
        active_session: Some(SessionKey::new("session-1")),
        sessions: vec![SessionSummary {
            session_id: "session-1".to_owned(),
            ..SessionSummary::default()
        }],
        ..UiModel::default()
    };

    assert_eq!(active_session_label(&model), Some("Untitled session"));
}

#[test]
fn session_content_keeps_the_composer_below_the_transcript() {
    for height in 3..=6 {
        let [transcript, picker, jobs, todos, composer] =
            session_regions(ratatui::layout::Rect::new(0, 0, 80, height), 0, 0, 0, 2);
        assert!(transcript.y + transcript.height <= picker.y);
        assert!(picker.y + picker.height <= jobs.y);
        assert!(jobs.y + jobs.height <= todos.y);
        assert!(todos.y + todos.height <= composer.y);
        assert!(composer.y + composer.height <= height);
    }
}

#[test]
fn the_panels_sit_between_the_transcript_and_the_composer() {
    let [transcript, picker, jobs, todos, composer] =
        session_regions(ratatui::layout::Rect::new(0, 0, 80, 14), 2, 3, 4, 2);

    assert!(transcript.y + transcript.height <= picker.y);
    assert_eq!(picker.height, 2);
    assert_eq!(jobs.height, 3);
    assert_eq!(todos.height, 4);
    assert!(todos.y + todos.height <= composer.y);
}

#[test]
fn budget_shortcut_submits_a_positive_usd_limit() {
    let mut view = SessionView::default();
    let model = UiModel {
        active_session: Some(SessionKey::new("session-1")),
        budget_action: BudgetAction::Stop,
        ..UiModel::default()
    };
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    for code in [
        KeyCode::Char('b'),
        KeyCode::Char('1'),
        KeyCode::Char('.'),
        KeyCode::Char('5'),
        KeyCode::Enter,
    ] {
        view.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &model);
    }

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::SetBudget {
            limit,
            action: BudgetAction::Stop
        }) if (limit - 1.5).abs() < f64::EPSILON
    ));
}

#[test]
fn ask_budget_prompts_for_a_one_turn_approval() {
    let mut view = SessionView::default();
    let model = UiModel {
        budget_approval: Some(crate::ui_model::BudgetApprovalRequest {
            scope: BudgetScope::Session,
            unit: BudgetUnit::Usd,
            spent: 1.5,
            limit: 1.0,
        }),
        ..UiModel::default()
    };

    view.handle_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::ApproveBudget)
    ));
}

#[test]
fn composer_is_focused_by_default_and_sends_when_attached() {
    let mut view = SessionView::default();
    let model = UiModel {
        active_session: Some(SessionKey::new("session-1")),
        attachment_state: SessionAttachmentState::Attached,
        ..UiModel::default()
    };
    for code in [KeyCode::Char('h'), KeyCode::Char('i'), KeyCode::Enter] {
        view.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &model);
    }

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::SendMessage { text, chips }) if text == "hi" && chips.is_empty()
    ));
}

#[test]
fn composer_keeps_the_draft_until_the_session_is_attached() {
    let mut view = SessionView::default();
    let model = UiModel {
        active_session: Some(SessionKey::new("session-1")),
        attachment_state: SessionAttachmentState::Connecting,
        ..UiModel::default()
    };
    for code in [KeyCode::Char('h'), KeyCode::Char('i'), KeyCode::Enter] {
        view.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &model);
    }

    assert!(view.take_action().is_none());
    assert_eq!(view.composer.text, "hi");
}

#[test]
fn export_shortcut_requests_the_active_session_bundle() {
    let mut view = SessionView::default();
    let model = UiModel {
        active_session: Some(SessionKey::new("session-1")),
        ..UiModel::default()
    };
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::View(ViewAction::ExportSession))
    ));
}

#[test]
fn cancel_key_stops_the_running_turn() {
    let mut view = SessionView::default();
    let model = UiModel::default();
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('C'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::CancelTurn)
    ));
}

#[test]
fn control_c_stops_the_running_turn_from_the_composer() {
    let mut view = SessionView::default();
    let model = UiModel {
        turn_state: crate::ui_model::TurnState::Streaming,
        ..UiModel::default()
    };
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
fn reasoning_key_cycles_the_effort() {
    let mut view = SessionView::default();
    let model = UiModel::default();
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::SetReasoning(ReasoningLevel::Disabled))
    ));
}

#[test]
fn rewind_picks_the_turn_the_reader_names() {
    let mut view = SessionView::default();
    let model = UiModel {
        turn_points: vec![
            TurnPoint {
                turn_id: "turn-1".to_owned(),
                preview: "first".to_owned(),
            },
            TurnPoint {
                turn_id: "turn-2".to_owned(),
                preview: "second".to_owned(),
            },
        ],
        ..UiModel::default()
    };
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(
        KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::Rewind { turn_id }) if turn_id == "turn-2"
    ));
}

#[test]
fn pin_marks_the_turn_the_reader_names() {
    let mut view = SessionView::default();
    let model = UiModel {
        turn_points: vec![TurnPoint {
            turn_id: "turn-1".to_owned(),
            preview: "first".to_owned(),
        }],
        ..UiModel::default()
    };
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('P'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(
        KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::PinTurn { turn_id, pinned: true }) if turn_id == "turn-1"
    ));
}

#[test]
fn pin_unmarks_a_turn_that_is_already_pinned() {
    let mut view = SessionView::default();
    let model = UiModel {
        turn_points: vec![TurnPoint {
            turn_id: "turn-1".to_owned(),
            preview: "first".to_owned(),
        }],
        pinned_turns: vec!["turn-1".to_owned()],
        ..UiModel::default()
    };
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('P'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(
        KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::PinTurn { pinned: false, .. })
    ));
}

#[test]
fn the_turn_picker_closes_without_acting() {
    let mut view = SessionView::default();
    let model = UiModel {
        turn_points: vec![TurnPoint {
            turn_id: "turn-1".to_owned(),
            preview: "first".to_owned(),
        }],
        ..UiModel::default()
    };
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);

    assert!(view.take_action().is_none());
}

fn running_job(id: &str, label: &str) -> JobInfo {
    JobInfo {
        id: id.to_owned(),
        label: label.to_owned(),
        script: label.to_owned(),
        state: JobState::Running,
        exit_code: None,
        timed_out: false,
        elapsed_ms: 0,
        output: String::new(),
        dropped: false,
        output_bytes_total: 0,
    }
}

#[test]
fn the_job_picker_stops_the_job_the_reader_names() {
    let mut view = SessionView::default();
    let model = UiModel {
        jobs: vec![
            running_job("job-1", "cargo build"),
            running_job("job-2", "cargo test"),
        ],
        ..UiModel::default()
    };
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::StopJob { job_id }) if job_id == "job-2"
    ));
}

#[test]
fn the_job_picker_ignores_a_number_that_names_no_job() {
    let mut view = SessionView::default();
    let model = UiModel {
        jobs: vec![running_job("job-1", "cargo build")],
        ..UiModel::default()
    };
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(
        KeyEvent::new(KeyCode::Char('9'), KeyModifiers::NONE),
        &model,
    );

    assert!(view.take_action().is_none());
}
