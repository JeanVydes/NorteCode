//! Background jobs: work that is not waited for.
//!
//! A call is a job when nobody is left waiting for it, which happens two ways.
//! It asked to be detached — `detach: true`, or a `Ctrl-G` on a call that was
//! being waited for — and so answered before it had anything to show. A call
//! nobody detached is not a job at all, and its result is its own answer.
//!
//! A job is whatever work can be left running: a shell process, a subagent's
//! session converting. The registry does not know which. It is where a job's
//! output is accumulated while it runs, so `manage_jobs` can read it, and
//! where its cancellation token lives, so `Ctrl-C` and `/jobs stop` can end
//! it. Nothing else needs to know whether a call was waited for.
//!
//! That is what makes the end of a job unambiguous: it ends when its runner
//! settles, never when the call that started it finished — for a call that let
//! go happens at the very start. For shell jobs, stream closure and the runner's
//! terminal result together provide that settlement.
//!
//! There is no unbounded job: the executor that runs the work applies its
//! own bound either way (a shell timeout, a subagent's max iterations), so
//! a background job is killed at the same bound a waited one is.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use norte::{CallId, Cancellation, SessionId};
use norte_code_protocol::JobInfo;

/// How much of a job's output is kept for `manage_jobs` to read.
///
/// The executor already bounds what reaches the model; this bounds what the
/// registry holds on top of that, so a chatty job cannot grow without limit
/// while nobody is looking at it.
pub(super) const JOB_BUFFER_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_CONCURRENT_JOBS: usize = 8;
const DEFAULT_JOB_OUTPUT_RETENTION_S: u64 = 86_400;

#[derive(Debug, thiserror::Error)]
#[error("SANDBOX_LIMIT_EXCEEDED: maximum concurrent jobs per session is {limit}")]
pub(super) struct JobStartError {
    limit: usize,
}

/// Where a job stands, as the wire names it.
///
/// It is the protocol's own enum and not a copy: the service writes it into
/// every `JobInfo` it hands out, and a second type here would be a second
/// spelling of the same three words — which is what the string was.
pub(super) use norte_code_protocol::JobState;

/// What a job looks like from outside.
#[derive(Clone, Debug)]
pub(super) struct JobSummary {
    /// The id the model was given.
    pub(super) id: String,
    /// The session it belongs to.
    pub(super) session: SessionId,
    /// The call it came from, so its output can be found among the events.
    pub(super) call_id: CallId,
    /// The label, clipped to one line for a listing.
    ///
    /// A shell's label is its script; a subagent's is its title. The registry
    /// does not know which: it is the one line a listing has for the job.
    pub(super) label: String,
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
    /// Total bytes written, including output no longer retained.
    pub(super) output_bytes_total: u64,
}

/// One job, as the registry holds it.
struct Job {
    summary: JobSummary,
    cancel: Arc<Cancellation>,
    started: std::time::Instant,
    finished: Option<std::time::Instant>,
    bytes: usize,
    terminal: bool,
    terminal_tx: tokio::sync::watch::Sender<bool>,
}

impl Job {
    /// Records how long it has run, if it is still running.
    ///
    /// A job that has ended keeps the time its end recorded. Without this a
    /// finished job reported the time since it *started*, which grew every
    /// time it was read: a build that took eight seconds read as seventeen,
    /// then twenty-nine, for as long as anyone asked about it.
    fn refresh_elapsed(&mut self) {
        if !self.summary.state.is_done() {
            self.summary.elapsed_ms = millis(self.started.elapsed());
        }
    }
}

/// The jobs a service knows about, oldest last.
pub(super) struct JobRegistry {
    jobs: Mutex<BTreeMap<String, Job>>,
    next: AtomicU64,
    max_concurrent: AtomicUsize,
    output_retention_s: AtomicU64,
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(0),
            max_concurrent: AtomicUsize::new(DEFAULT_MAX_CONCURRENT_JOBS),
            output_retention_s: AtomicU64::new(DEFAULT_JOB_OUTPUT_RETENTION_S),
        }
    }
}

impl JobRegistry {
    /// Applies the server's concurrent-job and output-retention limits.
    pub(super) fn configure_limits(&self, max_concurrent: usize, output_retention_s: u64) {
        self.max_concurrent.store(max_concurrent, Ordering::Relaxed);
        self.output_retention_s
            .store(output_retention_s, Ordering::Relaxed);
    }

