//! The shell call: one executor, waited for or let go of.
//!
//! There is one `shell` tool, and what distinguishes a call that holds the turn
//! from one that does not is a field on the request. `detach: true` lets the
//! model start something and carry on; the person at the keyboard can ask for the
//! same thing from outside with `Ctrl-G` while a call runs. Both end the same way:
//! the process is left running, it is registered as a job, `manage_jobs` can read
//! it and stop it, and what it writes afterwards reaches the model as a message
//! when the process ends.
//!
//! That is why the two paths are one wrapper, and why this module replaces the
//! two that used to exist. `shell_bg` was a second declaration over the same
//! executor, and the pair drifted apart in every place that had to name them: the
//! renderer, the tool list, the tests, the description the model reads. A field
//! says the same thing in one place, and the executor grew it in
//! `ShellRequest::detach`.
//!
//! What is left for this wrapper is the half the executor cannot do: a job has to
//! be registered, its output filed somewhere a reader can find it, and its end
//! reported to a model that has no tool call left to answer. The engine's own
//! progress sink reports into the running turn's channel, which is a live view of
//! a call that is being waited for; a call nobody waits for has no such channel,
//! so [`ShellProgress`] keeps the output in the registry instead.
//!
//! A detached process is not out of reach. The token a job is stopped with is a
//! token of its own, linked to the turn's, so `manage_jobs stop` kills the group
//! without touching the turn, and cancelling the turn still kills the call.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use norte::{
    CallId, Cancellation, FnExecutor, Part, ProgressSink, Result, SessionId, ToolContext,
    ToolDeclaration, ToolOutput, ToolProgress,
};
use norte_tools::ShellRequest;

use super::detach::DetachRegistry;
use super::jobs::{JobRegistry, JobSummary};
use super::steering::{Steered, Steering};

/// What a call hands back when it is detached before it has anything to show.
///
/// It is a normal tool result, which is the point: the conversation stays
/// coherent — a `tool_call` always has its `tool_result` — and the job reports
/// separately when it has something to say.
fn started(id: &str, script: &str) -> ToolOutput {
    let head: String = script
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect();
    ToolOutput::text(format!(
        "Job `{id}` is running in the background: {head}\n\n\
         Its output will arrive as a message when it ends. Use `manage_jobs` with \
         `action: check` and this id to read what it has written so far, or `action: stop` \
         to end it."
    ))
}

/// The shell's executor, with a job for every call that is let go of.
pub(super) struct ShellCalls {
    /// The executor being wrapped, with the host's limits already applied.
    inner: Arc<dyn FnExecutor<ShellRequest, ToolOutput>>,
    /// Where a running job is registered, so it can be read and stopped.
    jobs: Arc<JobRegistry>,
    /// How a job's result reaches the model when there is no call to answer.
    steering: Arc<Steering>,
    /// Where the running call's detach token is published, so `Ctrl-G` reaches it.
    detaches: Arc<DetachRegistry>,
}

impl ShellCalls {
    /// Wraps `declaration`, so its calls can be waited for or let go of.
    pub(super) fn declaration(
        declaration: ToolDeclaration<ShellRequest>,
        jobs: Arc<JobRegistry>,
        steering: Arc<Steering>,
        detaches: Arc<DetachRegistry>,
    ) -> ToolDeclaration<ShellRequest> {
        ToolDeclaration {
            name: declaration.name,
            description: declaration.description,
            parameters: declaration.parameters,
            executor: Arc::new(Self {
                inner: declaration.executor,
                jobs,
                steering,
                detaches,
            }),
        }
    }
}

