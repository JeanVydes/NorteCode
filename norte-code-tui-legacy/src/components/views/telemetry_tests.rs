//! The telemetry page: sessions first, their requests under them, ten a page.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use norte_code_wire::{TelemetryRecord, TelemetryReport};

use super::super::telemetry_pages::{Pages, page_count};
use super::TelemetryView;
use crate::components::Component;
use crate::ui_model::UiModel;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn record(session: &str, id: &str) -> TelemetryRecord {
    TelemetryRecord {
        id: id.to_owned(),
        session: session.to_owned(),
        ..TelemetryRecord::default()
    }
}

fn model_with(records: Vec<TelemetryRecord>) -> UiModel {
    UiModel {
        telemetry: Some(TelemetryReport {
            session: None,
            records,
            ..TelemetryReport::default()
        }),
        ..UiModel::default()
    }
}

#[test]
fn a_page_holds_ten_rows() {
    assert_eq!(page_count(0), 1);
    assert_eq!(page_count(10), 1);
    assert_eq!(page_count(11), 2);
    assert_eq!(page_count(25), 3);
}

#[test]
fn the_page_lists_only_the_rows_it_covers() {
    let pages = Pages::default();
    let items = (0..25).collect::<Vec<_>>();

    assert_eq!(pages.page_items(&items).len(), 10);
}

#[test]
fn enter_focuses_a_list_and_escape_lets_go() {
    let mut view = TelemetryView::default();
    let model = model_with(vec![record("a", "r1")]);

    view.handle_key(key(KeyCode::Enter), &model);
    assert!(view.captures_global_keys());

    view.handle_key(key(KeyCode::Esc), &model);
    assert!(!view.captures_global_keys());
}

#[test]
fn enter_twice_moves_from_the_sessions_to_the_requests() {
    let mut view = TelemetryView::default();
    let model = model_with(vec![record("a", "r1")]);

    view.handle_key(key(KeyCode::Enter), &model);
    view.handle_key(key(KeyCode::Enter), &model);

    assert!(view.captures_global_keys());
}

#[test]
fn the_arrows_walk_the_focused_list() {
    let mut view = TelemetryView::default();
    let records = (0..25)
        .map(|index| record(&format!("session-{index}"), &format!("r{index}")))
        .collect();
    let model = model_with(records);

    view.handle_key(key(KeyCode::Enter), &model);
    view.handle_key(key(KeyCode::Right), &model);

    assert_eq!(view.page_for_test(), 1);
}

#[test]
fn a_short_list_never_turns_a_page() {
    let mut view = TelemetryView::default();
    let model = model_with(vec![record("a", "r1")]);

    view.handle_key(key(KeyCode::Enter), &model);
    for _ in 0..5 {
        view.handle_key(key(KeyCode::Right), &model);
    }

    assert_eq!(view.page_for_test(), 0);
}

#[test]
fn refresh_asks_the_host() {
    let mut view = TelemetryView::default();
    let model = model_with(Vec::new());

    view.handle_key(key(KeyCode::Char('r')), &model);

    assert!(view.take_action().is_some());
}

#[test]
fn the_sessions_sit_above_the_requests() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut view = TelemetryView::default();
    let model = model_with(vec![record("alpha", "r1")]);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), &model))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..30 {
        for x in 0..100 {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }

    let sessions = text.find("sessions").expect("a sessions panel");
    let requests = text.find("requests").expect("a requests panel");
    assert!(sessions < requests, "sessions must come first:\n{text}");
    assert!(text.contains("1 sessions"));
    assert!(text.contains("page 1/1"));
}
