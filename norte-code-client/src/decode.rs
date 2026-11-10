use norte_code_wire::{
    AgentChange, CatalogKind, ClientRequest, JobStatus, ServerEvent, ServiceDataKind,
};
use raep_types::Envelope;
use serde_json::Value;

#[allow(clippy::wildcard_imports)]
use crate::canonical::*;
use crate::error::ClientError;
#[allow(clippy::wildcard_imports)]
use crate::frame::*;

pub(crate) fn timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

pub(crate) fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}
#[allow(clippy::too_many_lines)]
/// The counts a catalogue or skills update reports.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct CatalogCount {
    #[serde(default)]
    pub(crate) models: usize,
    #[serde(default)]
    pub(crate) skills: usize,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn standard_event_from_frame(
    frame: &Envelope<Value>,
) -> Result<Option<ServerEvent>, ClientError> {
    let frame = FrameView::from_envelope(frame);
    let frame_type = frame.frame_type;
    let turn = frame.turn;
    let extension_finished = frame.extension_turn_finished.clone();
    let payload = frame.payload;
    let session = frame.session;
    let event = |kind| ServerEvent {
        request_id: 0,
        session: session.clone(),
        kind,
    };
    let kind = match frame_type {
        None => return Ok(None),
        Some(FrameKind::Other | FrameKind::Error) if payload.is_none() => return Ok(None),
        Some(frame_kind) => {
            let Some(payload) = payload else {
                return Ok(None);
            };
            match frame_kind {
                FrameKind::TokenDelta => {
                    let decoded: TokenDeltaPayload = from_payload(payload)?;
                    let delta = match decoded.channel.as_str() {
                        "reasoning" => norte_code_wire::Delta::Reasoning {
                            index: decoded.index,
                            text: decoded.delta,
                        },
                        _ => norte_code_wire::Delta::Text {
                            index: decoded.index,
                            text: decoded.delta,
                        },
                    };
                    event(norte_code_wire::ServerEventKind::Delta { turn, delta })
                }
                FrameKind::TurnStateChanged => {
                    // The payload is the spec's own `TurnStateChanged` struct: decode
                    // it whole, so an unknown state or a missing field is an error
                    // from serde, not a silent drop or a hand-rolled lookup.
                    let changed: raep_types::TurnStateChanged = from_payload(payload)?;
                    match changed.state {
                        raep_types::TurnState::Queued => {
                            event(norte_code_wire::ServerEventKind::TurnStarted { turn })
                        }
                        raep_types::TurnState::Completed => {
                            // `phase`/`committed` are NorteCode's own fields: the RAEP
                            // spec has no equivalent, so they travel inside the
                            // compatibility extension the server writes, not in the
                            // canonical payload. Read them from there, defaulting to
                            // the only honest values (idle, committed) when absent.
                            let phase = extension_finished
                                .as_ref()
                                .and_then(|finished| finished.phase)
                                .unwrap_or(norte_code_wire::Phase::Idle);
                            let committed = extension_finished
                                .as_ref()
                                .and_then(|finished| finished.committed)
                                .unwrap_or(true);
                            event(norte_code_wire::ServerEventKind::TurnFinished {
                                turn,
                                phase,
                                committed,
                                output_schema_valid: None,
                            })
                        }
                        raep_types::TurnState::Cancelled | raep_types::TurnState::Failed => {
                            let reason = match changed.stop_reason {
                                Some(raep_types::StopReason::EndTurn) => "end_turn",
                                Some(raep_types::StopReason::MaxTokens) => "max_tokens",
                                Some(raep_types::StopReason::Error) => "error",
                                Some(raep_types::StopReason::ToolLimit) => "tool_limit",
                                Some(raep_types::StopReason::BudgetExceeded) => "budget_exceeded",
                                Some(raep_types::StopReason::Cancelled) | None => "cancelled",
                            }
                            .to_owned();
                            event(norte_code_wire::ServerEventKind::TurnStopped { reason })
                        }
                        // Intermediate states carry no turn-boundary meaning for the
                        // client; the next terminal state will arrive on its own.
                        raep_types::TurnState::CallingProvider
                        | raep_types::TurnState::ExecutingTool
                        | raep_types::TurnState::AwaitingApproval => return Ok(None),
                    }
                }
                FrameKind::ToolCallStarted => {
                    let decoded: ToolCallStartedPayload = from_payload(payload)?;
                    event(norte_code_wire::ServerEventKind::ToolStarted {
                        turn,
                        call_id: decoded.tool_call_id,
                        tool: decoded.tool_name,
                        arguments: decoded.arguments,
                        source: decoded.source,
                    })
                }
                FrameKind::ToolOutputDelta => {
                    let decoded: ToolOutputDeltaPayload = from_payload(payload)?;
                    event(norte_code_wire::ServerEventKind::ToolOutput {
                        turn,
                        call_id: decoded.tool_call_id,
                        tool: decoded.tool_name,
                        stream: decoded.stream,
                        text: decoded.delta,
                        last: false,
                    })
                }
                FrameKind::ToolCallFinished => {
                    let decoded: ToolCallFinishedPayload = from_payload(payload)?;
                    event(norte_code_wire::ServerEventKind::ToolFinished {
                        turn,
                        call_id: decoded.tool_call_id,
                        // The finish frame carries the name the start announced, so
                        // the ending row can say which call ended. Older servers
                        // omitted it; fall back to empty rather than failing.
                        tool: decoded.tool_name.unwrap_or_default(),
                        is_error: decoded.status == ToolStatus::Error,
                        error: None,
                        output: decoded.result_summary,
                        truncated: decoded.result_truncated,
                        detached: decoded.status == ToolStatus::Cancelled,
                        duration_ms: decoded.duration_ms,
                    })
                }
                FrameKind::JobListResult => {
                    let decoded: norte_code_wire::JobList = from_payload(payload)?;
                    let jobs = decoded
                        .jobs
                        .into_iter()
                        .map(|job| {
                            let state = match job.status {
                                JobStatus::Completed => norte_code_wire::JobState::Finished,
                                JobStatus::Failed | JobStatus::Killed | JobStatus::TimedOut => {
                                    norte_code_wire::JobState::Cancelled
                                }
                                JobStatus::Running => norte_code_wire::JobState::Running,
                            };
                            norte_code_wire::JobInfo {
                                id: job.job_id,
                                label: job.command,
                                script: String::new(),
                                state,
                                exit_code: job.exit_code,
                                timed_out: job.status == JobStatus::TimedOut,
                                elapsed_ms: 0,
                                output: String::new(),
                                dropped: false,
                                output_bytes_total: 0,
                            }
                        })
                        .collect();
                    event(norte_code_wire::ServerEventKind::JobList { jobs })
                }
                FrameKind::Error => {
                    let decoded: ErrorPayload = from_payload(payload)?;
                    let code = decoded.code.to_ascii_lowercase();
                    let code = serde_json::from_value(serde_json::Value::String(code))
                        .unwrap_or(norte_code_wire::ErrorCode::Internal);
                    event(norte_code_wire::ServerEventKind::Error {
                        code,
                        message: decoded.message,
                    })
                }
                FrameKind::Other => return Ok(None),
            }
        }
    };
    Ok(Some(kind))
}