    /// Registers a job that has just been started, and returns the id to hand
    /// the model.
    ///
    /// The cancellation token is the caller's: cancelling it is what stops the
    /// call, and the registry only holds it so `/jobs stop` can reach it.
    pub(super) fn try_start(
        &self,
        session: SessionId,
        call_id: CallId,
        label: &str,
        cancel: Arc<Cancellation>,
    ) -> Result<String, JobStartError> {
        let mut jobs = self.jobs();
        let limit = self.max_concurrent.load(Ordering::Relaxed);
        let running = jobs
            .values()
            .filter(|job| job.summary.session == session && !job.terminal)
            .count();
        if running >= limit {
            return Err(JobStartError { limit });
        }
        let number = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("job-{number}");
        let summary = JobSummary {
            id: id.clone(),
            session,
            call_id,
            label: one_line(label),
            state: JobState::Running,
            exit_code: None,
            timed_out: false,
            elapsed_ms: 0,
            output: String::new(),
            dropped: false,
            output_bytes_total: 0,
        };
        let (terminal_tx, _) = tokio::sync::watch::channel(false);
        jobs.insert(
            id.clone(),
            Job {
                summary,
                cancel,
                started: std::time::Instant::now(),
                finished: None,
                bytes: 0,
                terminal: false,
                terminal_tx,
            },
        );
        Ok(id)
    }

    #[cfg(test)]
    pub(super) fn start(
        &self,
        session: SessionId,
        call_id: CallId,
        label: &str,
        cancel: Arc<Cancellation>,
    ) -> String {
        self.try_start(session, call_id, label, cancel)
            .expect("test job is within the default limit")
    }

    /// Appends a chunk of a call's output to its job, when it is one.
    ///
    /// Chunks arrive for every tool call, so this is a no-op for a call that is
    /// not a job — which is what keeps the registry out of the way of the
    /// waited path. Callers that hold no call id (a subagent job, whose
    /// session is the child's) use [`JobRegistry::write_id`].
    /// Writers that do hold the job's id use that directly.
    pub(super) fn write(&self, call_id: CallId, text: &str) -> Option<u64> {
        let mut jobs = self.jobs();
        let job = jobs
            .values_mut()
            .find(|job| job.summary.call_id == call_id && !job.terminal)?;
        let index = job.summary.output_bytes_total;
        append_output(job, text);
        Some(index)
    }

    /// Appends to a job by its id.
    ///
    /// For writers without streams, whose job they hold by the id they were
    /// handed: a subagent writes its final message when the child's turn
    /// settles, not a stream of chunks.
    pub(super) fn write_id(&self, id: &str, text: &str) {
        let mut jobs = self.jobs();
        let Some(job) = jobs.get_mut(id) else {
            return;
        };
        if job.terminal {
            return;
        }
        append_output(job, text);
    }

    /// Marks a job as having ended by id, recording its exit code when it has one.
    ///
    /// For writers without streams: the child turn settled, and the job is
    /// over. A shell's `close` does the same thing from a stream close.
    pub(super) fn finish(&self, id: &str, exit_code: Option<i32>) {
        let mut jobs = self.jobs();
        let Some(job) = jobs.get_mut(id) else {
            return;
        };
        if job.terminal {
            return;
        }
        job.summary.elapsed_ms = millis(job.started.elapsed());
        if job.summary.state != JobState::Cancelled {
            job.summary.state = JobState::Finished;
        }
        if job.summary.state != JobState::Cancelled
            && let Some(code) = exit_code
        {
            job.summary.exit_code = Some(code);
        }
        job.terminal = true;
        job.finished = Some(std::time::Instant::now());
        job.terminal_tx.send_replace(true);
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
        if !job.terminal && job.summary.state != JobState::Cancelled {
            job.summary.state = JobState::Finished;
        }
        if job.summary.state != JobState::Cancelled
            && let Some(code) = exit_code
        {
            job.summary.exit_code = Some(code);
        }
        job.terminal = true;
        job.finished = Some(std::time::Instant::now());
        job.terminal_tx.send_replace(true);
    }

    /// Marks a job cancelled and signals its token. Answers whether it was one.
    pub(super) fn cancel(&self, id: &str) -> bool {
        self.cancel_with_grace(id, None)
    }

    pub(super) fn cancel_with_grace(&self, id: &str, grace: Option<Duration>) -> bool {
        let mut jobs = self.jobs();
        let Some(job) = jobs.get_mut(id) else {
            return false;
        };
        if job.summary.state.is_done() {
            return false;
        }
        job.summary.state = JobState::Cancelled;
        Self::signal_cancel(Arc::clone(&job.cancel), grace);
        true
    }

