//! Background jobs, context usage, and compaction notices.

use terminalui::components::button::Button;

use crate::compact_notice;
use crate::context::ContextUsage;
use crate::ui;
use crate::ui_model::UiModel;

use super::{notice, toast};

pub(in crate::app) fn fold_job_started(
    job_id: &str,
    command: &str,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) {
    upsert_job(
        model,
        norte_code_wire::JobSummary {
            job_id: job_id.to_owned(),
            command: command.to_owned(),
            ..Default::default()
        },
    );
    notice(
        tree,
        crate::job_notice::started(job_id, command),
        ui::Tone::Neutral,
    );
}

pub(in crate::app) fn fold_job_finished(
    job: &norte_code_wire::JobInfo,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) {
    upsert_job(model, job_summary(job));
    let text = crate::job_notice::finished(job);
    let tone = crate::job_notice::tone(job);
    toast(tree, &text, tone);
    notice(tree, text, tone);
}

fn upsert_job(model: &mut UiModel, job: norte_code_wire::JobSummary) {
    model.jobs.retain(|existing| existing.job_id != job.job_id);
    model.jobs.insert(0, job);
}

pub(in crate::app) fn job_summary(info: &norte_code_wire::JobInfo) -> norte_code_wire::JobSummary {
    use norte_code_wire::{JobState, JobStatus};

    let status = if info.timed_out {
        JobStatus::TimedOut
    } else {
        match info.state {
            JobState::Running => JobStatus::Running,
            JobState::Finished => match info.exit_code {
                Some(code) if code != 0 => JobStatus::Failed,
                _ => JobStatus::Completed,
            },
            JobState::Cancelled => JobStatus::Killed,
        }
    };
    norte_code_wire::JobSummary {
        job_id: info.id.clone(),
        command: if info.label.is_empty() {
            info.script.clone()
        } else {
            info.label.clone()
        },
        status,
        exit_code: info.exit_code,
        output_snapshot: (!info.output.is_empty()).then(|| info.output.clone()),
        output_bytes_total: info.output_bytes_total,
        output_truncated: info.dropped,
        duration_ms: Some(info.elapsed_ms),
    }
}

fn set_compact_loading(tree: &mut terminalui::Tree, loading: bool) {
    if let Some(button) = tree.find_mut::<Button>("compact") {
        button.set_loading(loading);
    }
}

pub(in crate::app) fn fold_session_stats(
    context_used: u64,
    context_window: Option<u64>,
    model: &mut UiModel,
) {
    model.context = ContextUsage {
        used: context_used,
        window: context_window,
    };
}

pub(in crate::app) fn fold_request_usage(usage: &norte_code_wire::Usage, model: &mut UiModel) {
    let spent = usage
        .input_tokens
        .unwrap_or(0)
        .saturating_add(usage.output_tokens.unwrap_or(0));
    if spent > 0 {
        model.context.used = spent;
    }
}

pub(in crate::app) fn fold_compaction_started(
    reason: norte_code_wire::CompactionReason,
    tree: &mut terminalui::Tree,
) {
    notice(tree, compact_notice::started(reason), ui::Tone::Neutral);
    set_compact_loading(tree, true);
}

pub(in crate::app) fn fold_compaction_finished(
    compacted: bool,
    summary: Option<&str>,
    error: Option<&str>,
    tree: &mut terminalui::Tree,
) {
    let (text, tone) = compact_notice::finished(compacted, summary, error);
    if let Some(dock) = tree.find_mut::<ui::CommandDock>("command_dock") {
        dock.finish_progress(if error.is_some() {
            "Compaction failed"
        } else if compacted {
            "Context compacted"
        } else {
            "Nothing to compact"
        });
    }
    toast(tree, &text, tone);
    notice(tree, text, tone);
    set_compact_loading(tree, false);
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
