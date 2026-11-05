//! Subagents: a delegated turn on a child session.
//!
//! `subagent` is a tool with the same two shapes `shell` has: waited for, or
//! let go of. Waited for, it runs one turn on a child session and returns the
//! child's final message as its own result. Let go of (`detach: true`), it
//! becomes a job the registry holds, its result arrives as a steered message
//! when the child settles, and `manage_jobs` can read it and stop it.
//!
//! The child is a session like any other, created from the same service. The
//! constraints are the service's, not the engine's: the child never sees the
//! `subagent` tool (its declaration list is frozen without it, at the moment
//! it is created), so it cannot spawn another one and the recursion depth is
//! one by construction. How many may run at once is a semaphore, not a queue:
//! a queue would let one turn hold every provider slot behind an unbounded
//! fan-out; past the cap, the call is an error the model reads.
//!
//! The wrapper is here and not in the broker for the same reason `ShellCalls`
//! wraps the shell: what a job needs — a registration, an output that a reader
//! can find later, an end reported to a model that has no call left to answer
//! — is the host's business, and the engine knows nothing about it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use norte::{Cancellation, FnExecutor, Part, SessionId, ToolContext, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Service;
use super::detach::DetachRegistry;
use super::jobs::JobRegistry;
use super::steering::{Steered, Steering};

/// The arguments of `subagent`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SubagentArgs {
    /// The one task, with everything the child needs. The child starts blank.
    pub prompt: String,
    /// The child session's title.
    pub title: Option<String>,
    /// The provider the model asks for, or none to inherit the parent's.
    pub provider: Option<String>,
    /// The model the model asks for, or none to inherit the parent's.
    pub model: Option<String>,
    /// Whether the call is let go of as a job.
    pub detach: bool,
}

/// What one run of a child session produced.
#[derive(Debug)]
pub(super) struct SubagentOutcome {
    /// The child's final message, when it committed one.
    pub text: String,
    /// Whether the child ended with an error.
    pub is_error: bool,
}

/// Where a subagent's turn is run from.
///
/// The service implements this: create the child, build its engine, run one
/// turn, and read back what it committed. The tool does not reach into the
/// service's lock-guarded state — it asks through this trait.
#[async_trait]
pub(super) trait SubagentRunner: Send + Sync {
    /// Runs one turn of `prompt` on a fresh child session.
    ///
    /// `label` is what the job registry prints for the run; `cancel` stops it.
    async fn run(
        &self,
        prompt: &str,
        parent: SessionId,
        label: &str,
        cancel: &Cancellation,
    ) -> SubagentOutcome;
}

/// A gate that enforces a cap on concurrent subagents.
pub(super) struct Gate {
    cap: usize,
    running: AtomicUsize,
}

impl Gate {
    /// Takes a slot, or reports that there is none.
    pub(super) fn acquire(&self) -> std::result::Result<(), String> {
        let mut running = self.running.load(Ordering::Acquire);
        loop {
            if running >= self.cap {
                return Err(format!(
                    "too many subagents running ({} at once); stop one with manage_jobs or wait",
                    self.cap
                ));
            }
            match self.running.compare_exchange_weak(
                running,
                running + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(observed) => running = observed,
            }
        }
    }

    /// Gives a slot back when a run ends.
    pub(super) fn release_slot(&self) {
        // One decrement, no loop: a lost race only ever under-counts, which
        // the next acquire retries against the value it sees.
        if self.running.load(Ordering::Acquire) > 0 {
            self.running.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("cap", &self.cap)
            .finish_non_exhaustive()
    }
}

/// The tool itself: a runner, plus where its jobs and its detach tokens live.
pub(super) struct SubagentTool {
    runner: Arc<dyn SubagentRunner>,
    jobs: Arc<JobRegistry>,
    steering: Arc<Steering>,
    detaches: Arc<DetachRegistry>,
    /// The service, so a finished subagent can start a turn of its own when
    /// the flag is on.
    service: Arc<RwLock<Option<Arc<Service>>>>,
    gate: Gate,
}

impl SubagentTool {
    /// A tool backed by `runner`, capped at `max_concurrent` runs.
    pub(super) fn new(
        runner: Arc<dyn SubagentRunner>,
        jobs: Arc<JobRegistry>,
        steering: Arc<Steering>,
        detaches: Arc<DetachRegistry>,
        service: Arc<RwLock<Option<Arc<Service>>>>,
        max_concurrent: usize,
    ) -> Self {
        Self {
            runner,
            jobs,
            steering,
            detaches,
            service,
            gate: Gate {
                cap: max_concurrent.max(1),
                running: AtomicUsize::new(0),
            },
        }
    }

