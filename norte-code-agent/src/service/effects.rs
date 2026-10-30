//! Effect journal: refuse to replay a call a crash may have already run.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use norte::{CallId, SessionId};
use norte_code_protocol::sqlite::SessionDirectory;

use crate::ServiceError;

/// The tools whose effects are not safely repeatable after a crash.
///
/// Named as the registry names them. This list spelled the shell as `command`
/// for as long as the approval one did, so a command a crash may have run was
/// never journalled and a replay would have run it twice.
const JOURNALED_TOOLS: [&str; 4] = ["write", "edit", "shell", "skill_script"];

fn is_journaled(tool: &str) -> bool {
    JOURNALED_TOOLS.contains(&tool)
}

/// Intent/done records for effectful tools, keyed by session and call id.
///
/// Norte guarantees commit-at-most-once, not effect-at-most-once. The intent is
/// written by the engine's tool-metadata callback immediately before the
/// executor runs; a row still at `intent` when the process starts means a crash
/// may have happened mid-effect.
pub(super) struct EffectJournal {
    directory: Arc<dyn SessionDirectory>,
    epoch: String,
    suspects: Mutex<BTreeSet<(SessionId, CallId)>>,
}

impl EffectJournal {
    pub(super) fn new(directory: Arc<dyn SessionDirectory>) -> Self {
        Self {
            directory,
            epoch: uuid::Uuid::new_v4().to_string(),
            suspects: Mutex::new(BTreeSet::new()),
        }
    }

    /// Loads the calls a previous process started without finishing.
    pub(super) async fn load(&self) -> Result<(), ServiceError> {
        let pending = self.directory.unfinished_effects().await?;
        let mut suspects = self.suspects.lock().unwrap_or_else(PoisonError::into_inner);
        for effect in pending {
            suspects.insert((effect.session, effect.call_id));
        }
        Ok(())
    }

    pub(super) fn intent(&self, session: SessionId, call_id: CallId, tool: &str) {
        if !is_journaled(tool) {
            return;
        }
        if let Err(error) = self
            .directory
            .effect_started(session, call_id, tool, &self.epoch)
        {
            tracing::warn!(%error, %call_id, "could not journal a tool intent");
        }
    }

    pub(super) fn finished(&self, session: SessionId, call_id: CallId, tool: &str, is_error: bool) {
        if !is_journaled(tool) {
            return;
        }
        if let Err(error) = self.directory.effect_finished(session, call_id, is_error) {
            tracing::warn!(%error, %call_id, "could not journal a tool result");
        }
    }

    pub(super) fn is_suspect(&self, session: SessionId, call_id: CallId) -> bool {
        self.suspects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&(session, call_id))
    }
}

/// Wraps a declaration so a call a crash may have run is refused.
///
/// The journal knows the call id, and the engine hands it to the executor in
/// the context, so the gate belongs beside the call rather than in a rule the
/// engine evaluates before it: a call that started before a restart is a fact
/// about *this* call, not a property of the tool.
pub(super) fn journalled<A>(
    declaration: norte::ToolDeclaration<A>,
    journal: Arc<EffectJournal>,
) -> norte::ToolDeclaration<A>
where
    A: norte::ToolArgs + serde::de::DeserializeOwned + std::fmt::Debug + 'static,
{
    let name = declaration.name;
    norte::ToolDeclaration {
        name,
        description: declaration.description,
        parameters: declaration.parameters,
        executor: Arc::new(JournalledExecutor {
            inner: declaration.executor,
            journal,
            name,
        }),
    }
}

/// A tool executor that refuses a call the journal doubts.
struct JournalledExecutor<A> {
    inner: Arc<dyn norte::FnExecutor<A, norte::ToolOutput>>,
    journal: Arc<EffectJournal>,
    name: &'static str,
}

#[async_trait::async_trait]
impl<A> norte::FnExecutor<A, norte::ToolOutput> for JournalledExecutor<A>
where
    A: norte::ToolArgs,
{
    async fn call(&self, args: A) -> norte::Result<norte::ToolOutput> {
        self.inner.call(args).await
    }

    async fn call_with_context(
        &self,
        context: &norte::ToolContext,
        args: A,
    ) -> norte::Result<norte::ToolOutput> {
        if let (Some(session), Some(call_id)) = (context.session, context.call_id) {
            if is_journaled(self.name) && self.journal.is_suspect(session, call_id) {
                return Ok(norte::ToolOutput::error(format!(
                    "`{}` call {call_id} started before a restart and may already have taken \
                     effect, so it is not run again. Inspect the workspace; to retry, ask in a \
                     new turn so the model gets a fresh call.",
                    self.name
                )));
            }
        }
        self.inner.call_with_context(context, args).await
    }
}
