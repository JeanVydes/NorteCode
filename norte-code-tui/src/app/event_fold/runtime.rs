//! Runtime state: plugins, plans, jobs, stats, and compaction events.

use norte_code_wire::ServerEventKind;

use crate::ui_model::UiModel;

use super::jobs::{
    fold_compaction_finished, fold_compaction_started, fold_job_finished, fold_job_started,
    fold_request_usage, fold_session_stats, job_summary,
};

pub(in crate::app) fn fold_runtime_event(
    kind: &ServerEventKind,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) {
    if let ServerEventKind::PluginProgress {
        operation,
        step,
        total,
        stage,
    } = kind
    {
        model.plugin_job = Some(crate::ui_model::PluginJob {
            operation: operation.clone(),
            step: *step,
            total: *total,
            stage: stage.clone(),
        });
        return;
    }
    if let ServerEventKind::PluginChanged { .. } = kind {
        model.plugin_job = None;
        return;
    }
    if let ServerEventKind::PlanList { plans } = kind {
        model.plans.clone_from(plans);
        return;
    }
    if let ServerEventKind::JobList { jobs } = kind {
        model.jobs = jobs.iter().map(job_summary).collect();
        return;
    }
    if let ServerEventKind::JobStarted {
        job_id, command, ..
    } = kind
    {
        fold_job_started(job_id, command, tree, model);
        return;
    }
    if let ServerEventKind::JobFinished { job, .. } = kind {
        fold_job_finished(job, tree, model);
        return;
    }
    if let ServerEventKind::SessionStats {
        context_used,
        context_window,
        ..
    } = kind
    {
        fold_session_stats(*context_used, *context_window, model);
        return;
    }
    if let ServerEventKind::RequestReconstructed { usage, .. } = kind {
        fold_request_usage(usage, model);
        return;
    }
    if let ServerEventKind::CompactionStarted { reason } = kind {
        fold_compaction_started(*reason, tree);
        return;
    }
    if let ServerEventKind::CompactionFinished {
        compacted,
        summary,
        error,
        ..
    } = kind
    {
        fold_compaction_finished(*compacted, summary.as_deref(), error.as_deref(), tree);
    }
}
