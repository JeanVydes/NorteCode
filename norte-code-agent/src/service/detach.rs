//! Detaching the running call: stop waiting for a shell, without killing it.
//!
//! A waited `shell` call is the one thing a turn can be stuck on, and cancelling
//! the turn is a blunt answer: it kills the process. Detaching is the other
//! answer — the call returns at once with what has arrived, the process keeps
//! running, and the turn carries on with the partial output.
//!
//! Norte already gives a tool the means: [`ToolContext::detached`] is a token a
//! host signals from outside the call, and `norte-tools` watches it, disarms its
//! process-group guard and answers early. What Norte does not do on its own is
//! *hold* that token anywhere a client can reach — the engine builds the context
//! and runs the call in one move. So this is the missing half: a wrapper around
//! the shell's executor that mints a token per call, publishes it here, and
//! lets the client signal it by session.
//!
//! Only the foreground `shell` is wrapped. `shell_bg` needs no detaching (it
//! answers at once anyway), and a call whose session is unknown has nowhere to
//! be reached from, so it simply runs unwrapped.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use norte::{
    Cancellation, FnExecutor, Result, SessionId, ToolContext, ToolDeclaration, ToolOutput,
};
use norte_tools::ShellRequest;

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

    fn attach(&self, session: SessionId, token: Arc<Cancellation>) {
        self.lock().insert(session, token);
    }

    /// Forgets the session's token, unless a later call already replaced it.
    fn release(&self, session: SessionId, token: &Arc<Cancellation>) {
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

/// The shell's executor, with a detach token a client can signal.
pub(super) struct Detachable {
    /// The executor being wrapped, with the host's limits already applied.
    inner: Arc<dyn FnExecutor<ShellRequest, ToolOutput>>,
    /// Where the running call's token is published.
    detaches: Arc<DetachRegistry>,
}

impl Detachable {
    /// Wraps `declaration`, publishing a detach token for each of its calls.
    pub(super) fn declaration(
        declaration: ToolDeclaration<ShellRequest>,
        detaches: Arc<DetachRegistry>,
    ) -> ToolDeclaration<ShellRequest> {
        ToolDeclaration {
            name: declaration.name,
            description: declaration.description,
            parameters: declaration.parameters,
            executor: Arc::new(Self {
                inner: declaration.executor,
                detaches,
            }),
            approval: declaration.approval,
        }
    }
}

#[async_trait]
impl FnExecutor<ShellRequest, ToolOutput> for Detachable {
    async fn call(&self, request: ShellRequest) -> Result<ToolOutput> {
        self.inner.call(request).await
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        request: ShellRequest,
    ) -> Result<ToolOutput> {
        // Without a session there is no one to signal the token, so the call
        // runs exactly as it would unwrapped.
        let Some(session) = context.session else {
            return self.inner.call_with_context(context, request).await;
        };
        let token = Arc::new(Cancellation::new());
        self.detaches.attach(session, Arc::clone(&token));
        // The turn's own cancellation is left alone: detaching must not make a
        // later Ctrl-C stop reaching the call.
        let scoped = context.clone().with_detach((*token).clone());
        let outcome = self.inner.call_with_context(&scoped, request).await;
        self.detaches.release(session, &token);
        outcome
    }
}
