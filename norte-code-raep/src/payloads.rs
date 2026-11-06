//! Canonical RAEP payloads emitted by the `NorteCode` adapter.

use norte_code_protocol::{Delta, ServerEventKind};
use serde::Serialize;
use serde_json::{Value, json};

/// Converts a `NorteCode` event into the stable payload understood by RAEP
/// clients.
///
/// The complete `NorteCode` event remains available in the envelope extension.
/// That keeps the adapter lossless while preventing clients from depending on
/// `NorteCode`'s request/session wrapper for the common event families.
#[allow(clippy::too_many_lines)]
pub(crate) fn canonical_event_payload(kind: &ServerEventKind) -> Value {
    let payload = match kind {
        ServerEventKind::SessionCreated { title } => CanonicalEvent::SessionCreated { title },
        ServerEventKind::SessionList { sessions } => CanonicalEvent::SessionList {
            sessions: serde_json::to_value(sessions).expect("session list is serializable"),
        },
        ServerEventKind::SessionDeleted => CanonicalEvent::SessionDeleted,
        ServerEventKind::SessionRenamed { title } => CanonicalEvent::SessionRenamed { title },
        ServerEventKind::SessionForked { from } => CanonicalEvent::SessionForked {
            from: from.as_str(),
        },
        ServerEventKind::SessionSwitched => CanonicalEvent::SessionSwitched,
        ServerEventKind::TurnStarted { turn } => CanonicalEvent::TurnStarted { turn: *turn },
        ServerEventKind::Delta { turn, delta } => CanonicalEvent::TokenDelta {
            turn: *turn,
            delta: serde_json::to_value(delta).expect("protocol delta is serializable"),
        },
        ServerEventKind::ToolStarted {
            turn,
            call_id,
            tool,
            arguments,
            source,
        } => CanonicalEvent::ToolCallStarted {
            turn: *turn,
            call_id,
            tool,
            arguments,
            source,
        },
        ServerEventKind::ToolOutput {
            turn,
            call_id,
            tool,
            stream,
            text,
            last,
        } => CanonicalEvent::JobProgressDelta {
            turn: *turn,
            call_id,
            tool,
            stream: serde_json::to_value(stream).expect("output stream is serializable"),
            text,
            last: *last,
        },
        ServerEventKind::ToolFinished {
            turn,
            call_id,
            tool,
            is_error,
            error,
            output,
            truncated,
            detached,
            duration_ms,
        } => CanonicalEvent::ToolCallFinished {
            turn: *turn,
            call_id,
            tool,
            is_error: *is_error,
            error,
            output,
            truncated: *truncated,
            detached: *detached,
            duration_ms: *duration_ms,
        },
        ServerEventKind::JobList { jobs } => CanonicalEvent::JobList {
            jobs: serde_json::to_value(jobs).expect("job list is serializable"),
        },
        ServerEventKind::JobsStopped { stopped } => {
            CanonicalEvent::JobsStopped { stopped: *stopped }
        }
        ServerEventKind::QuestionAsked {
            turn,
            question,
            prompt,
            options,
            multiple,
        } => CanonicalEvent::QuestionAsked {
            turn: *turn,
            question,
            prompt,
            options,
            multiple: *multiple,
        },
        ServerEventKind::QuestionAnswered { question } => CanonicalEvent::ApprovalResolved {
            approval_id: question,
        },
        ServerEventKind::TurnFinished {
            turn,
            phase,
            committed,
        } => CanonicalEvent::TurnStateChanged {
            turn: Some(*turn),
            state: json!({"state": "finished", "phase": phase, "committed": committed}),
        },
        ServerEventKind::TurnStopped { reason } => CanonicalEvent::TurnStateChanged {
            turn: None,
            state: json!({"state": "cancelled", "reason": reason}),
        },
        ServerEventKind::RequestReconstructed {
            turn,
            index,
            usage,
            cost_usd,
            truncated,
            duration_ms,
            first_token_ms,
            tokens_per_second,
            cache_hit,
            tools,
        } => CanonicalEvent::OutputPartFinished {
            turn: *turn,
            index: *index,
            usage: serde_json::to_value(usage).expect("usage is serializable"),
            cost_usd: *cost_usd,
            truncated: *truncated,
            duration_ms: *duration_ms,
            first_token_ms: *first_token_ms,
            tokens_per_second: *tokens_per_second,
            cache_hit: *cache_hit,
            tools,
        },
        ServerEventKind::Committed { turn, version } => CanonicalEvent::Committed {
            turn: *turn,
            version: *version,
        },
        ServerEventKind::MessageReplayed {
            index,
            role,
            kind,
            text,
        } => CanonicalEvent::HistoryMessage {
            index: *index,
            role: serde_json::to_value(role).expect("message role is serializable"),
            kind: serde_json::to_value(kind).expect("replay kind is serializable"),
            text,
        },
        ServerEventKind::SessionTodos { todos } => CanonicalEvent::SessionTodos {
            todos: serde_json::to_value(todos).expect("todo list is serializable"),
        },
        ServerEventKind::CompactionStarted { reason } => CanonicalEvent::ContextUpdate {
            state: "started",
            reason: Some(serde_json::to_value(reason).expect("compaction reason is serializable")),
            compacted: None,
            summary: None,
            error: None,
            north_session: None,
        },
        ServerEventKind::CompactionFinished {
            compacted,
            north_session,
            summary,
            error,
        } => CanonicalEvent::ContextUpdate {
            state: "finished",
            reason: None,
            compacted: Some(*compacted),
            summary: summary.as_deref(),
            error: error.as_deref(),
            north_session: north_session.as_deref(),
        },
        ServerEventKind::BudgetWarning {
            scope,
            unit,
            spent,
            limit,
        } => CanonicalEvent::BudgetWarning {
            scope: serde_json::to_value(scope).expect("budget scope is serializable"),
            unit: serde_json::to_value(unit).expect("budget unit is serializable"),
            spent: *spent,
            limit: *limit,
        },
        ServerEventKind::BudgetExceeded {
            scope,
            unit,
            spent,
            limit,
            action,
        } => CanonicalEvent::BudgetExceeded {
            scope: serde_json::to_value(scope).expect("budget scope is serializable"),
            unit: serde_json::to_value(unit).expect("budget unit is serializable"),
            spent: *spent,
            limit: *limit,
            action: serde_json::to_value(action).expect("budget action is serializable"),
        },
        ServerEventKind::BudgetChanged { budget } => CanonicalEvent::BudgetChanged {
            budget: budget
                .as_ref()
                .map(|value| serde_json::to_value(value).expect("budget is serializable")),
        },
        ServerEventKind::SessionStats {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cost_usd,
            requests,
            context_used,
            context_window,
        } => CanonicalEvent::SessionStats {
            input_tokens: *input_tokens,
            output_tokens: *output_tokens,
            cache_read_tokens: *cache_read_tokens,
            cache_write_tokens: *cache_write_tokens,
            cost_usd: *cost_usd,
            requests: *requests,
            context_used: *context_used,
            context_window: *context_window,
        },
        ServerEventKind::CostUpdated {
            total_usd,
            requests,
        } => CanonicalEvent::CostUpdated {
            total_usd: *total_usd,
            requests: *requests,
        },
        ServerEventKind::CostNotReported { requests } => CanonicalEvent::CostNotReported {
            requests: *requests,
        },
        ServerEventKind::PluginList { plugins } => CanonicalEvent::PluginList {
            plugins: serde_json::to_value(plugins).expect("plugin list is serializable"),
        },
        ServerEventKind::PluginChanged { operation, name } => {
            CanonicalEvent::PluginChanged { operation, name }
        }
        ServerEventKind::SteerApplied { ids, messages } => {
            CanonicalEvent::UserInputAccepted { ids, messages }
        }
        ServerEventKind::ModelChanged { provider, model } => CanonicalEvent::AgentChanged {
            change: "model_changed",
            provider: Some(provider),
            model: Some(model),
            details: None,
        },
        ServerEventKind::ModelQueued { provider, model } => CanonicalEvent::AgentChanged {
            change: "model_queued",
            provider: Some(provider),
            model: Some(model),
            details: None,
        },
        ServerEventKind::ReasoningDowngraded { from, to, reason } => CanonicalEvent::AgentChanged {
            change: "reasoning_downgraded",
            provider: None,
            model: None,
            details: Some(json!({"from": from, "to": to, "reason": reason})),
        },
        ServerEventKind::CatalogUpdated { models } => CanonicalEvent::AgentChanged {
            change: "catalog_updated",
            provider: None,
            model: None,
            details: Some(json!({"models": models})),
        },
        ServerEventKind::ModelList { models } => CanonicalEvent::AgentCatalog {
            kind: "models",
            entries: serde_json::to_value(models).expect("model list is serializable"),
        },
        ServerEventKind::SkillList { skills, warnings } => CanonicalEvent::AgentCatalog {
            kind: "skills",
            entries: json!({"skills": skills, "warnings": warnings}),
        },
        ServerEventKind::SkillsUpdated { skills } => CanonicalEvent::AgentChanged {
            change: "skills_updated",
            provider: None,
            model: None,
            details: Some(json!({"skills": skills})),
        },
        ServerEventKind::RetryScheduled {
            turn,
            attempt,
            delay_ms,
            reason,
        } => CanonicalEvent::ProviderRetry {
            turn: *turn,
            state: "scheduled",
            attempt: *attempt,
            delay_ms: Some(*delay_ms),
            reason: Some(reason),
        },
        ServerEventKind::RetryStarted { turn, attempt } => CanonicalEvent::ProviderRetry {
            turn: *turn,
            state: "started",
            attempt: *attempt,
            delay_ms: None,
            reason: None,
        },
        ServerEventKind::Error { code, message } => CanonicalEvent::Error {
            code: serde_json::to_value(code).expect("error code is serializable"),
            message,
        },
        ServerEventKind::Telemetry { records } => CanonicalEvent::ServiceData {
            kind: "telemetry",
            data: serde_json::to_value(records).expect("telemetry is serializable"),
        },
        ServerEventKind::ToolList { tools } => CanonicalEvent::ServiceData {
            kind: "tool_list",
            data: serde_json::to_value(tools).expect("tool list is serializable"),
        },
        ServerEventKind::McpList { servers, failures } => CanonicalEvent::ServiceData {
            kind: "mcp_list",
            data: json!({"servers": servers, "failures": failures}),
        },
    };
    serde_json::to_value(payload).expect("canonical event is serializable")
}

