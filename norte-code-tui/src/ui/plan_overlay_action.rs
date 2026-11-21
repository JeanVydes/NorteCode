//! The decision a reader can make on a live plan review.

use crate::review_feedback::ReviewFeedback;

/// The prefix the agent puts on a plan review question.
pub(crate) const PLAN_REVIEW_PREFIX: &str = "NORTE_CODE_PLAN_REVIEW\n";

/// What the reader chose while a live review was open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PlanReviewAction {
    /// Activate the plan.
    Accept,
    /// Cancel the plan.
    Decline,
    /// Send feedback back to the model.
    Feedback(ReviewFeedback),
}
