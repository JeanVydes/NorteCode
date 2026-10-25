//! `manage_jobs`: what the model needs to read or stop a background job.
//!
//! `shell_bg` answers with a job id and nothing else, because a tool result is
//! the wrong place to put output that does not exist yet. This is where that
//! output is, and the only way to reach it: a job whose result arrived as a
//! message is gone from the conversation by design, so a model that wants what
//! a *running* job has written asks here.
//!
//! Three actions, and no more. `list` is what a model reaches for when it has
//! lost the id; `check` reads one; `stop` ends one. Anything else would be the
//! model guessing at a control surface it cannot see.

use async_trait::async_trait;
use norte::{FnExecutor, Result, ToolContext, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::background::describe;
use super::jobs::JobRegistry;
use std::sync::Arc;

/// What `manage_jobs` can do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
pub(super) enum JobAction {
    /// Every job this session has, oldest first.
    #[default]
    List,
    /// One job, with what it has written.
    Check,
    /// End one job and its process.
    Stop,
}

/// The arguments of `manage_jobs`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ManageJobsArgs {
    /// What to do.
    #[serde(default)]
    #[schemars(description = "`list` every job, `check` one by id, or `stop` one by id.")]
    pub(super) action: JobAction,
    /// The job to act on, for `check` and `stop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(example = &"job-1")]
    pub(super) id: Option<String>,
}

/// The tool.
pub(super) struct ManageJobs {
    jobs: Arc<JobRegistry>,
}

impl ManageJobs {
    /// A tool backed by `jobs`.
    pub(super) fn new(jobs: Arc<JobRegistry>) -> Self {
        Self { jobs }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<ManageJobsArgs> {
        norte::ToolDeclaration::new(
            "manage_jobs",
            "List the background jobs of this session, read what one has written, or stop one. \
             `shell_bg` returns the job id a call belongs to; use `check` to read a job that is \
             still running, and `stop` to end it early. A job that finished reports itself as a \
             message, so there is nothing to read here for one you have already seen.",
            Arc::new(self),
        )
    }

    fn check(&self, id: &str) -> ToolOutput {
        match self.jobs.get(id) {
            Some(job) => ToolOutput::text(describe(&job)),
            None => ToolOutput::error(format!(
                "no job `{id}` in this session; use `action: list` to see the ones there are"
            )),
        }
    }

    fn stop(&self, id: &str) -> ToolOutput {
        if self.jobs.cancel(id) {
            ToolOutput::text(format!(
                "Job `{id}` was stopped. Its process is being killed, and whatever it wrote first \
                 is in `action: check`."
            ))
        } else {
            ToolOutput::error(format!(
                "no running job `{id}`; it may have finished already, in which case `action: list` \
                 shows its result"
            ))
        }
    }
}

impl ManageJobs {
    /// The jobs of the session the call belongs to.
    fn list(&self, context: &ToolContext) -> ToolOutput {
        let jobs = self.jobs.list(context.session);
        if jobs.is_empty() {
            return ToolOutput::text("No background jobs in this session.");
        }
        let listing: Vec<String> = jobs.iter().map(describe).collect();
        ToolOutput::text(listing.join("\n\n"))
    }
}

#[async_trait]
impl FnExecutor<ManageJobsArgs, ToolOutput> for ManageJobs {
    async fn call(&self, args: ManageJobsArgs) -> Result<ToolOutput> {
        // Without a session there is no set of jobs to answer about, and `list`
        // for every session alive is not something a model should be able to
        // ask for.
        Ok(match args.action {
            JobAction::List => {
                ToolOutput::error("manage_jobs needs a client attached: a job belongs to a session")
            }
            JobAction::Check => match args.id.as_deref() {
                Some(id) => self.check(id),
                None => ToolOutput::error("`check` needs an `id`"),
            },
            JobAction::Stop => match args.id.as_deref() {
                Some(id) => self.stop(id),
                None => ToolOutput::error("`stop` needs an `id`"),
            },
        })
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: ManageJobsArgs,
    ) -> Result<ToolOutput> {
        Ok(match args.action {
            JobAction::List => self.list(context),
            JobAction::Check => match args.id.as_deref() {
                Some(id) => self.check(id),
                None => ToolOutput::error("`check` needs an `id`"),
            },
            JobAction::Stop => match args.id.as_deref() {
                Some(id) => self.stop(id),
                None => ToolOutput::error("`stop` needs an `id`"),
            },
        })
    }
}
