//! Canonical RAEP payloads emitted by the `NorteCode` adapter.

use norte_code_protocol::{Delta, ServerEventKind};
use raep_types::{ErrorPayload, HistoryItem, HistoryPage, StopReason, TurnState, TurnStateChanged};
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

/// Serializes a typed RAEP payload. The payloads are closed structs, so this
/// cannot fail; the helper exists so call sites never build wire JSON by hand.
///
/// The return is `Value` only because the envelope's payload slot is the
/// spec's generic `P` — every caller passes a closed struct, and the client
/// decodes it back into the matching struct. No payload is built as `Value`.
pub(crate) fn payload<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("typed RAEP payload is serializable")
}

/// A token delta frame. RAEP has no struct for this, so the adapter owns one
/// here and serializes it; the shape is the wire contract, not a literal.
#[derive(Debug, Serialize)]
struct TokenDeltaPayload {
    delta: String,
    index: u32,
    channel: &'static str,
}

/// A tool call start frame.
#[derive(Debug, Serialize)]
struct ToolCallStartedPayload<'a> {
    tool_call_id: &'a str,
    tool_name: &'a str,
    arguments: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,
    requires_approval: bool,
    executor: &'static str,
    attempt: u32,
}

/// A tool call finish frame.
#[derive(Debug, Serialize)]
struct ToolCallFinishedPayload<'a> {
    tool_call_id: &'a str,
    tool_name: &'a str,
    status: &'static str,
    duration_ms: u64,
    result_summary: String,
    result_truncated: bool,
}

/// A tool output delta frame.
#[derive(Debug, Serialize)]
struct ToolOutputDeltaPayload<'a> {
    tool_call_id: &'a str,
    tool_name: &'a str,
    stream: &'a norte_code_protocol::types::OutputStream,
    delta: &'a str,
    index: u32,
}

/// One job in a job list frame.
#[derive(Debug, Serialize)]
struct JobEntryPayload {
    job_id: String,
    command: String,
    status: &'static str,
}

/// A job list frame.
#[derive(Debug, Serialize)]
struct JobListPayload {
    jobs: Vec<JobEntryPayload>,
}

/// How a turn ended, in the canonical payload's spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CanonicalTurnState {
    Finished,
    Stopped,
}

/// The canonical `turn_state_changed` state block.
#[derive(Debug, Serialize)]
struct TurnStateChangedPayload {
    state: CanonicalTurnState,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<norte_code_protocol::Phase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// What kind of agent change a canonical payload describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AgentChange {
    ModelChanged,
    ModelQueued,
    ReasoningDowngraded,
    CatalogUpdated,
    SkillsUpdated,
}

/// The `details` of a `reasoning_downgraded` change.
#[derive(Debug, Serialize)]
struct ReasoningDowngradedDetails {
    from: String,
    to: String,
    reason: String,
}

/// The `details` of a `catalog_updated` change.
#[derive(Debug, Serialize)]
struct CatalogUpdatedDetails {
    models: usize,
}

/// The `details` of a `skills_updated` change.
#[derive(Debug, Serialize)]
struct SkillsUpdatedDetails {
    skills: usize,
}

/// What kind of catalogue a canonical payload carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CatalogKind {
    Models,
    Skills,
}

/// The `entries` of a `skills` catalogue.
#[derive(Debug, Serialize)]
struct SkillCatalogEntries {
    skills: Vec<norte_code_protocol::SkillInfo>,
    warnings: Vec<String>,
}

/// What kind of service data a canonical payload carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ServiceDataKind {
    Telemetry,
    ToolList,
    McpList,
}

/// The `data` of an `mcp_list` service payload.
#[derive(Debug, Serialize)]
struct McpListData {
    servers: Vec<norte_code_protocol::McpInfo>,
    failures: Vec<norte_code_protocol::McpFailure>,
}

