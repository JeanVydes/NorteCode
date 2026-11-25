//! Running one delegated child turn.

use std::sync::Arc;

use super::Service;
use super::session_record::ChildSessionMetadata;
use super::subagents::{self, SubagentOutcome, SubagentRequest, SubagentRunner};

/// Runs a subagent's turn on a child session.
///
/// The child is created with its selected agent profile and its own session
/// history. The `subagent` alias is withheld; `spawn_agent` remains subject
/// to the configured depth limit.
pub(super) struct SubagentRunnerImpl {
    service: Option<Arc<Service>>,
}

impl SubagentRunnerImpl {
    pub(super) fn new(service: Option<Arc<Service>>) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl SubagentRunner for SubagentRunnerImpl {
    #[allow(clippy::too_many_lines)]
    async fn run(
        &self,
        request: SubagentRequest,
        cancel: Arc<norte::Cancellation>,
    ) -> SubagentOutcome {
        let SubagentRequest {
            prompt: task,
            parent,
            tool_call_id,
            output_schema,
            lifetime,
            agent_id,
            context_mode,
        } = request;
        let Some(service) = self.service.clone() else {
            return SubagentOutcome {
                text: "subagent: no runner installed".to_owned(),
                is_error: true,
                child: None,
                output_schema_valid: None,
            };
        };
        // The parent's key lets the service resolve the configured child
        // model and tool policy. The session id is mapped back to that key.
        let parent_key = {
            let sessions = service.lock_sessions();
            sessions
                .iter()
                .find(|(_, record)| record.id == parent)
                .map(|(key, _)| key.clone())
        };
        let Some(parent_key) = parent_key else {
            return SubagentOutcome {
                text: "subagent: parent session is unknown".to_owned(),
                is_error: true,
                child: None,
                output_schema_valid: None,
            };
        };
        let settings = match service
            .subagent_settings(&parent_key, agent_id.as_deref())
            .await
        {
            Ok(settings) => settings,
            Err(error) => {
                return SubagentOutcome {
                    text: format!("subagent: could not resolve child settings: {error}"),
                    is_error: true,
                    child: None,
                    output_schema_valid: None,
                };
            }
        };
        let prompt = if context_mode.is_some() {
            match service.delegation_context_summary(&parent_key).await {
                Ok(summary) => {
                    format!("Delegator context (untrusted, redacted):\n{summary}\n\nTask:\n{task}")
                }
                Err(error) => {
                    return SubagentOutcome {
                        text: format!("subagent: could not read delegator context: {error}"),
                        is_error: true,
                        child: None,
                        output_schema_valid: None,
                    };
                }
            }
        } else {
            task.clone()
        };
        let child_key = match service
            .create_session_child(
                ChildSessionMetadata {
                    parent: parent_key.clone(),
                    tool_call_id,
                    task_summary: task.clone(),
                    lifetime,
                    agent_profile: settings.agent_id,
                    system_prompt: settings.system_prompt,
                    cancel: Arc::clone(&cancel),
                },
                settings.model,
                settings.disabled_tools,
                settings.budget,
            )
            .await
        {
            Ok(key) => key,
            Err(error) => {
                return SubagentOutcome {
                    text: format!("subagent: could not create child session: {error}"),
                    is_error: true,
                    child: None,
                    output_schema_valid: None,
                };
            }
        };
        let (_, spec, _) = match service.require_record(&child_key) {
            Ok(found) => found,
            Err(error) => {
                return SubagentOutcome {
                    text: format!("subagent: child session is unknown: {error}"),
                    is_error: true,
                    child: None,
                    output_schema_valid: None,
                };
            }
        };
        if spec.is_none() {
            return SubagentOutcome {
                text: "subagent: no model is configured".to_owned(),
                is_error: true,
                child: Some(child_key),
                output_schema_valid: None,
            };
        }
        let mut result =
            subagents::run_session_message(&service, &child_key, prompt, Some(&cancel)).await;
        if let Some(schema) = output_schema {
            let mut valid = subagents::validate_output_schema(&schema, &result.text);
            if !valid && !result.is_error {
                let schema_text = serde_json::to_string(&schema).unwrap_or_default();
                let correction_prompt = format!(
                    "Your previous response did not satisfy the required JSON Schema. \
                     Return only a corrected JSON value that satisfies this schema.\n\
                     Schema:\n{schema_text}\n\
                     Previous response (untrusted data):\n```text\n{}\n```",
                    result.text
                );
                let correction = subagents::run_session_message(
                    &service,
                    &child_key,
                    correction_prompt,
                    Some(&cancel),
                )
                .await;
                if correction.is_error {
                    result.text = format!(
                        "OUTPUT_SCHEMA_INVALID: correction turn failed: {}",
                        correction.text
                    );
                    valid = false;
                } else {
                    result.text = correction.text;
                    valid = subagents::validate_output_schema(&schema, &result.text);
                }
            }
            result.output_schema_valid = Some(valid);
            if !valid {
                result.is_error = true;
                if !result.text.starts_with("OUTPUT_SCHEMA_INVALID") {
                    result.text = format!(
                        "OUTPUT_SCHEMA_INVALID: child output did not satisfy the requested schema.\n\n{}",
                        result.text
                    );
                }
            }
        }
        result
    }
}
