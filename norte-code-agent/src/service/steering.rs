//! Live steering: a message that reaches the request already in flight.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use norte::harness::{ConsultationPoint, Interception, InterceptionContext, TurnInterceptor};
use norte::{Message, Result, SessionId};
use serde::{Deserialize, Deserializer, Serialize};

/// A steering message on its way to the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct Steered {
    /// The name the client gave the message, or the one the service made.
    pub(super) id: String,
    /// The text the model reads.
    pub(super) text: String,
}

impl Steered {
    /// A message with a name.
    pub(super) fn new(id: String, text: String) -> Self {
        Self { id, text }
    }

    /// The same message under another name.
    pub(super) fn with_id(self, id: String) -> Self {
        Self { id, ..self }
    }
}

impl<'de> Deserialize<'de> for Steered {
    /// Reads a message, accepting the older shape where it was only text.
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Shape {
            Named { id: String, text: String },
            Text(String),
        }
        Ok(match Shape::deserialize(deserializer)? {
            Shape::Named { id, text } => Self { id, text },
            Shape::Text(text) => Self {
                id: String::new(),
                text,
            },
        })
    }
}

/// The steering messages the running turns of the sessions have not reached.
///
/// The interceptor owns this state: the service hands it the messages and the
/// engine takes them at the safe points of a turn. Nothing else has to know
/// whether a turn is running.
#[derive(Debug, Default)]
pub(super) struct Steering {
    /// Waiting for the engine to take them.
    pending: Mutex<BTreeMap<SessionId, VecDeque<Steered>>>,
    /// Handed to the engine, waiting for the report that they were applied.
    applied: Mutex<BTreeMap<SessionId, VecDeque<Steered>>>,
}

impl Steering {
    /// Queues a message for the session. The next turn of that session takes
    /// it, before its own input.
    pub(super) fn push(&self, session: SessionId, message: Steered) {
        self.pending()
            .entry(session)
            .or_default()
            .push_back(message);
    }

    /// Whether the session is already waiting for this message.
    pub(super) fn is_pending(&self, session: SessionId, id: &str) -> bool {
        self.pending()
            .get(&session)
            .is_some_and(|queue| queue.iter().any(|message| message.id == id))
    }

    /// Takes the messages the engine has not reached, oldest first.
    pub(super) fn take(&self, session: SessionId) -> Vec<Steered> {
        let taken: VecDeque<Steered> = self.pending().remove(&session).unwrap_or_default();
        if !taken.is_empty() {
            self.applied_queue()
                .entry(session)
                .or_default()
                .extend(taken.iter().cloned());
        }
        taken.into_iter().collect()
    }

    /// The messages the engine reported as applied, oldest first.
    pub(super) fn applied(&self, session: SessionId, count: usize) -> Vec<Steered> {
        let mut applied = self.applied_queue();
        let Some(queue) = applied.get_mut(&session) else {
            return Vec::new();
        };
        let count = count.min(queue.len());
        queue.drain(..count).collect()
    }

    fn pending(&self) -> MutexGuard<'_, BTreeMap<SessionId, VecDeque<Steered>>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn applied_queue(&self) -> MutexGuard<'_, BTreeMap<SessionId, VecDeque<Steered>>> {
        self.applied.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The interceptor the engine consults before every request of a turn.
///
/// The engine commits what this returns, so the message lands in the history,
/// in the request digest and in what the policies read, and it reaches the
/// request that is already in flight instead of waiting for the next turn.
pub(super) struct SteeringInterceptor {
    steering: Arc<Steering>,
}

impl SteeringInterceptor {
    /// Builds an interceptor that reads the given steering state.
    pub(super) fn new(steering: Arc<Steering>) -> Self {
        Self { steering }
    }
}

#[async_trait::async_trait]
impl TurnInterceptor for SteeringInterceptor {
    fn name(&self) -> &'static str {
        "steering"
    }

    async fn intercept(&self, context: &InterceptionContext<'_>) -> Result<Interception> {
        // The engine asks about one call at `ToolCall`, and a message injected
        // there is dropped — the queue it came from loses it. Waiting for the
        // request that follows is what a steering message does anyway.
        if matches!(context.point, ConsultationPoint::ToolCall { .. }) {
            return Ok(Interception::Continue);
        }
        let messages: Vec<Message> = self
            .steering
            .take(context.session)
            .into_iter()
            .map(|steered| Message::user(steered.text))
            .collect();
        if messages.is_empty() {
            Ok(Interception::Continue)
        } else {
            Ok(Interception::Inject(messages))
        }
    }
}