#[allow(clippy::too_many_lines)]
pub(crate) fn server_event_from_frame(frame: &Envelope<Value>) -> Result<ServerEvent, ClientError> {
    if frame.version != raep_types::WIRE_VERSION {
        return Err(ClientError::new(format!(
            "unsupported RAEP wire version `{}`; this client speaks {}",
            frame.version,
            raep_types::WIRE_VERSION
        )));
    }
    // A canonical frame that fails to decode is a wire error, not a reason
    // to fall back to the extension: silently splicing in the compatibility
    // copy would hide a server that stopped following the spec.
    if let Some(event) = standard_event_from_frame(frame)? {
        return Ok(event);
    }
    if let Some(event) = frame
        .ext
        .as_ref()
        .and_then(|ext| ext.get("norte_code"))
        .cloned()
    {
        return serde_json::from_value(event)
            .map_err(|error| ClientError::new(format!("invalid NorteCode event: {error}")));
    }
    let session = (!frame.session_id.is_empty())
        .then(|| norte_code_wire::SessionKey::new(frame.session_id.clone()));
    let canonical = CanonicalPayload::from_value(frame.payload.clone())?;
    let kind = match canonical.kind {
        CanonicalKind::SessionCreated => {
            let decoded: CanonicalSessionCreated = canonical.decode()?;
            norte_code_wire::ServerEventKind::SessionCreated {
                title: decoded.title,
            }
        }
        CanonicalKind::SessionList => {
            let decoded: CanonicalSessionList = canonical.decode()?;
            norte_code_wire::ServerEventKind::SessionList {
                sessions: decoded.sessions,
            }
        }
        CanonicalKind::SessionDeleted => norte_code_wire::ServerEventKind::SessionDeleted,
        CanonicalKind::SessionForked => {
            let decoded: CanonicalSessionForked = canonical.decode()?;
            norte_code_wire::ServerEventKind::SessionForked {
                from: norte_code_wire::SessionKey::new(decoded.from),
                from_turn_id: decoded.from_turn_id,
                title: decoded.title,
            }
        }
        CanonicalKind::TurnStarted => {
            let decoded: CanonicalTurnStarted = canonical.decode()?;
            norte_code_wire::ServerEventKind::TurnStarted { turn: decoded.turn }
        }
        CanonicalKind::TokenDelta => {
            let decoded: CanonicalTokenDelta = canonical.decode()?;
            norte_code_wire::ServerEventKind::Delta {
                turn: decoded.turn,
                delta: decoded.delta,
            }
        }
        CanonicalKind::Committed => {
            let decoded: CanonicalCommitted = canonical.decode()?;
            norte_code_wire::ServerEventKind::Committed {
                turn: decoded.turn,
                version: decoded.version,
            }
        }
        CanonicalKind::HistoryMessage => {
            let decoded: CanonicalHistoryMessage = canonical.decode()?;
            norte_code_wire::ServerEventKind::MessageReplayed {
                index: decoded.index,
                role: decoded.role,
                kind: decoded.kind,
                text: decoded.text,
            }
        }
        CanonicalKind::SessionTodos => {
            let decoded: CanonicalSessionTodos = canonical.decode()?;
            norte_code_wire::ServerEventKind::SessionTodos {
                todos: decoded.todos,
            }
        }
        CanonicalKind::PlanList => {
            let decoded: CanonicalPlanList = canonical.decode()?;
            norte_code_wire::ServerEventKind::PlanList {
                plans: decoded.plans,
            }
        }
        CanonicalKind::QuestionAsked => {
            let decoded: CanonicalQuestionAsked = canonical.decode()?;
            norte_code_wire::ServerEventKind::QuestionAsked {
                turn: decoded.turn,
                question: decoded.question,
                prompt: decoded.prompt,
                options: decoded.options,
                multiple: decoded.multiple,
            }
        }
        CanonicalKind::ToolCallStarted => {
            let decoded: CanonicalToolCallStarted = canonical.decode()?;
            norte_code_wire::ServerEventKind::ToolStarted {
                turn: decoded.turn,
                call_id: decoded.call_id,
                tool: decoded.tool,
                arguments: decoded.arguments,
                source: decoded.source,
            }
        }
        CanonicalKind::JobProgressDelta => {
            let decoded: CanonicalJobProgressDelta = canonical.decode()?;
            norte_code_wire::ServerEventKind::ToolOutput {
                turn: decoded.turn,
                call_id: decoded.call_id,
                tool: decoded.tool,
                stream: decoded.stream,
                text: decoded.text,
                last: decoded.last,
            }
        }
        CanonicalKind::ToolCallFinished => {
            let decoded: CanonicalToolCallFinished = canonical.decode()?;
            norte_code_wire::ServerEventKind::ToolFinished {
                turn: decoded.turn,
                call_id: decoded.call_id,
                tool: decoded.tool,
                is_error: decoded.is_error,
                error: decoded.error,
                output: decoded.output,
                truncated: decoded.truncated,
                detached: decoded.detached,
                duration_ms: decoded.duration_ms,
            }
        }
        CanonicalKind::OutputPartFinished | CanonicalKind::RequestCompleted => {
            let decoded: CanonicalOutputPartFinished = canonical.decode()?;
            norte_code_wire::ServerEventKind::RequestReconstructed {
                turn: decoded.turn,
                index: decoded.index,
                usage: decoded.usage,
                cost_usd: decoded.cost_usd,
                truncated: decoded.truncated,
                duration_ms: decoded.duration_ms,
                first_token_ms: decoded.first_token_ms,
                tokens_per_second: decoded.tokens_per_second,
                cache_hit: decoded.cache_hit,
                tools: decoded.tools,
                provider: decoded.provider,
                model: decoded.model,
            }
        }
        CanonicalKind::ApprovalResolved => {
            let decoded: CanonicalApprovalResolved = canonical.decode()?;
            norte_code_wire::ServerEventKind::QuestionAnswered {
                question: decoded.approval_id,
            }
        }
        CanonicalKind::UserInputAccepted => {
            let decoded: CanonicalUserInputAccepted = canonical.decode()?;
            norte_code_wire::ServerEventKind::SteerApplied {
                ids: decoded.ids,
                messages: decoded.messages,
            }
        }
        CanonicalKind::ProviderRetry => {
            let decoded: CanonicalProviderRetry = canonical.decode()?;
            match decoded.state {
                RetryState::Scheduled => norte_code_wire::ServerEventKind::RetryScheduled {
                    turn: decoded.turn,
                    attempt: decoded.attempt,
                    delay_ms: decoded.delay_ms,
                    reason: decoded.reason,
                },
                RetryState::Started => norte_code_wire::ServerEventKind::RetryStarted {
                    turn: decoded.turn,
                    attempt: decoded.attempt,
                },
            }
        }
        CanonicalKind::AgentChanged => {
            let decoded: CanonicalAgentChanged = canonical.decode()?;
            match decoded.change {
                AgentChange::ModelChanged => norte_code_wire::ServerEventKind::ModelChanged {
                    provider: decoded.provider.unwrap_or_default(),
                    model: decoded.model.unwrap_or_default(),
                },
                AgentChange::ModelQueued => norte_code_wire::ServerEventKind::ModelQueued {
                    provider: decoded.provider.unwrap_or_default(),
                    model: decoded.model.unwrap_or_default(),
                },
                AgentChange::ReasoningDowngraded => {
                    let details: ReasoningDowngradedDetails = decoded
                        .details
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|error| {
                            ClientError::new(format!("invalid reasoning downgrade: {error}"))
                        })?
                        .unwrap_or(ReasoningDowngradedDetails {
                            from: String::new(),
                            to: String::new(),
                            reason: String::new(),
                        });
                    norte_code_wire::ServerEventKind::ReasoningDowngraded {
                        from: details.from,
                        to: details.to,
                        reason: details.reason,
                    }
                }
                AgentChange::CatalogUpdated => {
                    let count: CatalogCount = decoded
                        .details
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|error| {
                            ClientError::new(format!("invalid catalogue update: {error}"))
                        })?
                        .unwrap_or_default();
                    norte_code_wire::ServerEventKind::CatalogUpdated {
                        models: count.models,
                    }
                }
                AgentChange::SkillsUpdated => {
                    let count: CatalogCount = decoded
                        .details
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|error| {
                            ClientError::new(format!("invalid skills update: {error}"))
                        })?
                        .unwrap_or_default();
                    norte_code_wire::ServerEventKind::SkillsUpdated {
                        skills: count.skills,
                    }
                }
            }
        }
        CanonicalKind::AgentCatalog => {
            let decoded: CanonicalAgentCatalog = canonical.decode()?;
            match decoded.kind {
                CatalogKind::Models => norte_code_wire::ServerEventKind::ModelList {
                    models: serde_json::from_value(decoded.entries).map_err(|error| {
                        ClientError::new(format!("invalid model list: {error}"))
                    })?,
                },
                CatalogKind::Skills => {
                    let entries: SkillCatalogEntries = serde_json::from_value(decoded.entries)
                        .map_err(|error| {
                            ClientError::new(format!("invalid skill list: {error}"))
                        })?;
                    norte_code_wire::ServerEventKind::SkillList {
                        skills: entries.skills,
                        warnings: entries.warnings,
                    }
                }
                // The editable catalogue and the plan list have their own
                // frames; an `agent_catalog` carrying them has nothing to add
                // to this event stream.
                CatalogKind::Providers | CatalogKind::Plans => {
                    return Err(ClientError::new(
                        "agent_catalog carried a catalogue this client reads elsewhere",
                    ));
                }
            }
        }
        CanonicalKind::SessionRenamed => {
            let decoded: CanonicalSessionRenamed = canonical.decode()?;
            norte_code_wire::ServerEventKind::SessionRenamed {
                title: decoded.title,
            }
        }
        CanonicalKind::SessionStats => {
            let decoded: CanonicalSessionStats = canonical.decode()?;
            norte_code_wire::ServerEventKind::SessionStats {
                input_tokens: decoded.input_tokens,
                output_tokens: decoded.output_tokens,
                cache_read_tokens: decoded.cache_read_tokens,
                cache_write_tokens: decoded.cache_write_tokens,
                cost_usd: decoded.cost_usd,
                requests: decoded.requests,
                context_used: decoded.context_used,
                context_window: decoded.context_window,
            }
        }
        CanonicalKind::JobList => {
            let decoded: CanonicalJobList = canonical.decode()?;
            norte_code_wire::ServerEventKind::JobList { jobs: decoded.jobs }
        }
        CanonicalKind::TurnStateChanged => {
            let decoded: CanonicalTurnStateChanged = canonical.decode()?;
            match decoded.state {
                CanonicalTurnState::Finished => norte_code_wire::ServerEventKind::TurnFinished {
                    turn: decoded.turn,
                    phase: decoded.phase.unwrap_or(norte_code_wire::Phase::Idle),
                    committed: decoded.committed.unwrap_or(true),
                    output_schema_valid: None,
                },
                CanonicalTurnState::Stopped => norte_code_wire::ServerEventKind::TurnStopped {
                    reason: decoded.reason.unwrap_or_default(),
                },
            }
        }
        CanonicalKind::ContextUpdate => {
            let decoded: CanonicalContextUpdate = canonical.decode()?;
            match decoded.state {
                CanonicalContextState::Started => {
                    norte_code_wire::ServerEventKind::CompactionStarted {
                        reason: decoded.reason.unwrap_or_default(),
                    }
                }
                CanonicalContextState::Finished => {
                    norte_code_wire::ServerEventKind::CompactionFinished {
                        compacted: decoded.compacted.unwrap_or(false),
                        north_session: decoded.north_session,
                        summary: decoded.summary,
                        error: decoded.error,
                    }
                }
            }
        }
        CanonicalKind::BudgetWarning => {
            let decoded: CanonicalBudgetWarning = canonical.decode()?;
            norte_code_wire::ServerEventKind::BudgetWarning {
                scope: decoded.scope,
                unit: decoded.unit,
                spent: decoded.spent,
                limit: decoded.limit,
            }
        }
        CanonicalKind::BudgetExceeded => {
            let decoded: CanonicalBudgetExceeded = canonical.decode()?;
            norte_code_wire::ServerEventKind::BudgetExceeded {
                scope: decoded.scope,
                unit: decoded.unit,
                spent: decoded.spent,
                limit: decoded.limit,
                action: decoded.action,
            }
        }
        CanonicalKind::BudgetChanged => {
            let decoded: CanonicalBudgetChanged = canonical.decode()?;
            norte_code_wire::ServerEventKind::BudgetChanged {
                budget: decoded.budget,
            }
        }
        CanonicalKind::CostUpdated => {
            let decoded: CanonicalCostUpdated = canonical.decode()?;
            norte_code_wire::ServerEventKind::CostUpdated {
                total_usd: decoded.total_usd,
                requests: decoded.requests,
            }
        }
        CanonicalKind::CostNotReported => {
            let decoded: CanonicalCostNotReported = canonical.decode()?;
            norte_code_wire::ServerEventKind::CostNotReported {
                requests: decoded.requests,
            }
        }
        CanonicalKind::PluginList => {
            let decoded: CanonicalPluginList = canonical.decode()?;
            norte_code_wire::ServerEventKind::PluginList {
                plugins: decoded.plugins,
            }
        }
        CanonicalKind::PluginChanged => {
            let decoded: CanonicalPluginChanged = canonical.decode()?;
            norte_code_wire::ServerEventKind::PluginChanged {
                operation: decoded.operation,
                name: decoded.name,
            }
        }
        CanonicalKind::ServiceData => {
            let decoded: CanonicalServiceData = canonical.decode()?;
            match decoded.kind {
                ServiceDataKind::Telemetry => norte_code_wire::ServerEventKind::Telemetry {
                    records: serde_json::from_value(decoded.data).map_err(|error| {
                        ClientError::new(format!("invalid telemetry payload: {error}"))
                    })?,
                },
                ServiceDataKind::ToolList => norte_code_wire::ServerEventKind::ToolList {
                    tools: serde_json::from_value(decoded.data).map_err(|error| {
                        ClientError::new(format!("invalid tool list payload: {error}"))
                    })?,
                },
                ServiceDataKind::McpList => {
                    let data: McpListData =
                        serde_json::from_value(decoded.data).map_err(|error| {
                            ClientError::new(format!("invalid MCP payload: {error}"))
                        })?;
                    norte_code_wire::ServerEventKind::McpList {
                        servers: data.servers,
                        failures: data.failures,
                    }
                }
            }
        }
        CanonicalKind::ChildSessionStarted => {
            let decoded: CanonicalChildSessionStarted = canonical.decode()?;
            norte_code_wire::ServerEventKind::ChildSessionStarted {
                child_session_id: decoded.child_session_id,
                agent_id: decoded.agent_id,
                tool_call_id: decoded.tool_call_id,
                lifetime: decoded.lifetime,
                task_summary: decoded.task_summary,
            }
        }
        CanonicalKind::ChildSessionUpdated => {
            let decoded: CanonicalChildSessionUpdated = canonical.decode()?;
            norte_code_wire::ServerEventKind::ChildSessionUpdated {
                child_session_id: decoded.child_session_id,
                state: decoded.state,
                reason: decoded.reason,
                result: decoded.result,
                output_schema_valid: decoded.output_schema_valid,
            }
        }
        CanonicalKind::DelegatorStateChanged => {
            let decoded: CanonicalDelegatorStateChanged = canonical.decode()?;
            norte_code_wire::ServerEventKind::DelegatorStateChanged {
                child_session_id: decoded.child_session_id,
                delegator_suspended: decoded.delegator_suspended,
                by: decoded.by,
                queued_messages: decoded.queued_messages,
            }
        }
        CanonicalKind::ChildAwaitingInput => {
            let decoded: CanonicalChildAwaitingInput = canonical.decode()?;
            norte_code_wire::ServerEventKind::ChildAwaitingInput {
                child_session_id: decoded.child_session_id,
                question: decoded.question,
                prompt: decoded.prompt,
            }
        }
        CanonicalKind::Error => {
            let decoded: CanonicalError = canonical.decode()?;
            let code = serde_json::from_value(decoded.code.clone())
                .unwrap_or(norte_code_wire::ErrorCode::Internal);
            norte_code_wire::ServerEventKind::Error {
                code,
                message: decoded.message,
            }
        }
        CanonicalKind::Other => {
            return Err(ClientError::new(
                "unsupported RAEP canonical event".to_owned(),
            ));
        }
    };
    Ok(ServerEvent {
        request_id: 0,
        session,
        kind,
    })
}