    /// The gate, for tests.
    #[cfg(test)]
    pub(super) fn gate_for_test(&self) -> &Gate {
        &self.gate
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<SubagentArgs> {
        norte::ToolDeclaration::new(
            "subagent",
            "Run one turn on a new session and return what it said. Use it to delegate a task that does not need the parent's context: the child starts blank, so the prompt must carry everything it needs. With `detach: true` the call becomes a job (`job-N`): the run continues in the background, `manage_jobs` reads it and stops it, and its result arrives as a message when it ends. A child never has `subagent`, so nesting stops at one.",
            Arc::new(self),
        )
    }
}

/// What a detached subagent hands back before the child has anything to show.
///
/// It is a normal tool result, which is the point: the conversation stays
/// coherent — a `tool_call` always has its `tool_result` — and the child
/// reports separately when it has something to say.
fn started(id: &str, label: &str) -> ToolOutput {
    ToolOutput::text(format!(
        "Subagent `{id}` is running in the background: {label}\n\n\
         Its result will arrive as a message when it ends. Use `manage_jobs` with \
         `action: check` and this id to read what it has written so far, or `action: stop` \
         to end it."
    ))
}

/// What the model reads when a subagent job ends.
fn report(id: &str, outcome: &SubagentOutcome) -> String {
    let head = if outcome.is_error {
        "Subagent error"
    } else {
        "Subagent result"
    };
    let clipped: String = outcome.text.chars().take(4000).collect();
    let tail = if outcome.text.chars().count() > 4000 {
        "\n[result truncated; read the rest with manage_jobs]"
    } else {
        ""
    };
    format!("{head} from `{id}`.\n\n{clipped}{tail}")
}

#[async_trait]
impl FnExecutor<SubagentArgs, ToolOutput> for SubagentTool {
    async fn call(&self, _args: SubagentArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error(
            "subagent needs a client attached: a child session belongs to a parent one",
        ))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: SubagentArgs,
    ) -> norte::Result<ToolOutput> {
        let (Some(parent), Some(_call_id)) = (context.session, context.call_id) else {
            return Ok(ToolOutput::error(
                "subagent needs a client attached: a child session belongs to a parent one",
            ));
        };
        if let Err(message) = self.gate.acquire() {
            return Ok(ToolOutput::error(message));
        }
        let label = subagent_label(&args);
        let cancel = Arc::new(Cancellation::new());
        // The detach token this call watches, published so `Ctrl-G` reaches
        // it, like any other call of the session.
        let token = Arc::new(Cancellation::new());
        self.detaches.attach(parent, Arc::clone(&token));

        let detached_early = args.detach;
        let job = detached_early.then(|| {
            self.jobs
                .start(parent, _call_id, &label, Arc::clone(&cancel))
        });

        // Unlike a shell call, a subagent's result is read by the holder
        // (this future), so a job whose child ends reports itself from here
        // rather than from a progress sink: the holder is the one that saw
        // the outcome, and it is also the task the job's cancellation
        // reaches.
        let turn_cancelled = context.cancellation();
        let outcome = tokio::select! {
            outcome = self.runner.run(&args.prompt, parent, &label, &cancel) => outcome,
            () = token.cancelled() => SubagentOutcome {
                text: String::new(),
                is_error: false,
            },
            () = turn_cancelled.cancelled() => SubagentOutcome {
                text: String::new(),
                is_error: true,
            },
        };
        self.detaches.release(parent, &token);
        self.gate.release_slot();

        let let_go = detached_early || token.is_cancelled();
        let job = match job {
            // A `detach: true` job is registered before the child runs, so
            // it exists before the child has written anything.
            Some(id) => Some(id),
            // A `Ctrl-G` on a child that was already running makes it a job
            // only now, on the way out.
            None if let_go => Some(
                self.jobs
                    .start(parent, _call_id, &label, Arc::clone(&cancel)),
            ),
            None => None,
        };

        if let Some(id) = &job {
            self.jobs.write_id(id, &outcome.text);
            self.jobs.finish(id, None);
            self.steering.push(
                parent,
                Steered::new(format!("subagent:{id}"), report(id, &outcome)),
            );
            if let Some(service) = self
                .service
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            {
                service.schedule_auto_turn(parent);
            }
        }

        match (detached_early, &job) {
            (true, Some(id)) if let_go => Ok(started(id, &label)),
            (false, Some(id)) if let_go => Ok(name_outcome(outcome, id)),
            _ => Ok(outcome_to_output(outcome)),
        }
    }
}

/// The one line a listing has for the child.
fn subagent_label(args: &SubagentArgs) -> String {
    match &args.title {
        Some(title) if !title.trim().is_empty() => format!("subagent: {title}"),
        _ => {
            let prompt: String = args.prompt.chars().take(80).collect();
            format!("subagent: {prompt}")
        }
    }
}

/// Appends the job's name to what a let-go child returned.
fn name_outcome(mut outcome: SubagentOutcome, id: &str) -> ToolOutput {
    let output = outcome_to_output(std::mem::replace(
        &mut outcome,
        SubagentOutcome {
            text: String::new(),
            is_error: false,
        },
    ));
    let mut output = output;
    output.parts.push(Part::text(format!(
        "\nThe child was detached. Its run continues as job `{id}`: read what it has \
         written with `manage_jobs` (`action: check`), end it with `action: stop`, or wait for \
         its report when it ends."
    )));
    output
}

/// A child's outcome as the tool result the parent reads.
fn outcome_to_output(outcome: SubagentOutcome) -> ToolOutput {
    if outcome.is_error {
        ToolOutput::error(outcome.text)
    } else {
        ToolOutput::text(outcome.text)
    }
}
