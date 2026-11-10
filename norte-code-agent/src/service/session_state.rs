//! One of the service's domains, as `impl Service` blocks.
//!
//! Child modules see the service's private fields, so a domain owns its
//! behaviour here while the state stays in one place.

use std::sync::PoisonError;

use norte_code_wire::ContextUsage;
use norte_code_wire::{Reasoning, SessionKey};

use crate::provider::ModelSpec;

#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    /// The `provider/model` reference a session is on, if it is known.
    ///
    /// A client needs it to change one half of the reference while keeping the
    /// other: `/provider` swaps the provider and keeps the model, and it cannot
    /// read that from anywhere of its own while a turn is running.
    #[must_use]
    pub fn session_model(&self, session: &SessionKey) -> Option<String> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.spec.as_ref().map(ModelSpec::label))
    }
    /// The model a session is on right now, with its capabilities.
    ///
    /// The whole spec and not just its label: `/debug` reports what the model in
    /// play can do, and after a `/model` those are the new model's capabilities,
    /// not the ones the process started with.
    #[must_use]
    pub fn session_spec(&self, session: &SessionKey) -> Option<ModelSpec> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.spec.clone())
    }
    /// The turn currently running for a session, if one has started.
    #[must_use]
    pub fn session_active_turn_id(&self, session: &SessionKey) -> Option<u64> {
        self.active_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
            .copied()
    }
    /// How much of its model's context a session is using.
    ///
    /// The used part is what the last request carried: the whole
    /// conversation goes again every time: and the window is what the
    /// catalogue says the model holds. A session that has made no request yet
    /// has used nothing.
    #[must_use]
    pub fn session_context(&self, session: &SessionKey) -> ContextUsage {
        self.lock_sessions()
            .get(session)
            .map_or_else(ContextUsage::default, |record| ContextUsage {
                used: record.context_used,
                window: match &record.spec {
                    Some(s) => s.capabilities.max_context,
                    None => None,
                },
            })
    }
    /// The reasoning override a session carries, if it has one.
    ///
    /// `None` is not "no reasoning": it means the session set none, so the
    /// answer is the config's default as the model resolves it: a different
    /// thing, because a model that cannot reason turns `auto` into `off`.
    #[must_use]
    pub fn session_reasoning(&self, session: &SessionKey) -> Option<Reasoning> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.reasoning.clone())
    }
    /// The title a session has, if one was set.
    ///
    /// `None` means the session is still known by its key alone. The renderer
    /// reads this so the footer can show the title instead of the key.
    #[must_use]
    pub fn session_title(&self, session: &SessionKey) -> Option<String> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.title.clone())
    }
    /// Whether the session has a background job still running.
    ///
    /// The CLI asks before deciding what `Ctrl-C` means: it ends a job when
    /// there is one and does nothing otherwise. A plain `bool` rather than a
    /// borrow, so the answer does not hold the sessions lock across a redraw.
    #[must_use]
    pub fn has_running_jobs(&self, session: &SessionKey) -> bool {
        self.lock_sessions()
            .get(session)
            .is_some_and(|record| self.jobs.any_running(record.id))
    }
}