/// Builds the standard RAEP payload for events that have a faithful standard
/// representation. Returns `None` for NorteCode-specific events; those keep
/// the adapter's canonical payload and are sent as extensions.
#[allow(clippy::too_many_lines)]
pub(crate) fn standard_event_payload(kind: &ServerEventKind) -> Option<Value> {
    match kind {
        ServerEventKind::TurnStarted { .. } => Some(json!({"state": "QUEUED"})),
        ServerEventKind::Delta { delta, .. } => {
            let (text, index, channel) = match delta {
                Delta::Text { index, text } | Delta::Refusal { index, text } => {
                    (text.clone(), *index, "text")
                }
                Delta::Reasoning { index, text } => (text.clone(), *index, "reasoning"),
                Delta::ToolArguments {
                    index, arguments, ..
                } => (arguments.clone(), *index, "text"),
                Delta::Media { index, .. } => (String::new(), *index, "text"),
            };
            Some(json!({"delta": text, "index": index, "channel": channel}))
        }
        ServerEventKind::ToolStarted {
            call_id,
            tool,
            arguments,
            ..
        } => {
            let arguments = arguments
                .as_ref()
                .and_then(|arguments| arguments.as_object())
                .cloned()
                .map_or_else(|| json!({}), Value::Object);
            Some(json!({
                "tool_call_id": call_id,
                "tool_name": tool,
                "arguments": arguments,
                "requires_approval": false,
                "executor": "server",
                "attempt": 1
            }))
        }
        ServerEventKind::ToolFinished {
            call_id,
            is_error,
            detached,
            output,
            error,
            truncated,
            duration_ms,
            ..
        } => {
            let status = if *detached {
                "CANCELLED"
            } else if *is_error {
                "ERROR"
            } else {
                "OK"
            };
            let summary = error.clone().or_else(|| output.clone()).unwrap_or_default();
            Some(json!({
                "tool_call_id": call_id,
                "status": status,
                "duration_ms": duration_ms,
                "result_summary": summary,
                "result_truncated": truncated
            }))
        }
        ServerEventKind::ToolOutput {
            call_id,
            tool,
            stream,
            text,
            ..
        } => Some(json!({
            "tool_call_id": call_id,
            "tool_name": tool,
            "stream": stream,
            "delta": text,
            "index": 0
        })),
        // RAEP `TurnStateChanged` carries only `state` and `stop_reason`;
        // `phase`/`committed` are NorteCode's own and travel in the
        // compatibility extension (see `canonical_event_payload`), never
        // smuggled into the canonical payload.
        ServerEventKind::TurnFinished { .. } => {
            Some(json!({"state": "COMPLETED", "stop_reason": "end_turn"}))
        }
        ServerEventKind::TurnStopped { reason } => {
            Some(json!({"state": "CANCELLED", "stop_reason": reason}))
        }
        ServerEventKind::JobList { jobs } => {
            let jobs = jobs
                .iter()
                .map(|job| {
                    let status = match job.state {
                        norte_code_protocol::JobState::Running => "RUNNING",
                        norte_code_protocol::JobState::Finished => "COMPLETED",
                        norte_code_protocol::JobState::Cancelled => "KILLED",
                    };
                    let command = if job.script.is_empty() {
                        job.label.clone()
                    } else {
                        job.script.clone()
                    };
                    json!({
                        "job_id": job.id,
                        "command": command,
                        "status": status
                    })
                })
                .collect::<Vec<_>>();
            Some(json!({ "jobs": jobs }))
        }
        ServerEventKind::MessageReplayed { role, text, .. } => {
            let role = match role {
                norte_code_protocol::MessageRole::Tool => "tool",
                norte_code_protocol::MessageRole::System
                | norte_code_protocol::MessageRole::Developer
                | norte_code_protocol::MessageRole::Unknown => "system_event",
                norte_code_protocol::MessageRole::User => "user",
                norte_code_protocol::MessageRole::Assistant => "assistant",
            };
            Some(json!({
                "items": [{"role": role, "content": text}],
                "has_more": false
            }))
        }
        ServerEventKind::Error { code, message } => Some(json!({
            "code": serde_json::to_value(code).unwrap_or_else(|_| json!("INTERNAL")),
            "message": message,
            "retryable": false
        })),
        _ => None,
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum CanonicalEvent<'a> {
    /// Norte Code service-specific data exposed through the RAEP adapter.
    ServiceData { kind: &'static str, data: Value },
    /// The plugins installed on the remote host.
    PluginList { plugins: Value },
    /// A plugin lifecycle operation completed.
    PluginChanged { operation: &'a str, name: &'a str },
    /// A session was created.
    SessionCreated {
        #[serde(skip_serializing_if = "Option::is_none")]
        title: &'a Option<String>,
    },
    /// The current session list.
    SessionList { sessions: Value },
    /// A session was deleted.
    SessionDeleted,
    /// A session was renamed.
    SessionRenamed { title: &'a str },
    /// A session was forked.
    SessionForked { from: &'a str },
    /// The active session changed.
    SessionSwitched,
    /// A turn began.
    TurnStarted { turn: u64 },
    /// A model token delta.
    TokenDelta { turn: u64, delta: Value },
    /// A committed turn.
    Committed { turn: u64, version: u64 },
    /// A replayed history message.
    HistoryMessage {
        index: u64,
        role: Value,
        kind: Value,
        text: &'a str,
    },
    /// The current session todos.
    SessionTodos { todos: Value },
    /// A tool call began.
    ToolCallStarted {
        turn: u64,
        call_id: &'a str,
        tool: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: &'a Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: &'a Option<String>,
    },
    /// A detached tool/job produced output.
    JobProgressDelta {
        turn: u64,
        call_id: &'a str,
        tool: &'a str,
        stream: Value,
        text: &'a str,
        last: bool,
    },
    /// A tool call ended.
    ToolCallFinished {
        turn: u64,
        call_id: &'a str,
        tool: &'a str,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: &'a Option<String>,
        truncated: bool,
        detached: bool,
        duration_ms: u64,
    },
    /// The jobs currently known by the session.
    JobList { jobs: Value },
    /// The result of stopping jobs.
    JobsStopped { stopped: usize },
    /// A question requiring client input.
    QuestionAsked {
        turn: u64,
        question: &'a str,
        prompt: &'a str,
        options: &'a Vec<String>,
        multiple: bool,
    },
    /// An approval/question was resolved.
    ApprovalResolved { approval_id: &'a str },
    /// A turn changed lifecycle state.
    TurnStateChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        turn: Option<u64>,
        state: Value,
    },
    /// Provider usage and cost for one output part.
    OutputPartFinished {
        turn: u64,
        index: u64,
        usage: Value,
        cost_usd: Option<f64>,
        truncated: bool,
        duration_ms: u64,
        first_token_ms: Option<u64>,
        tokens_per_second: Option<f64>,
        cache_hit: bool,
        tools: &'a Vec<String>,
    },
    /// A context compaction lifecycle update.
    ContextUpdate {
        state: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        compacted: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        north_session: Option<&'a str>,
    },
    /// A budget warning.
    BudgetWarning {
        scope: Value,
        unit: Value,
        spent: f64,
        limit: f64,
    },
    /// A budget was exhausted.
    BudgetExceeded {
        scope: Value,
        unit: Value,
        spent: f64,
        limit: f64,
        action: Value,
    },
    /// The session budget changed.
    BudgetChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        budget: Option<Value>,
    },
    /// Session usage totals.
    SessionStats {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        cost_usd: f64,
        requests: u64,
        context_used: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    /// Running cost total changed.
    CostUpdated { total_usd: f64, requests: u64 },
    /// Provider did not report cost.
    CostNotReported { requests: u64 },
    /// Steering input was accepted.
    UserInputAccepted {
        ids: &'a Vec<String>,
        messages: &'a Vec<String>,
    },
    /// Agent configuration changed.
    AgentChanged {
        change: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    /// An agent catalogue was returned.
    AgentCatalog { kind: &'static str, entries: Value },
    /// A provider retry changed state.
    ProviderRetry {
        turn: u64,
        state: &'static str,
        attempt: u32,
        delay_ms: Option<u64>,
        reason: Option<&'a String>,
    },
    /// A protocol error.
    Error { code: Value, message: &'a str },
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte_code_protocol::types::OutputStream;

    #[test]
    fn tool_output_uses_canonical_job_payload_and_preserves_wire_event() {
        let payload = canonical_event_payload(&ServerEventKind::ToolOutput {
            turn: 3,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: OutputStream::Stdout,
            text: "hello".to_owned(),
            last: true,
        });

        assert_eq!(payload["event"], "job_progress_delta");
        assert_eq!(payload["turn"], 3);
        assert_eq!(payload["call_id"], "call-1");
        assert_eq!(payload["stream"], "stdout");
        assert_eq!(payload["last"], true);
    }

    #[test]
    fn standard_payloads_match_the_raep_schemas() {
        let tool_output = standard_event_payload(&ServerEventKind::ToolOutput {
            turn: 2,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: OutputStream::Stdout,
            text: "hello".to_owned(),
            last: false,
        })
        .expect("standard tool output");
        assert_eq!(
            tool_output,
            json!({
                "tool_call_id": "call-1",
                "tool_name": "shell",
                "stream": "stdout",
                "delta": "hello",
                "index": 0
            })
        );

        let history = standard_event_payload(&ServerEventKind::MessageReplayed {
            index: 0,
            role: norte_code_protocol::MessageRole::User,
            kind: norte_code_protocol::ReplayedKind::Text,
            text: "hi".to_owned(),
        })
        .expect("standard history page");
        assert_eq!(
            history,
            json!({
                "items": [{"role": "user", "content": "hi"}],
                "has_more": false
            })
        );

        let error = standard_event_payload(&ServerEventKind::Error {
            code: norte_code_protocol::ErrorCode::Internal,
            message: "boom".to_owned(),
        })
        .expect("standard error");
        assert_eq!(error["message"], "boom");
        assert_eq!(error["retryable"], false);
        assert!(error["code"].is_string());

        let jobs = standard_event_payload(&ServerEventKind::JobList {
            jobs: vec![norte_code_protocol::JobInfo {
                id: "job-1".to_owned(),
                label: "sleep 1".to_owned(),
                script: String::new(),
                state: norte_code_protocol::JobState::Running,
                exit_code: None,
                timed_out: false,
                elapsed_ms: 10,
                output: String::new(),
                dropped: false,
            }],
        })
        .expect("standard job list");
        assert_eq!(
            jobs,
            json!({
                "jobs": [{"job_id": "job-1", "command": "sleep 1", "status": "RUNNING"}]
            })
        );
    }

    #[test]
    fn standard_token_and_tool_payloads_match_the_raep_schemas() {
        let delta = standard_event_payload(&ServerEventKind::Delta {
            turn: 2,
            delta: Delta::Text {
                index: 3,
                text: "hello".to_owned(),
            },
        })
        .expect("standard token delta");
        assert_eq!(
            delta,
            json!({"delta": "hello", "index": 3, "channel": "text"})
        );

        let finished = standard_event_payload(&ServerEventKind::TurnFinished {
            turn: 2,
            phase: norte_code_protocol::Phase::Idle,
            committed: true,
        })
        .expect("standard turn state");
        assert_eq!(
            finished,
            json!({"state": "COMPLETED", "stop_reason": "end_turn"})
        );

        let started = standard_event_payload(&ServerEventKind::ToolStarted {
            turn: 2,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            arguments: Some(json!({"command": "ls"})),
            source: None,
        })
        .expect("standard tool start");
        assert_eq!(
            started,
            json!({
                "tool_call_id": "call-1",
                "tool_name": "shell",
                "arguments": {"command": "ls"},
                "requires_approval": false,
                "executor": "server",
                "attempt": 1
            })
        );

        let tool_finished = standard_event_payload(&ServerEventKind::ToolFinished {
            turn: 2,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            is_error: false,
            error: None,
            output: Some("done".to_owned()),
            truncated: false,
            detached: false,
            duration_ms: 12,
        })
        .expect("standard tool finish");
        assert_eq!(
            tool_finished,
            json!({
                "tool_call_id": "call-1",
                "status": "OK",
                "duration_ms": 12,
                "result_summary": "done",
                "result_truncated": false
            })
        );
    }
}
