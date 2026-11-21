//! The free-text note a reader sends back with a rejected plan.

use crate::answer::QuestionAnswer;

#[cfg(test)]
#[path = "review_feedback_tests.rs"]
mod tests;

/// Maximum number of characters a plan note can carry.
const MAX_CHARS: usize = 8192;

/// Validated feedback about a plan under review.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewFeedback(String);

impl ReviewFeedback {
    /// Trim and validate the typed note; `None` when it cannot be sent.
    pub(crate) fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into().trim().to_owned();
        let useful = !text.is_empty() && text.chars().count() <= MAX_CHARS;
        useful.then_some(Self(text))
    }

    /// Turn the note into the wire answer the host expects.
    pub(crate) fn into_answer(self) -> QuestionAnswer {
        QuestionAnswer::from_validated(self.0)
    }
}
