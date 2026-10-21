//! Live steering: a message that reaches the request already in flight.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use norte::{InterceptionContext, Message, Result, TurnInterceptor};

/// The inbox a running turn reads its steering messages from.
///
/// The queue of the session is a durable field, so a message written to it
/// while a turn runs cannot be committed without racing that turn. This inbox
/// is the other half: the running turn takes the message from here and commits
/// it itself, and the service keeps the durable copy until the engine says it
/// applied it.
#[derive(Debug, Default)]
pub(super) struct SteerInbox {
    messages: Mutex<VecDeque<String>>,
}

impl SteerInbox {
    /// Queues a message for the next request of the running turn.
    pub(super) fn push(&self, text: String) {
        self.lock().push_back(text);
    }

    /// Takes the queued messages, oldest first.
    pub(super) fn take(&self) -> Vec<String> {
        self.lock().drain(..).collect()
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<String>> {
        self.messages.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The interceptor the engine consults before every request of a turn.
///
/// The engine commits what this returns, so the message lands in the history,
/// in the request digest and in what the policies read, and it reaches the
/// request that is already in flight instead of waiting for the next turn.
pub(super) struct SteeringInterceptor {
    inbox: Arc<SteerInbox>,
}

impl SteeringInterceptor {
    /// Builds an interceptor that reads the given inbox.
    pub(super) fn new(inbox: Arc<SteerInbox>) -> Self {
        Self { inbox }
    }
}

#[async_trait::async_trait]
impl TurnInterceptor for SteeringInterceptor {
    fn name(&self) -> &'static str {
        "steering"
    }

    async fn intercept(&self, _context: &InterceptionContext<'_>) -> Result<Vec<Message>> {
        Ok(self.inbox.take().into_iter().map(Message::user).collect())
    }
}
