//! The ask-user overlay: state transitions and commit semantics.

use crate::answer::QuestionAnswer;
use crate::ui::question_prompt::{InputMode, QuestionPrompt};
use crate::ui_model::PendingQuestion;

fn answer(text: &str) -> QuestionAnswer {
    QuestionAnswer::new(text).expect("answer")
}

fn question(options: Vec<&str>, multiple: bool) -> PendingQuestion {
    PendingQuestion {
        question: "q1".to_owned(),
        prompt: "¿qué job?".to_owned(),
        options: options.into_iter().map(str::to_owned).collect(),
        multiple,
    }
}

#[test]
fn commit_with_single_option_uses_the_selected_row() {
    let mut prompt = QuestionPrompt::new();
    prompt.begin(question(vec!["sleep", "uptime"], false));
    prompt.move_down();
    prompt.commit();
    assert_eq!(
        prompt.take_pending_answer(),
        Some(("q1".to_owned(), answer("uptime")))
    );
    assert!(!prompt.is_open());
}

#[test]
fn commit_ignored_when_no_response_and_no_option() {
    let mut prompt = QuestionPrompt::new();
    prompt.begin(question(Vec::new(), false));
    prompt.commit();
    assert_eq!(prompt.take_pending_answer(), None);
    assert!(prompt.is_open());
}

#[test]
fn free_text_commits_when_options_are_absent() {
    let mut prompt = QuestionPrompt::new();
    prompt.begin(question(Vec::new(), false));
    for ch in "un sleep".chars() {
        prompt.push_answer_char(ch);
    }
    prompt.commit();
    assert_eq!(
        prompt.take_pending_answer(),
        Some(("q1".to_owned(), answer("un sleep")))
    );
}

#[test]
fn multiple_choice_commits_toggled_options_comma_joined() {
    let mut prompt = QuestionPrompt::new();
    prompt.begin(question(vec!["a", "b", "c"], true));
    prompt.toggle();
    prompt.move_down();
    prompt.toggle();
    prompt.commit();
    assert_eq!(
        prompt.take_pending_answer(),
        Some(("q1".to_owned(), answer("a, b")))
    );
}

#[test]
fn closing_resets_state_for_the_next_question() {
    let mut prompt = QuestionPrompt::new();
    prompt.begin(question(vec!["x"], false));
    prompt.move_down();
    prompt.close();
    assert!(!prompt.is_open());
    prompt.begin(question(vec!["y"], false));
    assert_eq!(prompt.take_pending_answer(), None);
}

#[test]
fn answer_mode_commits_back_to_respond() {
    let mut prompt = QuestionPrompt::new();
    prompt.begin(question(vec!["x"], false));
    prompt.mode = InputMode::Answer;
    for ch in "text".chars() {
        prompt.push_answer_char(ch);
    }
    prompt.push_answer_char('x');
    prompt.mode = InputMode::Respond;
    assert_eq!(prompt.mode, InputMode::Respond);
    assert_eq!(prompt.take_pending_answer(), None);
}