/// The `content` of a replayed history item.
#[derive(Debug, Serialize)]
struct HistoryContent<'a> {
    role: &'a str,
    content: &'a str,
}

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
            state: TurnStateChangedPayload {
                state: CanonicalTurnState::Finished,
                phase: Some(*phase),
                committed: Some(*committed),
                reason: None,
            },
        },
        ServerEventKind::TurnStopped { reason } => CanonicalEvent::TurnStateChanged {
            turn: None,
            state: TurnStateChangedPayload {
                state: CanonicalTurnState::Stopped,
                phase: None,
                committed: None,
                reason: Some(reason.clone()),
            },
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
            change: AgentChange::ModelChanged,
            provider: Some(provider),
            model: Some(model),
            details: None,
        },
        ServerEventKind::ModelQueued { provider, model } => CanonicalEvent::AgentChanged {
            change: AgentChange::ModelQueued,
            provider: Some(provider),
            model: Some(model),
            details: None,
        },
        ServerEventKind::ReasoningDowngraded { from, to, reason } => CanonicalEvent::AgentChanged {
            change: AgentChange::ReasoningDowngraded,
            provider: None,
            model: None,
            details: Some(payload(ReasoningDowngradedDetails {
                from: from.clone(),
                to: to.clone(),
                reason: reason.clone(),
            })),
        },
        ServerEventKind::CatalogUpdated { models } => CanonicalEvent::AgentChanged {
            change: AgentChange::CatalogUpdated,
            provider: None,
            model: None,
            details: Some(payload(CatalogUpdatedDetails { models: *models })),
        },
        ServerEventKind::ModelList { models } => CanonicalEvent::AgentCatalog {
            kind: CatalogKind::Models,
            entries: serde_json::to_value(models).expect("model list is serializable"),
        },
        ServerEventKind::SkillList { skills, warnings } => CanonicalEvent::AgentCatalog {
            kind: CatalogKind::Skills,
            entries: payload(SkillCatalogEntries {
                skills: skills.clone(),
                warnings: warnings.clone(),
            }),
        },
        ServerEventKind::SkillsUpdated { skills } => CanonicalEvent::AgentChanged {
            change: AgentChange::SkillsUpdated,
            provider: None,
            model: None,
            details: Some(payload(SkillsUpdatedDetails { skills: *skills })),
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
            kind: ServiceDataKind::Telemetry,
            data: serde_json::to_value(records).expect("telemetry is serializable"),
        },
        ServerEventKind::ToolList { tools } => CanonicalEvent::ServiceData {
            kind: ServiceDataKind::ToolList,
            data: serde_json::to_value(tools).expect("tool list is serializable"),
        },
        ServerEventKind::McpList { servers, failures } => CanonicalEvent::ServiceData {
            kind: ServiceDataKind::McpList,
            data: payload(McpListData {
                servers: servers.clone(),
                failures: failures.clone(),
            }),
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
        ServerEventKind::TurnStarted { .. } => Some(payload(TurnStateChanged {
            state: TurnState::Queued,
            stop_reason: None,
            usage: None,
            error: None,
        })),
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
            Some(payload(TokenDeltaPayload {
                delta: text,
                index,
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
            Some(payload(ToolCallStartedPayload {
                tool_call_id: call_id,
                tool_name: tool,
                arguments: Value::Object(arguments),
                source: source.as_deref(),
                requires_approval: false,
                executor: "server",
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
            let status = if *detached {
                "CANCELLED"
            } else if *is_error {
                "ERROR"
            } else {
                "OK"
            };
            let summary = error.clone().or_else(|| output.clone()).unwrap_or_default();
            Some(payload(ToolCallFinishedPayload {
                tool_call_id: call_id,
                tool_name: tool,
                status,
                duration_ms: *duration_ms,
                result_summary: summary,
                result_truncated: *truncated,
            }))
        }
        ServerEventKind::ToolOutput {
            call_id,
            tool,
            stream,
            text,
            ..
        } => Some(payload(ToolOutputDeltaPayload {
            tool_call_id: call_id,
            tool_name: tool,
            stream,
            delta: text,
            index: 0,
        })),
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
            state: TurnState::Cancelled,
            stop_reason: Some(match reason.as_str() {
                "max_tokens" => StopReason::MaxTokens,
                "tool_limit" => StopReason::ToolLimit,
                "budget" | "budget_exceeded" => StopReason::BudgetExceeded,
                _ => StopReason::Cancelled,
            }),
            usage: None,
            error: None,
        })),
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
                    JobEntryPayload {
                        job_id: job.id.clone(),
                        command,
                        status,
                    }
                })
                .collect::<Vec<_>>();
            Some(payload(JobListPayload { jobs }))
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
            Some(payload(HistoryPage {
                items: vec![HistoryItem {
                    id: String::new(),
                    kind: role.to_owned(),
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
        _ => None,
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum CanonicalEvent<'a> {
    /// Norte Code service-specific data exposed through the RAEP adapter.
    ServiceData { kind: ServiceDataKind, data: Value },
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
        state: TurnStateChangedPayload,
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
        change: AgentChange,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    /// An agent catalogue was returned.
    AgentCatalog { kind: CatalogKind, entries: Value },
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
                "items": [{
                    "id": "",
                    "kind": "user",
                    "content": {"role": "user", "content": "hi"},
                    "created_at": 0
                }]
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
                "tool_name": "shell",
                "status": "OK",
                "duration_ms": 12,
                "result_summary": "done",
                "result_truncated": false
            })
        );
    }
}
