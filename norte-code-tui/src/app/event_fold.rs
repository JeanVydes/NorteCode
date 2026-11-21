//! Event folding: server and client events become model and tree state.

#[path = "event_fold/client.rs"]
mod client;
#[path = "event_fold/errors.rs"]
mod errors;
#[path = "event_fold/history.rs"]
mod history;
#[path = "event_fold/jobs.rs"]
mod jobs;
#[path = "event_fold/runtime.rs"]
mod runtime;
#[path = "event_fold/stream.rs"]
mod stream;

use norte_code_wire::ServerEventKind;

use crate::attachment::{AttachmentNotice, NoticeTone};
use crate::client::UiCommand;
use crate::ui;
use crate::ui_model::UiModel;

pub(in crate::app) use client::drain_server_events;
#[cfg(test)]
pub(in crate::app) use client::fold_upload_event;

use errors::fold_error_event;
use runtime::fold_runtime_event;
use stream::{fold_tool_event, fold_turn_event};

pub(in crate::app) fn fold(
    event: &norte_code_wire::ServerEvent,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
) {
    if fold_error_event(&event.kind, tree, model) {
        return;
    }
    match &event.kind {
        kind @ (ServerEventKind::TurnStarted { .. }
        | ServerEventKind::InputAccepted { .. }
        | ServerEventKind::TurnPhase { .. }
        | ServerEventKind::QuestionAnswered { .. }
        | ServerEventKind::TurnFinished { .. }
        | ServerEventKind::TurnStopped { .. }) => {
            fold_turn_event(kind, tree, model, commands);
        }
        kind @ (ServerEventKind::ToolStarted { .. }
        | ServerEventKind::ToolOutput { .. }
        | ServerEventKind::ToolFinished { .. }
        | ServerEventKind::Delta { .. }) => {
            fold_tool_event(kind, tree, model, commands);
        }
        kind @ (ServerEventKind::PluginProgress { .. }
        | ServerEventKind::PluginChanged { .. }
        | ServerEventKind::PlanList { .. }
        | ServerEventKind::JobList { .. }
        | ServerEventKind::JobStarted { .. }
        | ServerEventKind::JobFinished { .. }
        | ServerEventKind::SessionStats { .. }
        | ServerEventKind::RequestReconstructed { .. }
        | ServerEventKind::CompactionStarted { .. }
        | ServerEventKind::CompactionFinished { .. }) => {
            fold_runtime_event(kind, tree, model);
        }
        ServerEventKind::SessionTodos { todos } => {
            model.todos.clone_from(todos);
        }
        ServerEventKind::Error { .. }
        | ServerEventKind::BudgetExceeded { .. }
        | ServerEventKind::BudgetWarning { .. }
        | ServerEventKind::QuestionAsked { .. }
        | ServerEventKind::ModelQueued { .. }
        | ServerEventKind::ModelChanged { .. }
        | ServerEventKind::RetryScheduled { .. }
        | ServerEventKind::RetryStarted { .. }
        | ServerEventKind::ReasoningDowngraded { .. }
        | ServerEventKind::SessionCreated { .. }
        | ServerEventKind::SessionList { .. }
        | ServerEventKind::SessionDeleted
        | ServerEventKind::SessionRenamed { .. }
        | ServerEventKind::SessionForked { .. }
        | ServerEventKind::ChildSessionStarted { .. }
        | ServerEventKind::ChildSessionUpdated { .. }
        | ServerEventKind::MediaPartCompleted { .. }
        | ServerEventKind::Committed { .. }
        | ServerEventKind::MessageReplayed { .. }
        | ServerEventKind::ImportResult { .. }
        | ServerEventKind::SessionExportReady { .. }
        | ServerEventKind::ModelList { .. }
        | ServerEventKind::ToolList { .. }
        | ServerEventKind::PluginList { .. }
        | ServerEventKind::McpList { .. }
        | ServerEventKind::SkillList { .. }
        | ServerEventKind::SkillsUpdated { .. }
        | ServerEventKind::Telemetry { .. }
        | ServerEventKind::CostUpdated { .. }
        | ServerEventKind::CostNotReported { .. }
        | ServerEventKind::BudgetChanged { .. }
        | ServerEventKind::CatalogUpdated { .. }
        | ServerEventKind::CatalogDetail { .. }
        | ServerEventKind::SteerApplied { .. }
        | ServerEventKind::JobProgressDelta { .. }
        | ServerEventKind::JobsStopped { .. }
        | ServerEventKind::DelegatorStateChanged { .. }
        | ServerEventKind::ChildAwaitingInput { .. }
        | ServerEventKind::HistoryPage { .. }
        | ServerEventKind::HistoryRewound { .. }
        | ServerEventKind::OutputPartStarted { .. }
        | ServerEventKind::OutputPartFinished { .. }
        | ServerEventKind::SecretSaved { .. } => {}
    }
}

pub(in crate::app) fn push_attachment_notice(
    tree: &mut terminalui::Tree,
    notice: &AttachmentNotice,
) {
    let tone = match notice.tone() {
        NoticeTone::Warn => ui::Tone::Warn,
        NoticeTone::Error => ui::Tone::Error,
    };
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::Notice {
            text: notice.text(),
            tone,
        });
    }
}

pub(in crate::app) fn notice(tree: &mut terminalui::Tree, text: String, tone: ui::Tone) -> bool {
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::Notice { text, tone });
    }
    true
}

pub(in crate::app) fn toast(tree: &mut terminalui::Tree, text: &str, tone: ui::Tone) {
    use terminalui::components::toast::ToastKind;

    let kind = match tone {
        ui::Tone::Neutral => ToastKind::Info,
        ui::Tone::Warn => ToastKind::Warning,
        ui::Tone::Error => ToastKind::Destructive,
    };
    if let Some(host) = tree.find_mut::<ui::ToastHost>("toast") {
        host.show(kind, text.to_owned());
    }
}
