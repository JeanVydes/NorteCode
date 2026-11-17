use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use norte_code_wire::{SessionKey, SessionSummary};

use super::{SessionView, active_session_label, session_regions};
use crate::{
    components::{Component, ComponentAction, ViewAction},
    ui_model::{SessionAttachmentState, UiModel},
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
        let [transcript, todos, composer] =
            session_regions(ratatui::layout::Rect::new(0, 0, 80, height), 0, 2);
        assert!(transcript.y + transcript.height <= todos.y);
        assert!(todos.y + todos.height <= composer.y);
        assert!(composer.y + composer.height <= height);
    }
}

#[test]
fn the_todos_sit_between_the_transcript_and_the_composer() {
    let [transcript, todos, composer] =
        session_regions(ratatui::layout::Rect::new(0, 0, 80, 14), 4, 2);

    assert!(transcript.y + transcript.height <= todos.y);
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
fn the_reasoning_key_opens_the_effort_menu() {
    let mut view = SessionView::default();
    let model = UiModel::default();
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::OpenHeaderModal(
            crate::components::HeaderModal::Reasoning
        ))
    ));
}

#[test]
fn the_queue_key_opens_the_queue_menu() {
    let mut view = SessionView::default();
    let model = UiModel::default();
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
        &model,
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::OpenHeaderModal(
            crate::components::HeaderModal::Queue
        ))
    ));
}

#[test]
fn right_clicking_a_turn_and_choosing_rewind_lands_on_it() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 3, 0), &model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::Rewind { turn_id }) if turn_id == "turn_1"
    ));
}
#[test]
fn the_context_menu_pins_the_turn_the_reader_clicked() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 3, 0), &model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::PinTurn { turn_id, pinned: true }) if turn_id == "turn_1"
    ));
}
#[test]
fn the_context_menu_unpins_a_turn_that_is_already_pinned() {
    let mut view = view_with_separators();
    let model = UiModel {
        pinned_turns: vec!["turn_1".to_owned()],
        ..transcript_model()
    };

    view.handle_mouse(mouse(CLICK_RIGHT, 3, 0), &model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::PinTurn { pinned: false, .. })
    ));
}
#[test]
fn escape_closes_the_turn_menu_without_acting() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 3, 0), &model);
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);

    assert!(view.take_action().is_none());
}

const CLICK_LEFT: MouseEventKind = MouseEventKind::Down(MouseButton::Left);
const CLICK_RIGHT: MouseEventKind = MouseEventKind::Down(MouseButton::Right);

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn view_with_separators() -> SessionView {
    SessionView {
        last_render_area: ratatui::layout::Rect::new(0, 0, 80, 20),
        layout: super::transcript::TranscriptLayout {
            max_scroll: 0,
            width: 80,
            text_area: ratatui::layout::Rect::new(0, 0, 80, 20),
            separators: vec![
                (0, super::transcript::TurnKey::Live(1)),
                (5, super::transcript::TurnKey::Live(2)),
            ],
            thoughts: Vec::new(),
        },
        ..SessionView::default()
    }
}

fn transcript_model() -> crate::ui_model::UiModel {
    let items = vec![
        crate::ui_model::TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "hola".to_owned(),
            chips: Vec::new(),
        },
        crate::ui_model::TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: norte_code_wire::ServerEventKind::Delta {
                turn: 1,
                delta: norte_code_wire::Delta::Text {
                    index: 0,
                    text: "buenos dias".to_owned(),
                },
            },
        })),
        crate::ui_model::TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "segunda".to_owned(),
            chips: Vec::new(),
        },
        crate::ui_model::TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: norte_code_wire::ServerEventKind::Delta {
                turn: 2,
                delta: norte_code_wire::Delta::Text {
                    index: 0,
                    text: "otra vez".to_owned(),
                },
            },
        })),
    ];
    UiModel {
        transcript: items,
        ..UiModel::default()
    }
}

#[test]
fn z_folds_the_turn_under_the_reader() {
    let mut view = SessionView {
        layout: super::transcript::TranscriptLayout {
            max_scroll: 2,
            width: 80,
            text_area: ratatui::layout::Rect::new(0, 0, 80, 20),
            separators: Vec::new(),
            thoughts: Vec::new(),
        },
        ..SessionView::default()
    };
    let model = transcript_model();

    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
        &model,
    );

    assert!(view.folded.contains(&super::transcript::TurnKey::Live(1)));
}

