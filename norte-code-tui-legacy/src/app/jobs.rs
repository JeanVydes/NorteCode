//! The session's background jobs.
//!
//! A job is a tool call the host let run on its own. The reader follows it
//! while it runs and reads how it ended in the transcript.

use norte_code_wire::{JobInfo, JobState, ServerEventKind};

use super::App;
use crate::ui_model::TranscriptItem;

/// How much output a job keeps.
const OUTPUT_LIMIT: usize = 4_000;

/// The job a start event describes.
pub(super) fn started_job(job_id: &str, command: &str) -> JobInfo {
    JobInfo {
        id: job_id.to_owned(),
        label: command.to_owned(),
        script: command.to_owned(),
        state: JobState::Running,
        exit_code: None,
        timed_out: false,
        elapsed_ms: 0,
        output: String::new(),
        dropped: false,
        output_bytes_total: 0,
    }
}

/// Puts a job in the list, replacing the one it finishes.
pub(super) fn replace_job(jobs: &mut Vec<JobInfo>, job: JobInfo) {
    match jobs.iter().position(|existing| existing.id == job.id) {
        Some(index) => jobs[index] = job,
        None => jobs.push(job),
    }
}

/// Adds what a job wrote to what it wrote before.
pub(super) fn append_job_output(jobs: &mut [JobInfo], job_id: &str, delta: &str) {
    let Some(job) = jobs.iter_mut().find(|job| job.id == job_id) else {
        return;
    };
    job.output.push_str(delta);
    if job.output.len() > OUTPUT_LIMIT {
        let start = job.output.len() - OUTPUT_LIMIT;
        job.output = job.output[start..].to_owned();
        job.dropped = true;
    }
}

/// What the transcript says when a job ends.
pub(super) fn job_finished_message(job: &JobInfo) -> String {
    let name = if job.label.is_empty() {
        job.id.as_str()
    } else {
        job.label.as_str()
    };
    let state = match job.state {
        JobState::Running => "still running",
        JobState::Finished => "finished",
        JobState::Cancelled => "stopped",
    };
    let exit = job
        .exit_code
        .map(|code| format!(" (exit {code})"))
        .unwrap_or_default();
    format!("Job {state}: {name}{exit} · {} ms", job.elapsed_ms)
}

impl App {
    /// Replaces the jobs the host listed.
    pub(super) fn apply_job_list(&mut self, jobs: Vec<JobInfo>) {
        self.model.jobs = jobs;
    }

    /// Records a job that ended.
    pub(super) fn apply_finished_job(&mut self, job: &JobInfo) {
        replace_job(&mut self.model.jobs, job.clone());
        self.model
            .transcript
            .push(TranscriptItem::Status(job_finished_message(job)));
    }

    /// Follows the jobs the host reports.
    pub(super) fn track_job(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::JobStarted {
                job_id, command, ..
            } => {
                replace_job(&mut self.model.jobs, started_job(job_id, command));
                self.model
                    .transcript
                    .push(TranscriptItem::Status(format!("Job started: {command}")));
            }
            ServerEventKind::JobProgressDelta { job_id, delta, .. } => {
                append_job_output(&mut self.model.jobs, job_id, delta);
            }
            ServerEventKind::JobFinished { job, .. } => {
                replace_job(&mut self.model.jobs, job.clone());
                self.model
                    .transcript
                    .push(TranscriptItem::Status(job_finished_message(job)));
            }
            ServerEventKind::JobList { jobs } => {
                self.model.jobs.clone_from(jobs);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