#[async_trait]
impl FnExecutor<ShellRequest, ToolOutput> for ShellCalls {
    async fn call(&self, request: ShellRequest) -> Result<ToolOutput> {
        // Without an engine there is no session to report into and no call to
        // attach a job to, so the only answer available is the executor's own.
        self.inner.call(request).await
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        mut request: ShellRequest,
    ) -> Result<ToolOutput> {
        let (Some(session), Some(call_id)) = (context.session, context.call_id) else {
            return self.inner.call_with_context(context, request).await;
        };
        // No bound only means anything for a call nobody waits for. A request
        // that asks for one and does not detach is asking to run forever while
        // the turn is held, which is the one shape the host refuses: it gets the
        // host's default instead, exactly as it did when the two shells had
        // different limits.
        if !request.detach && request.timeout_ms == Some(0) {
            request.timeout_ms = None;
        }
        let script = request.script.clone();

        // The call runs under a token that either the turn or the job can
        // signal: the turn's cancellation is what `Escape` reaches, and the
        // job's is what `/jobs stop` and `Ctrl-C` reach. Linking them one way —
        // turn to job and not back — is what keeps stopping a job from stopping
        // a turn while leaving a running call cancellable the way it always was.
        let cancel = Arc::new(Cancellation::new());
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn({
            let cancel = Arc::clone(&cancel);
            let turn = context.cancellation();
            async move {
                tokio::select! {
                    () = turn.cancelled() => cancel.cancel(),
                    // The call is over: from here the process answers to `/jobs
                    // stop` and to its own bound, and not to a turn that has
                    // already moved on.
                    _ = done_rx => {}
                }
            }
        });

        // A call that asked to be detached is a job from the start, so the job
        // exists before the process has written anything. One that did not is a
        // job only if it is detached later, and then the sink below registers it
        // on the way past.
        let eager = request.detach;
        let id = eager.then(|| {
            self.jobs
                .start(session, call_id, &script, Arc::clone(&cancel))
        });

        // The detach token this call watches, published so `Ctrl-G` can reach it.
        let token = Arc::new(Cancellation::new());
        self.detaches.attach(session, Arc::clone(&token));

        let sink = Arc::new(ShellProgress {
            engine: context.progress(),
            jobs: Arc::clone(&self.jobs),
            steering: Arc::clone(&self.steering),
            session,
            call_id,
            script,
            detach: (*token).clone(),
            cancel: Arc::clone(&cancel),
            eager,
            // How many closing chunks to expect before the job is over: the
            // reader reports one per stream it reads, and merging means there is
            // only one. A process that never started emits none, and is settled
            // below instead.
            open: AtomicU8::new(if request.merge_stderr { 1 } else { 2 }),
            closed: AtomicBool::new(false),
            reported: AtomicBool::new(false),
            job: Mutex::new(id.clone()),
        });

        let scoped = context
            .clone()
            .with_cancellation((*cancel).clone())
            .with_detach((*token).clone())
            .with_progress(sink.clone());
        let outcome = self.inner.call_with_context(&scoped, request).await;
        let _ = done_tx.send(());
        self.detaches.release(session, &token);

        // What decides is what the call did, not when a chunk happened to
        // arrive. A `Ctrl-G` on a process that then says nothing would otherwise
        // leave no job behind at all — the sink only registers one as a chunk
        // goes past — and the model would be told the call went with nothing to
        // read from. `detached` is in the result either way, whoever asked.
        let let_go = outcome.as_ref().is_ok_and(detached);
        if let_go {
            sink.adopt();
        }
        let job = sink.job_id();
        // A call that failed without starting has no streams to close, so its
        // job would sit `running` for ever. Its own failure is its end.
        if let Some(id) = job.as_ref() {
            if !let_go {
                self.jobs.settle(id, None, false);
            }
        }

        match (eager, job) {
            // Detached before it had anything to show: the job id is the answer,
            // because there is no output yet to send with it.
            (true, Some(id)) if let_go => Ok(started(&id, sink.script())),
            // It let go of a call that was already running, so the result the
            // model asked for is not the whole of it: the call's own text says
            // what arrived before it went, and the job named beside it is where
            // the rest will be. This is the `Ctrl-G` answer, and it is what the
            // model needs, because the output it has is only the beginning.
            (false, Some(id)) if let_go => Ok(name_job(outcome?, &id)),
            _ => outcome,
        }
    }
}

