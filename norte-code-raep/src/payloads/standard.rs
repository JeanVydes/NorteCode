use norte_code_wire::ServerEventKind;
use serde_json::Value;

#[allow(clippy::wildcard_imports)]
use super::*;

#[allow(clippy::too_many_lines)]
pub(crate) fn standard_event_payload(kind: &ServerEventKind) -> Option<Value> {
    match kind {
        ServerEventKind::TurnStarted { .. } => Some(payload(TurnStateChanged {
            state: TurnState::CallingProvider,
            stop_reason: None,
            usage: None,
            error: None,
        })),
        ServerEventKind::Delta { delta, .. } => {
            let (text, index, channel) = match delta {
                Delta::Text { index, text } | Delta::Refusal { index, text } => {
                    (text.clone(), *index, TokenChannel::Text)
                }
                Delta::Reasoning { index, text } => (text.clone(), *index, TokenChannel::Reasoning),
                Delta::ToolArguments {
                    index, arguments, ..
                } => (arguments.clone(), *index, TokenChannel::ToolArguments),
                Delta::Media { index, .. } => (String::new(), *index, TokenChannel::Text),
            };
            Some(payload(TokenDelta {
                delta: text,
                index: u64::from(index),
                channel,
            }))
        }
        ServerEventKind::ToolStarted {
            call_id,
            tool,
            arguments,
            source,
            ..
        } => {
            let arguments = arguments
                .as_ref()
                .and_then(|arguments| arguments.as_object())
                .cloned()
                .unwrap_or_default();
            Some(payload(ToolStarted {
                tool_call_id: call_id.clone(),
                tool_name: tool.clone(),
                arguments: Value::Object(arguments),
                source: source.clone(),
                requires_approval: false,
                executor: norte_code_wire::ToolExecutor::Server,
                attempt: 1,
            }))
        }
        ServerEventKind::ToolFinished {
            call_id,
            tool,
            is_error,
            detached,
            output,
            error,
            truncated,
            duration_ms,
            ..
        } => {
            let status = ToolStatus::from_outcome(*detached, *is_error);
            let summary = error.clone().or_else(|| output.clone()).unwrap_or_default();
            Some(payload(ToolFinished {
                tool_call_id: call_id.clone(),
                tool_name: tool.clone(),
                status,
                duration_ms: Some(*duration_ms),
                result_summary: Some(summary),
                result_truncated: *truncated,
            }))
        }
        ServerEventKind::ToolOutput {
            call_id,
            tool,
            stream,
            text,
            ..
        } => Some(payload(ToolOutput {
            tool_call_id: call_id.clone(),
            tool_name: tool.clone(),
            stream: *stream,
            delta: text.clone(),
            index: 0,
        })),
        ServerEventKind::JobStarted {
            job_id,
            tool_call_id,
            command,
            started_at,
            limits,
        } => Some(payload(JobStarted {
            job_id: job_id.clone(),
            tool_call_id: tool_call_id.clone(),
            command: command.clone(),
            detached: true,
            started_at: *started_at,
            limits: Some(limits.clone()),
        })),
        ServerEventKind::JobProgressDelta {
            stream,
            delta,
            index,
            ..
        } => Some(payload(JobProgress {
            stream: *stream,
            delta: delta.clone(),
            index: *index,
        })),
        ServerEventKind::JobFinished {
            job,
            triggered_new_turn,
            new_turn_id,
            injected_into_turn_id,
        } => {
            let new_turn_id = new_turn_id.map(|turn| turn.to_string());
            let injected_into_turn_id = injected_into_turn_id.map(|turn| turn.to_string());
            Some(crate::host::job_finished_payload(
                job,
                *triggered_new_turn,
                new_turn_id.as_deref(),
                injected_into_turn_id.as_deref(),
            ))
        }
        // RAEP `TurnStateChanged` carries only `state` and `stop_reason`;
        // `phase`/`committed` are NorteCode's own and travel in the
        // compatibility extension (see `canonical_event_payload`), never
        // smuggled into the canonical payload.
        ServerEventKind::TurnFinished { .. } => Some(payload(TurnStateChanged {
            state: TurnState::Completed,
            stop_reason: Some(StopReason::EndTurn),
            usage: None,
            error: None,
        })),
        ServerEventKind::TurnStopped { reason } => Some(payload(TurnStateChanged {
            state: TurnState::Failed,
            stop_reason: Some(StopReason::Error),
            usage: None,
            error: Some(ErrorPayload {
                code: "INTERNAL".to_owned(),
                message: format!("turn stopped: {reason}"),
                retryable: Some(false),
                details: None,
            }),
        })),
        ServerEventKind::JobList { jobs } => {
            let jobs = jobs
                .iter()
                .map(|job| {
                    let command = if job.script.is_empty() {
                        job.label.clone()
                    } else {
                        job.script.clone()
                    };
                    norte_code_wire::JobEntry {
                        job_id: job.id.clone(),
                        command,
                        status: crate::host::job_status(job),
                        exit_code: job.exit_code,
                    }
                })
                .collect::<Vec<_>>();
            Some(payload(norte_code_wire::JobList { jobs }))
        }
        ServerEventKind::MessageReplayed {
            index,
            role,
            kind,
            text,
        } => {
            let role = match role {
                norte_code_wire::MessageRole::Tool => HistorySpeaker::Tool,
                norte_code_wire::MessageRole::System
                | norte_code_wire::MessageRole::Developer
                | norte_code_wire::MessageRole::Unknown => HistorySpeaker::SystemEvent,
                norte_code_wire::MessageRole::User => HistorySpeaker::User,
                norte_code_wire::MessageRole::Assistant => HistorySpeaker::Assistant,
            };
            let kind = match kind {
                norte_code_wire::ReplayedKind::Text => match role {
                    HistorySpeaker::Tool => "tool",
                    HistorySpeaker::SystemEvent => "system_event",
                    HistorySpeaker::User => "user",
                    HistorySpeaker::Assistant => "assistant",
                },
                norte_code_wire::ReplayedKind::Reasoning => "reasoning",
                norte_code_wire::ReplayedKind::ToolCall => "tool_call",
                norte_code_wire::ReplayedKind::ToolResult => "tool_result",
                norte_code_wire::ReplayedKind::Refusal => "refusal",
            };
            Some(payload(HistoryPage {
                items: vec![HistoryItem {
                    id: index.to_string(),
                    kind: kind.to_owned(),
                    content: payload(HistoryContent {
                        role,
                        content: text,
                    }),
                    created_at: 0,
                }],
                next: None,
            }))
        }
        ServerEventKind::Error { code, message } => Some(payload(ErrorPayload {
            code: match serde_json::to_value(code)
                .and_then(|value| serde_json::from_value::<String>(value.clone()))
            {
                Ok(text) => text,
                Err(_) => "INTERNAL".to_owned(),
            },
            message: message.clone(),
            retryable: Some(false),
            details: None,
        })),
        ServerEventKind::InputAccepted { turn_id, queued } => {
            Some(payload(UserInputAcceptedPayload {
                turn_id,
                origin: OriginPayload {
                    kind: OriginKind::User,
                    user_id: None,
                    display_name: None,
                },
                content: Vec::new(),
                position: queued.then_some(1),
                replaces_turn_id: None,
            }))
        }
        ServerEventKind::SteerApplied { ids, messages } => {
            let mut content = Vec::with_capacity(messages.len());
            for text in messages {
                content.push(AcceptedPart {
                    kind: AcceptedContentKind::Text,
                    text,
                });
            }
            Some(payload(UserInputAcceptedPayload {
                turn_id: ids.first().map_or("", String::as_str),
                origin: OriginPayload {
                    kind: OriginKind::User,
                    user_id: None,
                    display_name: None,
                },
                content,
                position: None,
                replaces_turn_id: None,
            }))
        }
        // Notifications the spec asks for as events, mapped from the
        // service's own events. Output parts are emitted by the host after it
        // stores `MediaPartCompleted`; `ProviderRetry`, `QuotaWarning`,
        // `ServerDraining` and `Presence` have no service-side source yet.
        ServerEventKind::BudgetWarning {
            scope,
            unit,
            spent,
            limit,
        } => Some(payload(BudgetNotice {
            scope: (*scope).into(),
            metric: *unit,
            used: *spent,
            limit: *limit,
            action: None,
        })),
        ServerEventKind::BudgetExceeded {
            scope,
            unit,
            spent,
            limit,
            action,
        } => Some(payload(BudgetNotice {
            scope: (*scope).into(),
            metric: *unit,
            used: *spent,
            limit: *limit,
            action: Some(*action),
        })),
        ServerEventKind::RetryStarted { turn, attempt } => Some(payload(RetryStartedPayload {
            turn_id: *turn,
            attempt: *attempt,
        })),
        ServerEventKind::RetryScheduled {
            turn,
            attempt,
            delay_ms,
            reason,
        } => Some(payload(RetryScheduledPayload {
            turn_id: *turn,
            attempt: *attempt,
            delay_ms: *delay_ms,
            reason,
        })),
        ServerEventKind::CompactionFinished {
            compacted,
            summary,
            error,
            ..
        } => {
            if *compacted {
                Some(payload(CompactionFinished {
                    trigger: norte_code_wire::CompactionTrigger::Auto,
                    summary: summary.clone(),
                    error: error.clone(),
                    compacted: Some(true),
                }))
            } else {
                None
            }
        }
        ServerEventKind::SessionStats {
            input_tokens,
            output_tokens,
            cost_usd,
            requests,
            ..
        } => Some(payload(SessionStats {
            input_tokens: *input_tokens,
            output_tokens: *output_tokens,
            cost_usd: *cost_usd,
            requests: *requests,
        })),
        ServerEventKind::QuestionAsked {
            turn,
            question,
            prompt,
            options,
            multiple,
        } => Some(payload(QuestionAsked {
            turn_id: *turn,
            question: question.clone(),
            prompt: prompt.clone(),
            options: options.clone(),
            multiple: *multiple,
        })),
        ServerEventKind::QuestionAnswered { question } => Some(payload(QuestionAnswered {
            question: question.clone(),
        })),
        ServerEventKind::SessionTodos { todos } => Some(payload(SessionTodos {
            todos: todos.clone(),
        })),
        ServerEventKind::ModelChanged { provider, model }
        | ServerEventKind::ModelQueued { provider, model } => Some(payload(AgentChanged {
            from_agent: norte_code_wire::AgentRef::default(),
            to_agent: norte_code_wire::AgentRef {
                id: provider.clone(),
                version: model.clone(),
            },
        })),
        ServerEventKind::ReasoningDowngraded { from, to, reason } => {
            Some(payload(ReasoningDowngraded {
                from: from.clone(),
                to: to.clone(),
                reason: reason.clone(),
            }))
        }
        ServerEventKind::JobsStopped { stopped } => {
            Some(payload(JobsStopped { stopped: *stopped }))
        }
        ServerEventKind::ChildSessionStarted {
            child_session_id,
            agent_id,
            tool_call_id,
            lifetime,
            task_summary,
        } => Some(payload(ChildSessionStarted {
            child_session_id: child_session_id.clone(),
            agent_id: agent_id.clone(),
            tool_call_id: tool_call_id.clone(),
            lifetime: lifetime.clone(),
            task_summary: task_summary.clone(),
        })),
        ServerEventKind::ChildSessionUpdated {
            child_session_id,
            state,
            reason,
            result,
            output_schema_valid,
        } => Some(payload(ChildSessionUpdated {
            child_session_id: child_session_id.clone(),
            state: *state,
            reason: reason.clone(),
            result: result.clone(),
            output_schema_valid: *output_schema_valid,
        })),
        ServerEventKind::DelegatorStateChanged {
            child_session_id,
            delegator_suspended,
            by,
            queued_messages,
        } => Some(payload(DelegatorStateChanged {
            child_session_id: child_session_id.clone(),
            suspended: *delegator_suspended,
            by: by.clone(),
            queued_messages: *queued_messages,
        })),
        ServerEventKind::ChildAwaitingInput {
            child_session_id,
            question,
            prompt,
        } => Some(payload(ChildAwaitingInput {
            child_session_id: child_session_id.clone(),
            question: question.clone(),
            prompt: prompt.clone(),
        })),
        _ => None,
    }
}
