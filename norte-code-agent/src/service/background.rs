//! `shell_bg`: the shell tool, run as a background job.
//!
//! It wraps the same `ShellTool` the foreground `shell` uses, so the sandbox,
//! the redaction, the output bounds and the timeout are the ones already
//! configured — this adds one thing, and only one: the call answers as soon as
//! the process starts instead of waiting for it.
//!
//! The waited path is the one the engine drives: it calls the tool and gets a
//! result. This one has to return *before* the result exists, which means the
//! tool call runs on its own task with its own cancellation token. What happens
//! to the process afterwards is the registry's business, and what happens to the
//! output is the steering's: when the task ends, its result reaches the model as
//! a user message, because a `tool_call` that is already answered cannot take a
//! second `tool_result`.
//!
//! So the model is told:
//!
//! ```text
//! Job job-1 is running. Its output will arrive as a message when it ends.
//! ```
//!
//! and the job id is what `manage_jobs` and `/jobs` take.

use std::sync::Arc;

use async_trait::async_trait;
use norte::{Cancellation, FnExecutor, Result, ToolContext, ToolDeclaration, ToolOutput};
use norte_tools::ShellRequest;

use super::jobs::{JobRegistry, JobSummary};
use super::steering::{Steered, Steering};

/// The tool name the model calls to start something in the background.
pub(super) const SHELL_BG: &str = "shell_bg";

/// What a background call hands back while it runs.
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

/// Everything a detached call needs to finish on its own.
pub(super) struct BackgroundShell {
    /// The executor being wrapped, with the host's limits already applied.
    inner: Arc<dyn FnExecutor<ShellRequest, ToolOutput>>,
    /// Where a running job is registered, so it can be read and stopped.
    jobs: Arc<JobRegistry>,
    /// How the result reaches the model when there is no tool call to answer.
    steering: Arc<Steering>,
}

impl BackgroundShell {
    /// Wraps `declaration`, running its calls in the background.
    pub(super) fn declaration(
        declaration: ToolDeclaration<ShellRequest>,
        jobs: Arc<JobRegistry>,
        steering: Arc<Steering>,
    ) -> ToolDeclaration<ShellRequest> {
        ToolDeclaration {
            name: SHELL_BG,
            description: "Run a shell script in the background and answer at once with a job id. \
                          Use it for something that takes a while, when you have other work to do \
                          meanwhile. The output arrives as a message when it ends; `manage_jobs` \
                          reads or stops a running job. For anything you need the answer to before \
                          continuing, use `shell`.",
            parameters: declaration.parameters,
            executor: Arc::new(BackgroundShell {
                inner: declaration.executor,
                jobs,
                steering,
            }),
            approval: declaration.approval,
        }
    }
}

#[async_trait]
impl FnExecutor<ShellRequest, ToolOutput> for BackgroundShell {
    async fn call(&self, request: ShellRequest) -> Result<ToolOutput> {
        // Without an engine there is no session to report into and no call to
        // attach a job to, so the only answer available is the waited one.
        self.inner.call(request).await
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        request: ShellRequest,
    ) -> Result<ToolOutput> {
        let (Some(session), Some(call_id)) = (context.session, context.call_id) else {
            return self.inner.call_with_context(context, request).await;
        };
        let script = request.script.clone();
        // The job's own token: stopping it must not stop the turn, and cancelling
        // the turn must not stop the job — they are different questions and the
        // process answers to both.
        let cancel = Arc::new(Cancellation::new());
        let id = self
            .jobs
            .start(session, call_id, &script, Arc::clone(&cancel));

        let inner = Arc::clone(&self.inner);
        let jobs = Arc::clone(&self.jobs);
        let steering = Arc::clone(&self.steering);
        let job_id = id.clone();
        // The context is a borrow and the task outlives this call, so what the
        // job needs is taken out of it first: the metadata is the host's, and
        // the detach token is how the job's own work is later asked to stop.
        let metadata: Vec<(String, serde_json::Value)> = context
            .metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let detach = context.detach();
        tokio::spawn(async move {
            let mut job_context = ToolContext::new()
                .with_session(session)
                .with_call(call_id)
                .with_cancellation((*cancel).clone())
                .with_detach(detach);
            for (key, value) in metadata {
                job_context = job_context.with_metadata(key, value);
            }
            let outcome = inner.call_with_context(&job_context, request).await;
            let (text, exit_code) = match &outcome {
                Ok(output) => (output_text(output), exit_code_of(output)),
                Err(error) => (format!("the job failed: {error}"), None),
            };
            jobs.finish(call_id, exit_code);
            // The result has no tool call left to answer, so it arrives as a
            // user message: the model reads it at the next safe point of the
            // turn, or at the start of the next one.
            steering.push(
                session,
                Steered::new(format!("job:{job_id}"), report(&job_id, &text)),
            );
        });

        Ok(started(&id, &script))
    }
}

/// The text of a `ToolOutput`, whatever shape it took.
fn output_text(output: &ToolOutput) -> String {
    let text = output
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        output
            .structured
            .as_ref()
            .map_or_else(String::new, ToString::to_string)
    } else {
        text
    }
}

/// The exit code a shell result reported, when it reported one.
fn exit_code_of(output: &ToolOutput) -> Option<i32> {
    output
        .structured
        .as_ref()
        .and_then(|value| value.get("exit_code"))
        .and_then(serde_json::Value::as_i64)
        .and_then(|code| i32::try_from(code).ok())
}

/// What the model reads when a background job ends.
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