#[test]
fn jumping_into_a_folded_turn_unfolds_it() {
    let mut view = SessionView {
        layout: super::transcript::TranscriptLayout {
            max_scroll: 2,
            width: 80,
            text_area: ratatui::layout::Rect::new(0, 0, 80, 20),
            separators: Vec::new(),
            thoughts: Vec::new(),
        },
        ..SessionView::default()
    };
    let model = transcript_model();

    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);
    view.handle_key(
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(
        KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE),
        &model,
    );
    view.handle_key(
        KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE),
        &model,
    );

    assert!(!view.folded.contains(&super::transcript::TurnKey::Live(1)));
}

#[test]
fn clicking_a_turn_separator_folds_the_turn() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_LEFT, 10, 0), &model);

    assert!(view.folded.contains(&super::transcript::TurnKey::Live(1)));
}

fn view_with_thoughts() -> SessionView {
    SessionView {
        last_render_area: ratatui::layout::Rect::new(0, 0, 80, 20),
        layout: super::transcript::TranscriptLayout {
            max_scroll: 0,
            width: 80,
            text_area: ratatui::layout::Rect::new(0, 0, 80, 20),
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
fn clicking_outside_a_separator_does_nothing() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_LEFT, 10, 7), &model);

    assert_eq!(view.folded.len(), 0);
}

#[test]
fn a_right_click_on_a_separator_opens_the_menu() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 10, 0), &model);

    assert!(view.context_menu.is_some());
}

#[test]
fn a_right_click_elsewhere_does_not_open_the_menu() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 10, 7), &model);

    assert!(view.context_menu.is_none());
}

#[test]
fn escape_closes_the_menu_without_an_action() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 10, 0), &model);
    view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &model);

    assert!(view.context_menu.is_none());
    assert!(view.take_action().is_none());
}

#[test]
fn the_menu_rewinds_to_the_clicked_turn() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 10, 0), &model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::Rewind { turn_id }) if turn_id == "turn_1"
    ));
}

#[test]
fn the_menu_pins_and_unpins_the_turn() {
    let model = UiModel {
        turn_points: Vec::new(),
        ..UiModel::default()
    };
    let mut view = view_with_separators();
    view.handle_mouse(mouse(CLICK_RIGHT, 10, 0), &model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::PinTurn { turn_id, pinned: true }) if turn_id == "turn_1"
    ));
}

#[test]
fn the_menu_folds_the_turn() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 10, 0), &model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(view.folded.contains(&super::transcript::TurnKey::Live(1)));
    assert!(view.take_action().is_none());
}

#[test]
fn clicking_a_menu_item_executes_it() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 4, 0), &model);
    view.handle_mouse(mouse(CLICK_LEFT, 24, 9), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::Rewind { turn_id }) if turn_id == "turn_1"
    ));
}

#[test]
fn a_click_on_the_menu_keeps_it_open() {
    let mut view = view_with_separators();
    let model = transcript_model();

    view.handle_mouse(mouse(CLICK_RIGHT, 4, 0), &model);
    view.handle_mouse(mouse(CLICK_LEFT, 24, 8), &model);

    assert!(view.context_menu.is_some());
}

#[test]
fn clicking_the_drawn_separator_folds_the_turn() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut view = SessionView::default();
    let model = transcript_model();
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), &model))
        .expect("render");
    let buffer = terminal.backend().buffer();
    let row = (0..20).find(|y| {
        (0..80)
            .map(|x| buffer[(x, *y)].symbol())
            .collect::<String>()
            .contains("turn 1")
    });

    view.handle_mouse(
        // px-4: the transcript starts at column 4.
        mouse(CLICK_LEFT, 6, row.expect("the separator is drawn")),
        &model,
    );

    assert!(view.folded.contains(&super::transcript::TurnKey::Live(1)));
}

#[test]
fn clicking_the_drawn_thought_summary_toggles_the_thought() {
    use ratatui::{Terminal, backend::TestBackend};

    let items = vec![
        crate::ui_model::TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "hola".to_owned(),
            chips: Vec::new(),
        },
        crate::ui_model::TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: norte_code_wire::ServerEventKind::Delta {
                turn: 1,
                delta: norte_code_wire::Delta::Reasoning {
                    index: 0,
                    text: "**quick thought**".to_owned(),
                },
            },
        })),
    ];
    let model = UiModel {
        transcript: items,
        ..UiModel::default()
    };
    let mut view = SessionView::default();
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), &model))
        .expect("render");
    let buffer = terminal.backend().buffer();
    let row = (0..20).find(|y| {
        (0..80)
            .map(|x| buffer[(x, *y)].symbol())
            .collect::<String>()
            .contains("Thought")
    });

    view.handle_mouse(
        // px-4: the transcript starts at column 4.
        mouse(CLICK_LEFT, 6, row.expect("the thought is drawn")),
        &model,
    );

    assert!(view.thoughts.iter().any(|fold| fold.expanded));
}