/// Whether a call's own output says it was let go of.
fn detached(output: &ToolOutput) -> bool {
    output
        .structured
        .as_ref()
        .and_then(|value| value.get("detached"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Appends the job's name to what a let-go call returned.
///
/// The model is told which job now owns the rest of the output, because the text
/// it has is the part that arrived before the call went: without the name there
/// is nothing to hand `manage_jobs`.
fn name_job(mut output: ToolOutput, id: &str) -> ToolOutput {
    output.parts.push(Part::text(format!(
        "\nThe call was detached. Its process keeps running as job `{id}`: read what it has \
         written with `manage_jobs` (`action: check`), end it with `action: stop`, or wait for \
         its report when it ends."
    )));
    output
}

/// Where a shell call's output goes, and what makes it a job.
///
/// While a call is being waited for this is a pass-through: it delivers to the
/// sink the engine installed, so the client keeps its live view. Once the call is
/// let go of there is no live view to keep — the turn has moved on — so it files
/// the bytes in the job registry instead, where `manage_jobs` can find them. The
/// same moment is when the job is registered for a call that was not a job when
/// it started.
struct ShellProgress {
    /// The sink the engine installed, absent when nothing is rendering.
    engine: Option<Arc<dyn ProgressSink>>,
    jobs: Arc<JobRegistry>,
    steering: Arc<Steering>,
    session: SessionId,
    call_id: CallId,
    script: String,
    /// Signalled when the call is let go of.
    detach: Cancellation,
    /// The token `/jobs stop` signals.
    cancel: Arc<Cancellation>,
    /// Whether the call was a job before it ran.
    eager: bool,
    /// How many streams have still to close.
    open: AtomicU8,
    /// Whether every stream has closed, which is the process ending.
    closed: AtomicBool,
    /// Whether the end has been reported, so it is reported once.
    reported: AtomicBool,
    /// The job, once there is one.
    job: Mutex<Option<String>>,
}

impl ShellProgress {
    /// The script the call runs, for the answer that names the job.
    fn script(&self) -> &str {
        &self.script
    }

    /// The job this call belongs to, if it belongs to one.
    fn job_id(&self) -> Option<String> {
        self.job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Registers the job for a call that was not one when it started.
    ///
    /// It runs when a chunk arrives after the call was let go of, and again when
    /// the call returns: the second is what covers a process that says nothing
    /// afterwards, which no chunk would ever register.
    fn adopt(&self) {
        {
            let mut job = self
                .job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if job.is_some() {
                return;
            }
            *job = Some(self.jobs.start(
                self.session,
                self.call_id,
                &self.script,
                Arc::clone(&self.cancel),
            ));
        }
        // The streams may have closed before there was a job to end; now there
        // is one, so the end it missed is reported.
        self.finish();
    }
}

impl ProgressSink for ShellProgress {
    fn send(&self, progress: ToolProgress) {
        // The call was let go of: it is a job now, and what it writes belongs to
        // whoever reads it later rather than to a turn that has moved on.
        let is_job = self.eager || self.detach.is_cancelled();
        if is_job {
            self.adopt();
        }
        if !progress.bytes.is_empty() {
            let text = String::from_utf8_lossy(&progress.bytes).into_owned();
            if is_job {
                self.jobs.write(self.call_id, &text);
            } else if let Some(engine) = &self.engine {
                engine.send(progress.clone());
            }
        }
        // A stream's closing chunk is its pipe closing. The job is over once
        // every stream has closed — one for a merged run, two otherwise — and
        // not before: ending on the first would cut off a job whose stderr is
        // still producing.
        if progress.last && self.open.fetch_sub(1, Ordering::AcqRel) <= 1 {
            self.closed.store(true, Ordering::Release);
            self.finish();
        }
    }
}

impl ShellProgress {
    /// Ends the job, and tells the model what it wrote.
    ///
    /// It does nothing until both halves are true: there is a job, and its
    /// streams have closed. Either can come first, so this is called from both
    /// ends and reports once — a process that ends before the call returns would
    /// otherwise be reported twice, or not at all.
    fn finish(&self) {
        if !self.closed.load(Ordering::Acquire) {
            return;
        }
        let Some(id) = self.job_id() else {
            return;
        };
        if self.reported.swap(true, Ordering::AcqRel) {
            return;
        }
        self.jobs.close(self.call_id, true);
        let text = self
            .jobs
            .get(&id)
            .map_or_else(String::new, |job| job.output);
        // The result has no tool call left to answer — the call that started it
        // was answered when it let go — so it arrives as a user message: the
        // model reads it at the next safe point of the turn, or at the start of
        // the next one.
        self.steering.push(
            self.session,
            Steered::new(format!("job:{id}"), report(&id, &text)),
        );
    }
}

/// What the model reads when a job ends.
fn report(id: &str, output: &str) -> String {
    let clipped: String = output.chars().take(4000).collect();
    let tail = if output.chars().count() > 4000 {
        "\n[output truncated; read the rest with manage_jobs]"
    } else {
        ""
    };
    format!("Background job `{id}` finished.\n\n{clipped}{tail}")
}

/// What a job looks like to the model, as `manage_jobs` prints it.
pub(super) fn describe(job: &JobSummary) -> String {
    let mut line = format!(
        "`{}` {} — {}{} ({} ms)",
        job.id,
        job.state.label(),
        job.script,
        match job.exit_code {
            Some(code) => format!(", exit {code}"),
            None => String::new(),
        },
        job.elapsed_ms
    );
    if job.timed_out {
        line.push_str("\n  it reached its time bound");
    }
    if job.dropped {
        line.push_str("\n  older output was dropped to stay within the buffer");
    }
    if !job.output.is_empty() {
        line.push_str("\n  output:\n");
        for chunk in job.output.lines().take(40) {
            line.push_str("    ");
            line.push_str(chunk);
            line.push('\n');
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sink with a job already registered, its streams closed.
    fn ended_sink() -> (
        Arc<ShellProgress>,
        Arc<JobRegistry>,
        Arc<Steering>,
        SessionId,
    ) {
        let jobs = Arc::new(JobRegistry::default());
        let steering = Arc::new(Steering::default());
        let session = SessionId::new();
        let call_id = CallId::new();
        let cancel = Arc::new(Cancellation::new());
        let id = jobs.start(session, call_id, "echo hi", Arc::clone(&cancel));
        let sink = Arc::new(ShellProgress {
            engine: None,
            jobs: Arc::clone(&jobs),
            steering: Arc::clone(&steering),
            session,
            call_id,
            script: "echo hi".to_owned(),
            detach: Cancellation::new(),
            cancel,
            eager: true,
            open: AtomicU8::new(0),
            closed: AtomicBool::new(true),
            reported: AtomicBool::new(false),
            job: Mutex::new(Some(id)),
        });
        (sink, jobs, steering, session)
    }

    #[test]
    fn a_job_is_reported_once_however_many_ends_see_it() {
        // The chunk that closes the streams and the call returning are two
        // threads, and either can be the one that notices the job is over: the
        // process may end while the call has not returned yet. Both call this,
        // and the model must not get a second message about the same process.
        let (sink, _jobs, steering, session) = ended_sink();
        sink.finish();
        sink.finish();
        sink.finish();
        let steered = steering.take(session);
        assert_eq!(
            steered.len(),
            1,
            "one process, one report, however many ends noticed: {steered:?}"
        );
        assert_eq!(steered[0].id, "job:job-1");
    }

    #[test]
    fn a_job_adopted_after_its_process_ended_still_reports() {
        // The race the two ends can lose. The closing chunk is read just before
        // the detach, so it finds no job to end and reports nothing; the call
        // then returns detached and registers the job — whose process is already
        // gone. Without the end being attempted again here, that job would never
        // be reported at all, and the model would wait forever for a result that
        // had already been produced.
        let jobs = Arc::new(JobRegistry::default());
        let steering = Arc::new(Steering::default());
        let session = SessionId::new();
        let call_id = CallId::new();
        let sink = Arc::new(ShellProgress {
            engine: None,
            jobs: Arc::clone(&jobs),
            steering: Arc::clone(&steering),
            session,
            call_id,
            script: "echo hi".to_owned(),
            detach: Cancellation::new(),
            cancel: Arc::new(Cancellation::new()),
            eager: false,
            open: AtomicU8::new(0),
            // The streams closed while this was not yet a job, so nothing was
            // reported then.
            closed: AtomicBool::new(true),
            reported: AtomicBool::new(false),
            job: Mutex::new(None),
        });

        sink.adopt();

        let job = jobs.get("job-1").expect("the job was registered");
        assert!(job.state.is_done(), "its process had already ended");
        let steered = steering.take(session);
        assert_eq!(
            steered.len(),
            1,
            "the end it missed was reported when it became a job: {steered:?}"
        );
    }

    #[test]
    fn nothing_is_reported_before_the_streams_close() {
        // The other half of the same rule: a job that has not ended has nothing
        // to report, so a call returning early must not announce an end that has
        // not happened.
        let (sink, _jobs, steering, session) = ended_sink();
        sink.closed.store(false, Ordering::Release);
        sink.finish();
        assert!(
            steering.take(session).is_empty(),
            "a job that is still running reports nothing"
        );
    }
}
