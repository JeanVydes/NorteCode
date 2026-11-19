//! The plan review reads like a file: numbered lines and clickable buttons.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

use super::PlanReviewView;
use crate::components::Component;
use crate::ui_model::UiModel;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn drawn(view: &mut PlanReviewView, model: &UiModel) -> String {
    let mut terminal = Terminal::new(TestBackend::new(90, 28)).expect("terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), model))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..28 {
        for x in 0..90 {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

fn review_with_plan() -> PlanReviewView {
    let mut view = PlanReviewView::default();
    view.begin(
        norte_code_wire::SessionKey::new("s1"),
        "q1",
        "NORTE_CODE_PLAN_REVIEW\n# The plan\nstep one\nstep two",
        &[],
        false,
    )
    .expect("review opens");
    view
}

#[test]
fn the_plan_is_rendered_as_markdown() {
    let mut view = PlanReviewView::default();
    view.begin(
        norte_code_wire::SessionKey::new("s1"),
        "q2",
        "NORTE_CODE_PLAN_REVIEW\n## Heading\n\n- first item\n- second item\n\n**bold claim**",
        &[],
        false,
    )
    .expect("review opens");
    let model = UiModel::default();

    let text = drawn(&mut view, &model);

    // The raw markdown markers are gone: headings and lists are drawn, not
    // printed as source.
    assert!(text.contains("Heading"), "drawn:\n{text}");
    assert!(text.contains("first item"), "drawn:\n{text}");
    assert!(text.contains('•'), "the list is bulleted:\n{text}");
    assert!(
        !text.contains("**bold claim**"),
        "the bold markers are drawn:\n{text}"
    );
    assert!(text.contains("bold claim"), "drawn:\n{text}");
}

#[test]
fn a_markdown_table_is_drawn_as_a_table() {
    let mut view = PlanReviewView::default();
    view.begin(
        norte_code_wire::SessionKey::new("s1"),
        "q3",
        "NORTE_CODE_PLAN_REVIEW\n| step | owner |\n|---|---|\n| build | me |",
        &[],
        false,
    )
    .expect("review opens");
    let model = UiModel::default();

    let text = drawn(&mut view, &model);

    assert!(text.contains("step"), "drawn:\n{text}");
    assert!(text.contains("owner"), "drawn:\n{text}");
    assert!(text.contains('│'), "the table has borders:\n{text}");
}

#[test]
fn the_document_carries_line_numbers() {
    let mut view = review_with_plan();
    let model = UiModel::default();

    let text = drawn(&mut view, &model);

    assert!(text.contains("plan review"), "drawn:\n{text}");
    assert!(text.contains("The plan"));
    assert!(text.contains('1'));
}

#[test]
fn the_buttons_are_named() {
    let mut view = review_with_plan();
    let model = UiModel::default();

    let text = drawn(&mut view, &model);

    assert!(text.contains("approve"));
    assert!(text.contains("reject"));
    assert!(text.contains("feedback"));
}

#[test]
fn scrolling_moves_the_document() {
    let mut view = review_with_plan();
    let model = UiModel::default();

    for _ in 0..40 {
        view.handle_key(key(KeyCode::PageDown), &model);
    }

    let text = drawn(&mut view, &model);
    assert!(text.contains("plan review"));
}

#[test]
fn the_footer_buttons_are_clickable() {
    let mut view = review_with_plan();
    let model = UiModel::default();
    let area = ratatui::layout::Rect::new(0, 0, 90, 28);
    let _ = drawn(&mut view, &model);

    let buttons = view.buttons(area);
    assert_eq!(buttons.len(), 3);
    let (rect, index) = buttons[0];
    assert_eq!(index, 0);
    assert_eq!(view.button_at(rect.x, rect.y, area), Some(0));
    let (third, index) = buttons[2];
    assert_eq!(index, 2);
    assert_eq!(view.button_at(third.x, third.y, area), Some(2));
}

#[test]
fn a_plan_approves_with_a() {
    let mut view = review_with_plan();
    let model = UiModel::default();

    view.handle_key(key(KeyCode::Char('a')), &model);

    assert!(view.take_action().is_some());
}
