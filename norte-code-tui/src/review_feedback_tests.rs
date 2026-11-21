//! Validation of plan review notes.

use super::ReviewFeedback;

#[test]
fn feedback_is_trimmed_before_it_becomes_an_answer() {
    let feedback = ReviewFeedback::new("  add retries  ").expect("feedback");
    assert_eq!(feedback.into_answer().as_str(), "add retries");
}

#[test]
fn empty_and_oversized_feedback_are_refused() {
    assert_eq!(ReviewFeedback::new("\n"), None);
    assert_eq!(ReviewFeedback::new("x".repeat(8193)), None);
}
