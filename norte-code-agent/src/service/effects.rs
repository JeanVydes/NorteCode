//! Effect journal: refuse to replay a call a crash may have already run.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use norte::harness::{Decision, Policy, PolicyView};
use norte::{CallId, SessionId};
use norte_code_store_sqlite::SessionDirectory;

use crate::ServiceError;

/// The tools whose effects are not safely repeatable after a crash.
const JOURNALED_TOOLS: [&str; 4] = ["write", "edit", "command", "skill_script"];

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

/// Refuses to replay an effectful call a crashed process may have run.
pub(super) struct EffectJournalPolicy {
    pub(super) journal: Arc<EffectJournal>,
}

impl Policy for EffectJournalPolicy {
    fn name(&self) -> &'static str {
        "effect-journal"
    }

    fn decide(&self, view: &PolicyView<'_>) -> Decision {
        let tool = view.call.name.as_str();
        if is_journaled(tool) && self.journal.is_suspect(view.session.id, view.call.call_id) {
            return Decision::Deny(format!(
                "`{tool}` call {} started before a restart and may already have taken effect, \
                 so it is not run again. Inspect the workspace; to retry, ask in a new turn \
                 so the model gets a fresh call.",
                view.call.call_id
            ));
        }
        Decision::Allow
    }
}