    /// Cancels a job only when it belongs to the supplied session.
    pub(super) fn cancel_in_session(&self, session: SessionId, id: &str) -> bool {
        self.cancel_in_session_with_grace(session, id, None)
    }

    pub(super) fn cancel_in_session_with_grace(
        &self,
        session: SessionId,
        id: &str,
        grace: Option<Duration>,
    ) -> bool {
        let mut jobs = self.jobs();
        let Some(job) = jobs.get_mut(id) else {
            return false;
        };
        if job.summary.session != session || job.summary.state.is_done() {
            return false;
        }
        job.summary.state = JobState::Cancelled;
        Self::signal_cancel(Arc::clone(&job.cancel), grace);
        true
    }

    fn signal_cancel(cancel: Arc<Cancellation>, grace: Option<Duration>) {
        match grace.filter(|duration| !duration.is_zero()) {
            Some(grace) => {
                tokio::spawn(async move {
                    tokio::time::sleep(grace).await;
                    cancel.cancel();
                });
            }
            None => cancel.cancel(),
        }
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
            job.refresh_elapsed();
            job.summary.clone()
        })
    }

    /// One job, only when it belongs to the supplied session.
    pub(super) fn get_for_session(&self, session: SessionId, id: &str) -> Option<JobSummary> {
        let job = self.get(id)?;
        (job.session == session).then_some(job)
    }

    /// One job, applying the RFC output-retention window before returning it.
    pub(super) fn get_for_session_with_retention(
        &self,
        session: SessionId,
        id: &str,
    ) -> Option<JobSummary> {
        let mut jobs = self.jobs();
        let job = jobs.get_mut(id)?;
        if job.summary.session != session {
            return None;
        }
        expire_output(
            job,
            self.output_retention_s.load(Ordering::Relaxed),
            std::time::Instant::now(),
        );
        job.refresh_elapsed();
        Some(job.summary.clone())
    }

    /// Subscribes to the actual process/runner termination of a job.
    ///
    /// Cancellation marks the job as killed when requested, but the terminal
    /// result is not ready until the runner has closed its streams or settled.
    pub(super) fn terminal_for_session(
        &self,
        session: SessionId,
        id: &str,
    ) -> Option<tokio::sync::watch::Receiver<bool>> {
        self.jobs()
            .get(id)
            .filter(|job| job.summary.session == session)
            .map(|job| job.terminal_tx.subscribe())
    }

    /// The jobs of one session, oldest first. `None` means every session.
    pub(super) fn list(&self, session: Option<SessionId>) -> Vec<JobSummary> {
        let mut jobs = self.jobs();
        let retention_s = self.output_retention_s.load(Ordering::Relaxed);
        let now = std::time::Instant::now();
        let mut out = Vec::new();
        for job in jobs.values_mut() {
            if session.is_some_and(|session| job.summary.session != session) {
                continue;
            }
            expire_output(job, retention_s, now);
            job.refresh_elapsed();
            out.push(job.summary.clone());
        }
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
        label: job.label.clone(),
        // The wire carries the label under its old name too until clients
        // stop reading one: an old payload reader has `deny_unknown_fields`
        // and an extra field is a parse error, not a silent miss.
        script: job.label.clone(),
        state: job.state,
        exit_code: job.exit_code,
        timed_out: job.timed_out,
        elapsed_ms: job.elapsed_ms,
        output: job.output.clone(),
        dropped: job.dropped,
        output_bytes_total: job.output_bytes_total,
    }
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Appends output while keeping only the newest bounded tail.
fn append_output(job: &mut Job, text: &str) {
    job.summary.output.push_str(text);
    job.bytes += text.len();
    job.summary.output_bytes_total = job
        .summary
        .output_bytes_total
        .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
    if job.bytes <= JOB_BUFFER_BYTES {
        return;
    }
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

fn expire_output(job: &mut Job, retention_s: u64, now: std::time::Instant) {
    let retention = Duration::from_secs(retention_s);
    if job
        .finished
        .is_some_and(|finished| now.saturating_duration_since(finished) >= retention)
        && !job.summary.output.is_empty()
    {
        job.summary.output.clear();
        job.bytes = 0;
        job.summary.dropped = true;
    }
}

/// The label as one line, for a listing that has one line per job.
fn one_line(label: &str) -> String {
    let mut line: String = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 80 {
        line = line.chars().take(77).collect::<String>() + "...";
    }
    line
}
