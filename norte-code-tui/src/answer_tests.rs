//! Validation of typed answers.

use super::QuestionAnswer;

#[test]
fn typed_answers_are_trimmed_and_kept() {
    let answer = QuestionAnswer::new("  looks good  ").expect("answer");
    assert_eq!(answer.as_str(), "looks good");
}

#[test]
fn empty_and_oversized_answers_are_refused() {
    assert_eq!(QuestionAnswer::new("   "), None);
    assert_eq!(QuestionAnswer::new("x".repeat(8193)), None);
}

#[test]
fn plan_decisions_are_always_valid_answers() {
    assert_eq!(QuestionAnswer::approve().as_str(), "Approve");
    assert_eq!(QuestionAnswer::reject().as_str(), "Reject");
}
