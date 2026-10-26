//! Where a running shell call's detach token is published.
//!
//! A shell call is the one thing a turn can be stuck on, and cancelling the turn
//! is a blunt answer: it kills the process. Detaching is the other answer — the
//! call returns at once with what has arrived, the process keeps running, and the
//! turn carries on with the partial output.
//!
//! Norte gives a tool the means: `ToolContext::detached` is a token a host
//! signals from outside the call, and `norte-tools` watches it and answers early.
//! What Norte does not do is *hold* that token anywhere a client can reach — the
//! engine builds the context and runs the call in one move. This registry is the
//! missing half, and [`super::background::ShellCalls`] is what mints a token per
//! call and publishes it here.
//!
//! A call whose session is unknown has nowhere to be reached from, so it simply
//! runs unwrapped.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use norte::{Cancellation, SessionId};

/// The detach token of the call each session's turn is waiting on.
///
/// One per session: a turn runs its calls in order, so the call a client would
/// want to detach is the one the session is on. A later call replaces an earlier
/// one, and only the call that published a token may remove it.
#[derive(Default)]
pub(super) struct DetachRegistry {
    running: Mutex<BTreeMap<SessionId, Arc<Cancellation>>>,
}

impl DetachRegistry {
    /// Asks the session's running call to stop waiting.
    ///
    /// Answers whether there was a call to ask: a session whose turn is between
    /// calls has none, and that is not an error — the keystroke simply had
    /// nothing to reach.
    pub(super) fn detach(&self, session: SessionId) -> bool {
        match self.lock().get(&session).cloned() {
            Some(token) if !token.is_cancelled() => {
                token.cancel();
                true
            }
            _ => false,
        }
    }

    pub(super) fn attach(&self, session: SessionId, token: Arc<Cancellation>) {
        self.lock().insert(session, token);
    }

    /// Forgets the session's token, unless a later call already replaced it.
    pub(super) fn release(&self, session: SessionId, token: &Arc<Cancellation>) {
        let mut running = self.lock();
        if running
            .get(&session)
            .is_some_and(|held| Arc::ptr_eq(held, token))
        {
            running.remove(&session);
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<SessionId, Arc<Cancellation>>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
