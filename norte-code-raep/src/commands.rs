//! Transport-neutral agent commands exposed through RAEP.

use norte_code_protocol::{Budget, Reasoning, ReasoningScope};
use serde::Deserialize;

/// Common agent operations that do not require a NorteCode-specific command
/// envelope.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AgentCommand {
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
        /// Scope of the setting.
        scope: ReasoningScope,
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
