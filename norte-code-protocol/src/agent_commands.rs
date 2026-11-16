//! Transport-neutral agent commands exposed through RAEP.
//!
//! These are the common agent operations that do not require a
//! NorteCode-specific command envelope. The enum lives here — in the
//! protocol crate both the client and the server already depend on — so
//! the client that sends them and the host that maps them share one
//! definition: a new operation is a compile error on both sides, not a
//! silent mismatch.

use serde::{Deserialize, Serialize};

use norte_code_wire::{Budget, Reasoning};

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
#[path = "agent_commands_tests.rs"]
mod tests;
