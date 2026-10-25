//! Background jobs: a shell call that is not waited for.
//!
//! Every `shell_bg` call is a job. Its call answers at once with the job's id,
//! and the job carries on in the background, reporting its output as
//! `EngineEvent::ToolOutput` like any other call.
//!
//! The registry is what makes the difference visible: it is where a job's
//! output is accumulated while it runs, so `manage_jobs` can read it, and where
//! its cancellation token lives, so `Ctrl-C` and `/jobs stop` can end it.
//! Nothing else needs to know whether a call was waited for.
//!
//! Only `shell_bg` registers here. A waited `shell` call is its own result and
//! has no id to be reached by, so putting it in the registry would list a job
//! nobody can name. That is what makes the end of a job unambiguous: it is the
//! closing of its streams, never the finishing of its call, which for a call
//! that answered at once happens at the very start.
//!
//! The job ends when its process does. There is no unbounded job: the shell
//! executor applies the request's timeout either way, so a background call is
//! killed at the same bound a waited one is.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use norte::{CallId, Cancellation, SessionId};
use norte_code_protocol::JobInfo;

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

/// The jobs a service knows about, oldest last.
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

    /// Marks a job's streams as closed, which is the process exiting.
    ///
    /// A stream reports `last` when its pipe closes, and the closing chunk
    /// always arrives, even when the process wrote nothing. It is the one end a
    /// job cannot miss: the call that started it answered at once, so its own
    /// finish says nothing about the process, and a job nobody is holding has
    /// no other task to report it.
    pub(super) fn close(&self, call_id: CallId, last: bool) {
        if !last {
            return;
        }
        let mut jobs = self.jobs();
        let Some(job) = jobs
            .values_mut()
            .find(|job| job.summary.call_id == call_id && !job.summary.state.is_done())
        else {
            return;
        };
        job.summary.elapsed_ms = millis(job.started.elapsed());
        job.summary.state = JobState::Finished;
    }

    /// Records the exit code a job's process reported.
    ///
    /// The code arrives with the result, which the holder of the process reads
    /// after its streams have closed, so it lands on a job that is already
    /// `Finished`. That is why this only records the code rather than also
    /// deciding the state — except for a job that was cancelled, where the code
    /// is the consequence of the cancellation and not a fact about it.
    pub(super) fn settle(&self, id: &str, exit_code: Option<i32>, timed_out: bool) {
        let mut jobs = self.jobs();
        let Some(job) = jobs.get_mut(id) else {
            return;
        };
        job.summary.elapsed_ms = millis(job.started.elapsed());
        job.summary.timed_out = timed_out;
        if job.summary.state == JobState::Cancelled {
            return;
        }
        if !job.summary.state.is_done() {
            job.summary.state = JobState::Finished;
        }
        if let Some(code) = exit_code {
            job.summary.exit_code = Some(code);
        }
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

    /// Stops every running job of one session, and answers how many it stopped.
    ///
    /// It is what `Ctrl-C` asks for, and what `/jobs stop` with no id asks for:
    /// the jobs of the work in hand, not every job the service holds.
    pub(super) fn cancel_session(&self, session: SessionId) -> usize {
        let mut stopped = 0;
        for id in self.running_of(session) {
            stopped += usize::from(self.cancel(&id));
        }
        stopped
    }

    /// Whether any job of the session is still running.
    pub(super) fn any_running(&self, session: SessionId) -> bool {
        !self.running_of(session).is_empty()
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

    /// The ids of the session's running jobs, oldest first.
    fn running_of(&self, session: SessionId) -> Vec<String> {
        let jobs = self.jobs();
        jobs.values()
            .filter(|job| job.summary.session == session && !job.summary.state.is_done())
            .map(|job| job.summary.id.clone())
            .collect()
    }

    fn jobs(&self) -> MutexGuard<'_, BTreeMap<String, Job>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The registry's view of a job, as the wire carries it.
pub(super) fn job_info(job: &JobSummary) -> JobInfo {
    JobInfo {
        id: job.id.clone(),
        script: job.script.clone(),
        state: job.state.label().to_owned(),
        exit_code: job.exit_code,
        timed_out: job.timed_out,
        elapsed_ms: job.elapsed_ms,
        output: job.output.clone(),
        dropped: job.dropped,
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
