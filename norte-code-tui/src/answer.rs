//! The text sent back to answer a host question.
//!
//! Both the ask-user prompt and the plan review produce this value, so an
//! empty or oversized answer cannot reach the wire.

use serde::Serialize;

#[cfg(test)]
#[path = "answer_tests.rs"]
mod tests;

/// Maximum number of characters a person can send as an answer.
const MAX_CHARS: usize = 8192;

/// A validated answer to a question the turn is parked on.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct QuestionAnswer(String);

impl QuestionAnswer {
    /// The fixed answer that approves a plan.
    pub(crate) fn approve() -> Self {
        Self("Approve".to_owned())
    }

    /// The fixed answer that rejects a plan.
    pub(crate) fn reject() -> Self {
        Self("Reject".to_owned())
    }

    /// Trim and validate typed text; `None` when it cannot be sent.
    pub(crate) fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into().trim().to_owned();
        let useful = !text.is_empty() && text.chars().count() <= MAX_CHARS;
        useful.then_some(Self(text))
    }

    /// Wrap text already validated by another answer type.
    pub(crate) fn from_validated(text: String) -> Self {
        Self(text)
    }

    /// The text as the wire API expects it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
