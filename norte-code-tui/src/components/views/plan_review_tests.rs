use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{PLAN_REVIEW_PREFIX, PlanReviewView};
use crate::{components::Component, ui_model::UiModel};

#[test]
fn opens_ordinary_questions_with_options() {
    let mut review = PlanReviewView::default();

    review
        .begin(
            norte_code_wire::SessionKey::new("session"),
            "question",
            "normal question",
            &["yes".to_owned(), "no".to_owned()],
            false,
        )
        .expect("lifecycle");
    assert!(review.is_open());
    review.handle_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &UiModel::default(),
    );
    review.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &UiModel::default(),
    );
    assert!(matches!(
        review.take_action(),
        Some(crate::components::ComponentAction::AnswerQuestion { answer, .. })
            if answer == "no"
    ));
}

#[test]
fn joins_selected_options_for_multiple_choice_questions() {
    let mut review = PlanReviewView::default();
    review
        .begin(
            norte_code_wire::SessionKey::new("session"),
            "question",
            "Choose tools",
            &["read".to_owned(), "grep".to_owned(), "write".to_owned()],
            true,
        )
        .expect("lifecycle");
    let model = UiModel::default();
    review.handle_key(
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        &model,
    );
    review.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    review.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model);
    review.handle_key(
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        &model,
    );
    review.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        review.take_action(),
        Some(crate::components::ComponentAction::AnswerQuestion { answer, .. })
            if answer == "read, write"
    ));
}

#[test]
fn sends_free_form_feedback_to_the_plan_question() {
    let mut review = PlanReviewView::default();
    review
        .begin(
            norte_code_wire::SessionKey::new("session"),
            "question",
            &format!("{PLAN_REVIEW_PREFIX}Plan body"),
            &[],
            false,
        )
        .expect("lifecycle");
    let model = UiModel::default();
    review.handle_key(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        &model,
    );
    review.handle_key(
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        &model,
    );
    review.handle_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        &model,
    );
    review.handle_key(
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        &model,
    );
    review.handle_key(
        KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
        &model,
    );
    review.handle_key(
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        &model,
    );
    review.handle_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        &model,
    );
    review.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &model);

    assert!(matches!(
        review.take_action(),
        Some(crate::components::ComponentAction::AnswerQuestion {
            session,
            question,
            answer,
        }) if session.as_str() == "session" && question == "question" && answer == "revise"
    ));
}

#[test]
fn rejects_a_plan_with_the_review_choice() {
    let mut review = PlanReviewView::default();
    review
        .begin(
            norte_code_wire::SessionKey::new("session"),
            "question",
            &format!("{PLAN_REVIEW_PREFIX}Plan body"),
            &[],
            false,
        )
        .expect("lifecycle");
    review.handle_key(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        &UiModel::default(),
    );

    assert!(matches!(
        review.take_action(),
        Some(crate::components::ComponentAction::AnswerQuestion { answer, .. })
            if answer == "Reject"
    ));
}
