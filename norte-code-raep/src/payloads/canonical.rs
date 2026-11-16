use norte_code_wire::ServerEventKind;
use serde_json::Value;

#[allow(clippy::wildcard_imports)]
use super::*;

#[allow(clippy::too_many_lines)]
pub(crate) fn canonical_event_payload(kind: &ServerEventKind) -> Value {
    let payload = match kind {
        ServerEventKind::SessionCreated { title } => CanonicalEvent::SessionCreated { title },
        ServerEventKind::SessionList { sessions } => CanonicalEvent::SessionList {
            sessions: serde_json::to_value(sessions).expect("session list is serializable"),
        },
        ServerEventKind::SessionDeleted => CanonicalEvent::SessionDeleted,
        ServerEventKind::SessionRenamed { title } => CanonicalEvent::SessionRenamed { title },
        ServerEventKind::SessionForked {
            from,
            from_turn_id,
            title,
        } => CanonicalEvent::SessionForked {
            from_session_id: from.as_str(),
            from_turn_id: from_turn_id.as_deref(),
            title: title.as_deref(),
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
        ServerEventKind::JobStarted {
            job_id,
            tool_call_id,
            command,
            started_at,
            limits,
        } => CanonicalEvent::JobStarted {
            job_id,
            tool_call_id,
            command,
            started_at: *started_at,
            limits,
        },
        ServerEventKind::JobProgressDelta {
            job_id,
            stream,
            delta,
            index,
        } => CanonicalEvent::DetachedJobProgress {
            job_id,
            stream: serde_json::to_value(stream).expect("job output stream is serializable"),
            delta,
            index: *index,
        },
        ServerEventKind::JobFinished {
            job,
            triggered_new_turn,
            new_turn_id,
            injected_into_turn_id,
        } => CanonicalEvent::JobFinished {
            job_id: &job.id,
            job,
            triggered_new_turn: *triggered_new_turn,
            new_turn_id: (*new_turn_id).map(|turn| turn.to_string()),
            injected_into_turn_id: (*injected_into_turn_id).map(|turn| turn.to_string()),
        },
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
        ServerEventKind::TurnPhase { turn, phase } => CanonicalEvent::TurnStateChanged {
            turn: Some(*turn),
            state: TurnStateChangedPayload {
                state: match phase {
                    norte_code_wire::TurnPhaseName::Queued => CanonicalTurnState::Queued,
                    norte_code_wire::TurnPhaseName::CallingProvider => {
                        CanonicalTurnState::CallingProvider
                    }
                    norte_code_wire::TurnPhaseName::ExecutingTool => {
                        CanonicalTurnState::ExecutingTool
                    }
                    norte_code_wire::TurnPhaseName::AwaitingApproval => {
                        CanonicalTurnState::AwaitingApproval
                    }
                },
                phase: None,
                committed: None,
                reason: None,
            },
        },
        ServerEventKind::InputAccepted { turn_id, .. } => CanonicalEvent::UserInputAccepted {
            ids: vec![turn_id.clone()],
            messages: Vec::new(),
        },
        ServerEventKind::TurnFinished {
            turn,
            phase,
            committed,
            ..
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
            provider,
            model,
        } => CanonicalEvent::RequestCompleted {
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
            provider: provider.clone(),
            model: model.clone(),
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
            role: *role,
            kind: *kind,
            text,
        },
        ServerEventKind::SessionTodos { todos } => CanonicalEvent::SessionTodos { todos },
        ServerEventKind::CompactionStarted { reason } => CanonicalEvent::ContextUpdate {
            state: CompactionState::Started,
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
            state: CompactionState::Finished,
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
            scope: (*scope).into(),
            unit: *unit,
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
            scope: (*scope).into(),
            unit: *unit,
            spent: *spent,
            limit: *limit,
            action: *action,
        },
        ServerEventKind::BudgetChanged { budget } => CanonicalEvent::BudgetChanged {
            budget: budget.as_ref(),
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
        ServerEventKind::SteerApplied { ids, messages } => CanonicalEvent::UserInputAccepted {
            ids: ids.clone(),
            messages: messages.clone(),
        },
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
        ServerEventKind::CatalogDetail { providers, models } => CanonicalEvent::AgentCatalog {
            kind: CatalogKind::Providers,
            entries: payload(CatalogDetailPayload {
                providers: providers.clone(),
                models: models.clone(),
            }),
        },
        ServerEventKind::PlanList { plans } => CanonicalEvent::AgentCatalog {
            kind: CatalogKind::Plans,
            entries: serde_json::to_value(plans).expect("plan list is serializable"),
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
            state: ProviderRetryState::Scheduled,
            attempt: *attempt,
            delay_ms: Some(*delay_ms),
            reason: Some(reason),
        },
        ServerEventKind::RetryStarted { turn, attempt } => CanonicalEvent::ProviderRetry {
            turn: *turn,
            state: ProviderRetryState::Started,
            attempt: *attempt,
            delay_ms: None,
            reason: None,
        },
        ServerEventKind::Error { code, message } => CanonicalEvent::Error { code, message },
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
        ServerEventKind::SessionExportReady { export_id, bundle } => {
            CanonicalEvent::SessionExportReady {
                export_id,
                bundle: serde_json::to_value(bundle).expect("export bundle is serializable"),
            }
        }
        ServerEventKind::ImportResult { session_id, id_map } => CanonicalEvent::ImportResult {
            session_id,
            id_map: serde_json::to_value(id_map).expect("id map is serializable"),
        },
        ServerEventKind::ChildSessionStarted {
            child_session_id,
            agent_id,
            tool_call_id,
            lifetime,
            task_summary,
        } => CanonicalEvent::ChildSessionStarted {
            child_session_id,
            agent_id,
            tool_call_id,
            lifetime,
            task_summary,
        },
        ServerEventKind::ChildSessionUpdated {
            child_session_id,
            state,
            reason,
            result,
            output_schema_valid,
        } => CanonicalEvent::ChildSessionUpdated {
            child_session_id,
            state,
            reason,
            result: result.as_ref(),
            output_schema_valid: output_schema_valid.unwrap_or(false),
        },
        ServerEventKind::DelegatorStateChanged {
            child_session_id,
            delegator_suspended,
            by,
            queued_messages,
        } => CanonicalEvent::DelegatorStateChanged {
            child_session_id,
            delegator_suspended: *delegator_suspended,
            by: by.as_deref().unwrap_or(""),
            queued_messages: queued_messages.unwrap_or(0),
        },
        ServerEventKind::ChildAwaitingInput {
            child_session_id,
            question,
            prompt,
        } => CanonicalEvent::ChildAwaitingInput {
            child_session_id,
            question,
            prompt,
        },
        ServerEventKind::MediaPartCompleted { turn, index, media } => {
            CanonicalEvent::MediaPartCompleted {
                turn: *turn,
                index: *index,
                media,
            }
        }
        ServerEventKind::HistoryPage { items, has_more } => CanonicalEvent::HistoryPage {
            items: serde_json::to_value(items).expect("history items serializable"),
            has_more: *has_more,
        },
        ServerEventKind::HistoryRewound {
            to_turn_id,
            inclusive,
            rewound_turn_ids,
            side_effects,
            workspace_restored,
        } => CanonicalEvent::HistoryRewound {
            to_turn_id,
            inclusive: *inclusive,
            rewound_turn_ids: serde_json::to_value(rewound_turn_ids)
                .expect("rewound turn ids serializable"),
            side_effects: serde_json::to_value(side_effects).expect("side effects serializable"),
            workspace_restored: *workspace_restored,
        },
        ServerEventKind::OutputPartStarted {
            turn,
            part_id,
            index,
            modality,
            mime_type,
            streamed,
            bytes_expected,
        } => CanonicalEvent::OutputPartStarted {
            turn: *turn,
            part_id,
            index: *index,
            modality: serde_json::to_value(modality).expect("modality serializable"),
            mime_type: mime_type.clone(),
            streamed: *streamed,
            bytes_expected: *bytes_expected,
        },
        ServerEventKind::OutputPartFinished {
            turn,
            part_id,
            index,
            modality,
            bytes,
            sha256,
            blob,
            error,
        } => CanonicalEvent::OutputPartFinished {
            turn: *turn,
            part_id,
            index: *index,
            modality: serde_json::to_value(modality).expect("modality serializable"),
            bytes: *bytes,
            sha256,
            blob: blob.clone(),
            error: error.clone(),
        },
    };
    serde_json::to_value(payload).expect("canonical event is serializable")
}
