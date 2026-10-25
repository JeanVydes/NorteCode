//! Background jobs: a shell call that is not waited for.
//!
//! Every `shell` call is a job. When the model asks to wait — the default — the
//! job runs and its result is the tool result, exactly as before. When it does
//! not, the call answers immediately with the job's id and the job carries on
//! in the background, reporting its output as `EngineEvent::ToolOutput` like any
//! other call.
//!
//! The registry is what makes the difference visible: it is where a job's
//! output is accumulated while it runs, so `manage_jobs` can read it, and where
//! its cancellation token lives, so `/jobs stop` can end it. Nothing else needs
//! to know whether a call was waited for.
//!
//! The job ends when its output ends. There is no unbounded job: the shell
//! executor applies the request's timeout either way, so a detached call is
//! killed at the same bound a waited one is.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use norte::{CallId, Cancellation, SessionId};

/// How much of a job's output is kept for `manage_jobs` to read.
///
/// The executor already bounds what reaches the model; this bounds what the
/// registry holds on top of that, so a chatty job cannot grow without limit
/// while nobody is looking at it.
pub(super) const JOB_BUFFER_BYTES: usize = 64 * 1024;

/// Where a job stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JobState {
    /// Running, or waiting to be read.
    Running,
    /// Ended on its own.
    Finished,
    /// Ended because someone asked it to.
    Cancelled,
}

impl JobState {
    /// The word the model and the CLI read.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether the job is over.
    pub(super) fn is_done(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// What a job looks like from outside.
#[derive(Clone, Debug)]
pub(super) struct JobSummary {
    /// The id the model was given.
    pub(super) id: String,
    /// The session it belongs to.
    pub(super) session: SessionId,
    /// The call it came from, so its output can be found among the events.
    pub(super) call_id: CallId,
    /// The script, clipped to one line for a listing.
    pub(super) script: String,
    /// Where it stands.
    pub(super) state: JobState,
    /// The exit code, once it ended on its own.
    pub(super) exit_code: Option<i32>,
    /// Whether it ended because its timeout expired.
    pub(super) timed_out: bool,
    /// Milliseconds since it started.
    pub(super) elapsed_ms: u64,
    /// The most recent output, newest last.
    pub(super) output: String,
    /// Whether older output was dropped to stay within the buffer.
    pub(super) dropped: bool,
}

/// One job, as the registry holds it.
struct Job {
    summary: JobSummary,
    cancel: Arc<Cancellation>,
    started: std::time::Instant,
    bytes: usize,
}

/// The jobs a service knows about, newest last.
#[derive(Default)]
pub(super) struct JobRegistry {
    jobs: Mutex<BTreeMap<String, Job>>,
    next: AtomicU64,
}

impl JobRegistry {
    /// Registers a job that has just been started, and returns the id to hand
    /// the model.
    ///
    /// The cancellation token is the caller's: cancelling it is what stops the
    /// call, and the registry only holds it so `/jobs stop` can reach it.
    pub(super) fn start(
        &self,
        session: SessionId,
        call_id: CallId,
        script: &str,
        cancel: Arc<Cancellation>,
    ) -> String {
        let number = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("job-{number}");
        let summary = JobSummary {
            id: id.clone(),
            session,
            call_id,
            script: one_line(script),
            state: JobState::Running,
            exit_code: None,
            timed_out: false,
            elapsed_ms: 0,
            output: String::new(),
            dropped: false,
        };
        self.jobs().insert(
            id.clone(),
            Job {
                summary,
                cancel,
                started: std::time::Instant::now(),
                bytes: 0,
            },
        );
        id
    }

    /// Appends a chunk of a call's output to its job, when it is one.
    ///
    /// Chunks arrive for every tool call, so this is a no-op for a call that is
    /// not a job — which is what keeps the registry out of the way of the
    /// waited path.
    pub(super) fn write(&self, call_id: CallId, text: &str) {
        let mut jobs = self.jobs();
        let Some(job) = jobs
            .values_mut()
            .find(|job| job.summary.call_id == call_id && !job.summary.state.is_done())
        else {
            return;
        };
        job.summary.output.push_str(text);
        job.bytes += text.len();
        // Keep the tail: the most recent output is what a reader wants.
        if job.bytes > JOB_BUFFER_BYTES {
            let overflow = job.bytes - JOB_BUFFER_BYTES;
            let drop_to = job
                .summary
                .output
                .char_indices()
                .map(|(index, _)| index)
                .find(|index| *index >= overflow)
                .unwrap_or(0);
            job.summary.output.drain(..drop_to);
            job.bytes -= drop_to;
            job.summary.dropped = true;
        }
    }

    /// Records that a call ended, unless it was cancelled first.
    ///
    /// A job cancelled through the registry keeps that state: the call ending is
    /// the consequence, not a new fact about it.
    pub(super) fn finish(&self, call_id: CallId, exit_code: Option<i32>) {
        let mut jobs = self.jobs();
        let Some(job) = jobs.values_mut().find(|job| job.summary.call_id == call_id) else {
            return;
        };
        job.summary.elapsed_ms = millis(job.started.elapsed());
        if job.summary.state == JobState::Cancelled {
            return;
        }
        job.summary.state = JobState::Finished;
        job.summary.exit_code = exit_code;
    }

    /// Marks a job cancelled and signals its token. Answers whether it was one.
    pub(super) fn cancel(&self, id: &str) -> bool {
        let mut jobs = self.jobs();
        let Some(job) = jobs.get_mut(id) else {
            return false;
        };
        if job.summary.state.is_done() {
            return false;
        }
        job.summary.state = JobState::Cancelled;
        job.cancel.cancel();
        true
    }

    /// One job, by id.
    pub(super) fn get(&self, id: &str) -> Option<JobSummary> {
        let mut jobs = self.jobs();
        jobs.get_mut(id).map(|job| {
            job.summary.elapsed_ms = millis(job.started.elapsed());
            job.summary.clone()
        })
    }

    /// The jobs of one session, oldest first. `None` means every session.
    pub(super) fn list(&self, session: Option<SessionId>) -> Vec<JobSummary> {
        let mut jobs = self.jobs();
        let mut out: Vec<JobSummary> = jobs
            .values_mut()
            .filter(|job| session.is_none_or(|session| job.summary.session == session))
            .map(|job| {
                job.summary.elapsed_ms = millis(job.started.elapsed());
                job.summary.clone()
            })
            .collect();
        out.sort_by_key(|job| job.id.clone());
        out
    }

    fn jobs(&self) -> MutexGuard<'_, BTreeMap<String, Job>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The script as one line, for a listing that has one line per job.
fn one_line(script: &str) -> String {
    let mut line: String = script.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 80 {
        line = line.chars().take(77).collect::<String>() + "...";
    }
    line
}
