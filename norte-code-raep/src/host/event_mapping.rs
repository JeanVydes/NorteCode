//! Mapping service events and requests onto RAEP frame behavior.

use norte_code_wire::{ClientRequestKind, ServerEventKind, SessionKey};
use raep_types::{FrameType, StandardFrameType};
use serde_json::Value;

pub(crate) fn turn_id_for_event(kind: &ServerEventKind) -> Option<u64> {
    match kind {
        ServerEventKind::TurnStarted { turn }
        | ServerEventKind::Delta { turn, .. }
        | ServerEventKind::MediaPartCompleted { turn, .. }
        | ServerEventKind::ToolStarted { turn, .. }
        | ServerEventKind::ToolOutput { turn, .. }
        | ServerEventKind::ToolFinished { turn, .. }
        | ServerEventKind::RequestReconstructed { turn, .. }
        | ServerEventKind::TurnFinished { turn, .. }
        | ServerEventKind::QuestionAsked { turn, .. }
        | ServerEventKind::RetryScheduled { turn, .. }
        | ServerEventKind::RetryStarted { turn, .. } => Some(*turn),
        _ => None,
    }
}

pub(crate) fn job_id_for_event(kind: &ServerEventKind) -> Option<String> {
    match kind {
        ServerEventKind::JobStarted { job_id, .. }
        | ServerEventKind::JobProgressDelta { job_id, .. } => Some(job_id.clone()),
        ServerEventKind::JobFinished { job, .. } => Some(job.id.clone()),
        _ => None,
    }
}

pub(crate) fn frame_type_for_event(kind: &ServerEventKind) -> FrameType {
    match kind {
        ServerEventKind::Delta { .. } => standard(StandardFrameType::TokenDelta),
        ServerEventKind::MediaPartCompleted { .. } => {
            standard(StandardFrameType::OutputPartFinished)
        }
        ServerEventKind::ToolStarted { .. } => standard(StandardFrameType::ToolCallStarted),
        ServerEventKind::ToolOutput { .. } => standard(StandardFrameType::ToolOutputDelta),
        ServerEventKind::ToolFinished { .. } => standard(StandardFrameType::ToolCallFinished),
        ServerEventKind::JobStarted { .. } => standard(StandardFrameType::JobStarted),
        ServerEventKind::JobProgressDelta { .. } => standard(StandardFrameType::JobProgressDelta),
        ServerEventKind::JobFinished { .. } => standard(StandardFrameType::JobFinished),
        ServerEventKind::TurnStarted { .. }
        | ServerEventKind::TurnFinished { .. }
        | ServerEventKind::TurnStopped { .. } => standard(StandardFrameType::TurnStateChanged),
        ServerEventKind::JobList { .. } => standard(StandardFrameType::JobListResult),
        ServerEventKind::MessageReplayed { .. } => standard(StandardFrameType::HistoryPage),
        ServerEventKind::Error { .. } => standard(StandardFrameType::Error),
        ServerEventKind::SteerApplied { .. } => standard(StandardFrameType::UserInputAccepted),
        ServerEventKind::ChildSessionStarted { .. } => {
            standard(StandardFrameType::ChildSessionStarted)
        }
        ServerEventKind::ChildSessionUpdated { .. }
        | ServerEventKind::ChildAwaitingInput { .. } => {
            standard(StandardFrameType::ChildSessionUpdated)
        }
        ServerEventKind::DelegatorStateChanged { .. } => {
            standard(StandardFrameType::DelegatorStateChanged)
        }
        other => FrameType::Extension(extension_name_for_event(other)),
    }
}

/// Names a `NorteCode` event that has no faithful standard frame.
///
/// Extensions carry the adapter's own payload, so a standard frame is never
/// sent with a shape the standard does not define.
pub(crate) fn extension_name_for_event(kind: &ServerEventKind) -> String {
    let tag = serde_json::to_value(kind)
        .ok()
        .and_then(|value| {
            value
                .get("event")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "event".to_owned());
    format!("norte_code.{tag}")
}

pub(crate) fn session_for_request(kind: &ClientRequestKind) -> Option<&SessionKey> {
    match kind {
        ClientRequestKind::RenameSession { session, .. }
        | ClientRequestKind::DeleteSession { session }
        | ClientRequestKind::CancelTurn { session }
        | ClientRequestKind::StopJobs { session, .. }
        | ClientRequestKind::DetachTurn { session }
        | ClientRequestKind::ListJobs { session }
        | ClientRequestKind::ListPlans { session }
        | ClientRequestKind::Resume { session, .. }
        | ClientRequestKind::AnswerQuestion { session, .. }
        | ClientRequestKind::SetModel { session, .. }
        | ClientRequestKind::SetReasoning { session, .. }
        | ClientRequestKind::SetBudget { session, .. }
        | ClientRequestKind::Compact { session }
        | ClientRequestKind::ForkSession { session, .. }
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
        ClientRequestKind::CreateSession { .. }
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
        | ClientRequestKind::ListMcp
        | ClientRequestKind::RefreshMcp
        | ClientRequestKind::SetMcpActive { .. }
        | ClientRequestKind::SessionImport { .. }
        | ClientRequestKind::SaveSecret { .. }
        | ClientRequestKind::DeleteSecret { .. } => None,
    }
}

/// Whether a command only reads state and can run without an attachment.
pub(crate) fn is_read_only_request(kind: &ClientRequestKind) -> bool {
    matches!(
        kind,
        ClientRequestKind::ListTelemetry { .. }
            | ClientRequestKind::CostReport { .. }
            | ClientRequestKind::ListJobs { .. }
            | ClientRequestKind::ListModels
            | ClientRequestKind::ListCatalog
            | ClientRequestKind::ListSkills
            | ClientRequestKind::ListMcp
            | ClientRequestKind::ListTools { .. }
            | ClientRequestKind::ListPlans { .. }
            | ClientRequestKind::ListSessions
    )
}

pub(crate) fn is_plugin_request(kind: &ClientRequestKind) -> bool {
    matches!(
        kind,
        ClientRequestKind::ListPlugins
            | ClientRequestKind::InstallPlugin { .. }
            | ClientRequestKind::UpdatePlugins { .. }
            | ClientRequestKind::RemovePlugin { .. }
            | ClientRequestKind::SetPluginEnabled { .. }
    )
}

/// The event a plugin step reports on the session's stream.
pub(crate) fn plugin_progress_event(
    operation: &str,
    step: usize,
    total: usize,
    stage: &str,
) -> norte_code_wire::ServerEvent {
    norte_code_wire::ServerEvent::new(
        0,
        None,
        norte_code_wire::ServerEventKind::PluginProgress {
            operation: operation.to_owned(),
            step: u64::try_from(step).unwrap_or(0),
            total: u64::try_from(total).unwrap_or(0),
            stage: stage.to_owned(),
        },
    )
}

pub(crate) fn standard(kind: StandardFrameType) -> FrameType {
    FrameType::Standard(kind)
}

pub(crate) fn observer_read_only_frame(kind: &FrameType) -> bool {
    matches!(
        kind,
        FrameType::Standard(
            StandardFrameType::Ack
                | StandardFrameType::HistoryGet
                | StandardFrameType::JobList
                | StandardFrameType::JobGet
        )
    )
}

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
