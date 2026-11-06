//! Transport-neutral agent commands exposed through RAEP.
//!
//! These are the common agent operations that do not require a
//! NorteCode-specific command envelope. The enum lives here — in the
//! protocol crate both the client and the server already depend on — so
//! the client that sends them and the host that maps them share one
//! definition: a new operation is a compile error on both sides, not a
//! silent mismatch.

use serde::{Deserialize, Serialize};

use crate::{Budget, Reasoning};

/// What a new input does when the session already has a turn running.
///
/// RAEP's `USER_INPUT.queue_policy` is the wire name for this: the client
/// asks what happens to a message typed while the model is answering, and
/// the service answers instead of guessing. RAEP calls it `QueuePolicy`
/// and spells the values `reject`, `queue` and `interrupt`; `NorteCode` owns
/// its own enum for its own config so the crate does not gain a second
/// RAEP dependency just to name a CLI setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueMode {
    /// Refuse the input while a turn is active (RAEP's default).
    #[default]
    Reject,
    /// Enqueue the input for the turn already in flight.
    Queue,
    /// Cancel the running turn, then enqueue the input.
    Interrupt,
}

/// Common agent operations that do not require a NorteCode-specific command
/// envelope.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentCommand {
    /// List background jobs.
    ListJobs,
    /// Stop one job or all jobs.
    StopJobs {
        /// Optional job identifier.
        id: Option<String>,
    },
    /// Change the model at the next turn boundary.
    SetModel {
        /// Provider name.
        provider: String,
        /// Model identifier.
        model: String,
    },
    /// Change reasoning configuration.
    SetReasoning {
        /// Requested reasoning.
        reasoning: Reasoning,
    },
    /// Set the session budget.
    SetBudget {
        /// New budget.
        budget: Budget,
    },
    /// Answer a pending question.
    AnswerQuestion {
        /// Question identifier.
        question: String,
        /// Human answer.
        answer: String,
    },
    /// Resume the session.
    Resume,
    /// Compact the session.
    Compact,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_closed_operation_names() {
        let command: AgentCommand =
            serde_json::from_str(r#"{"operation":"list_jobs"}"#).expect("command");
        assert!(matches!(command, AgentCommand::ListJobs));
    }

    #[test]
    fn rejects_unknown_operations() {
        let result = serde_json::from_str::<AgentCommand>(r#"{"operation":"delete_everything"}"#);
        assert!(result.is_err());
    }
}