pub(crate) fn session_from_request(request: &ClientRequest) -> Option<String> {
    use norte_code_wire::ClientRequestKind;
    let session = match &request.kind {
        ClientRequestKind::ListPlans { session }
        | ClientRequestKind::ForkSession { session, .. }
        | ClientRequestKind::RenameSession { session, .. }
        | ClientRequestKind::DeleteSession { session }
        | ClientRequestKind::Steer { session, .. }
        | ClientRequestKind::CancelTurn { session }
        | ClientRequestKind::StopJobs { session, .. }
        | ClientRequestKind::DetachTurn { session }
        | ClientRequestKind::ListJobs { session }
        | ClientRequestKind::Resume { session, .. }
        | ClientRequestKind::AnswerQuestion { session, .. }
        | ClientRequestKind::SetModel { session, .. }
        | ClientRequestKind::SetReasoning { session, .. }
        | ClientRequestKind::SetBudget { session, .. }
        | ClientRequestKind::Compact { session }
        | ClientRequestKind::ListTools {
            session: Some(session),
        }
        | ClientRequestKind::SetApprovalRule {
            session: Some(session),
            ..
        }
        | ClientRequestKind::ForgetApprovalRule {
            session: Some(session),
            ..
        }
        | ClientRequestKind::SessionCompact { session, .. }
        | ClientRequestKind::ContextPin { session, .. }
        | ClientRequestKind::SessionRewind { session, .. }
        | ClientRequestKind::HistoryGet { session, .. }
        | ClientRequestKind::SessionExport { session, .. } => Some(session),
        ClientRequestKind::SendMessage { session, .. }
        | ClientRequestKind::CostReport { session }
        | ClientRequestKind::ListTelemetry { session } => session.as_ref(),
        ClientRequestKind::ListMcp
        | ClientRequestKind::RefreshMcp
        | ClientRequestKind::SetMcpActive { .. }
        | ClientRequestKind::CreateSession { .. }
        | ClientRequestKind::ListSessions
        | ClientRequestKind::ListModels
        | ClientRequestKind::ListCatalog
        | ClientRequestKind::ListTools { session: None }
        | ClientRequestKind::SetApprovalRule { session: None, .. }
        | ClientRequestKind::ForgetApprovalRule { session: None, .. }
        | ClientRequestKind::RefreshCatalog
        | ClientRequestKind::CatalogUpsertProvider { .. }
        | ClientRequestKind::CatalogRemoveProvider { .. }
        | ClientRequestKind::CatalogUpsertModel { .. }
        | ClientRequestKind::CatalogRemoveModel { .. }
        | ClientRequestKind::ListSkills
        | ClientRequestKind::RefreshSkills
        | ClientRequestKind::ListPlugins
        | ClientRequestKind::InstallPlugin { .. }
        | ClientRequestKind::UpdatePlugins { .. }
        | ClientRequestKind::RemovePlugin { .. }
        | ClientRequestKind::SetPluginEnabled { .. }
        | ClientRequestKind::SessionImport { .. } => None,
    }?;
    Some(session.as_str().to_owned())
}
