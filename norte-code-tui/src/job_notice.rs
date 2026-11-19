//! The transcript notices a background job produces.

use norte_code_wire::{JobInfo, JobState};

use crate::ui::Tone;

/// What the transcript says when a call is detached into a background job.
pub(crate) fn started(job_id: &str, command: &str) -> String {
    format!("job {job_id} started in background · {command}")
}

/// What the transcript says when a job reaches a terminal state.
pub(crate) fn finished(job: &JobInfo) -> String {
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
        .map(|code| format!(" · exit {code}"))
        .unwrap_or_default();
    format!("job {state} · {name}{exit} · {} ms", job.elapsed_ms)
}

/// How loud the finish notice should be.
pub(crate) fn tone(job: &JobInfo) -> Tone {
    match job.state {
        JobState::Cancelled => Tone::Warn,
        JobState::Finished if job.exit_code.is_some_and(|code| code != 0) => Tone::Error,
        JobState::Finished | JobState::Running => Tone::Neutral,
    }
}

#[cfg(test)]
#[path = "job_notice_tests.rs"]
mod tests;
