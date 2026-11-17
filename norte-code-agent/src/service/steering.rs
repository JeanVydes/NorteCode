//! Live steering: a message that reaches the request already in flight.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::runtime::{ConsultationPoint, Interception, InterceptionContext, TurnInterceptor};
use norte::domain::part::Part;
use norte::{MediaPart, Message, Result, SessionId};
use norte_code_wire::Attachment;
use serde::{Deserialize, Deserializer, Serialize};

/// A steering message on its way to the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Steered {
    /// The name the client gave the message, or the one the service made.
    pub(crate) id: String,
    /// The text the model reads.
    pub(crate) text: String,
    /// The media parts the model reads, resolved before the message waited.
    ///
    /// The interceptor does not hold the blob store, so the service resolves
    /// the attachments while the turn the message joins is still running.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) parts: Vec<MediaPart>,
    /// The attachments behind the parts, for a message that starts a turn
    /// instead of joining one: that turn resolves them itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) attachments: Vec<Attachment>,
}

impl Steered {
    /// A message with a name.
    pub(crate) fn new(
        id: String,
        text: String,
        parts: Vec<MediaPart>,
        attachments: Vec<Attachment>,
    ) -> Self {
        Self {
            id,
            text,
            parts,
            attachments,
        }
    }
}

impl<'de> Deserialize<'de> for Steered {
    /// Reads a message, accepting the shapes where media was not carried.
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Shape {
            Named {
                id: String,
                text: String,
                #[serde(default)]
                parts: Vec<MediaPart>,
                #[serde(default)]
                attachments: Vec<Attachment>,
            },
            Text(String),
        }
        Ok(match Shape::deserialize(deserializer)? {
            Shape::Named {
                id,
                text,
                parts,
                attachments,
            } => Self {
                id,
                text,
                parts,
                attachments,
            },
            Shape::Text(text) => Self {
                id: String::new(),
                text,
                parts: Vec::new(),
                attachments: Vec::new(),
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
pub(crate) struct Steering {
    /// Waiting for the engine to take them.
    pending: Mutex<BTreeMap<SessionId, VecDeque<Steered>>>,
    /// Handed to the engine, waiting for the report that they were applied.
    applied: Mutex<BTreeMap<SessionId, VecDeque<Steered>>>,
}

impl Steering {
    /// Queues a message for the session. The next turn of that session takes
    /// it, before its own input.
    pub(crate) fn push(&self, session: SessionId, message: Steered) {
        self.pending()
            .entry(session)
            .or_default()
            .push_back(message);
    }

    /// Whether the session is already waiting for this message.
    pub(crate) fn is_pending(&self, session: SessionId, id: &str) -> bool {
        self.pending()
            .get(&session)
            .is_some_and(|queue| queue.iter().any(|message| message.id == id))
    }

    /// Takes the messages the engine has not reached, oldest first.
    pub(crate) fn take(&self, session: SessionId) -> Vec<Steered> {
        let taken: VecDeque<Steered> = self.pending().remove(&session).unwrap_or_default();
        if !taken.is_empty() {
            self.applied_queue()
                .entry(session)
                .or_default()
                .extend(taken.iter().cloned());
        }
        taken.into_iter().collect()
    }

    /// Takes one pending message by name, without the applied bookkeeping.
    ///
    /// The service calls this when it reclaims a message no turn will apply:
    /// the message starts its own turn instead of joining one, so it never
    /// reaches the applied queue.
    pub(crate) fn take_if_pending(&self, session: SessionId, id: &str) -> Option<Steered> {
        let mut pending = self.pending();
        let queue = pending.get_mut(&session)?;
        let index = queue.iter().position(|message| message.id == id)?;
        let message = queue.remove(index);
        if queue.is_empty() {
            pending.remove(&session);
        }
        message
    }

    /// The messages the engine reported as applied, oldest first.
    pub(crate) fn applied(&self, session: SessionId, count: usize) -> Vec<Steered> {
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
pub(crate) struct SteeringInterceptor {
    steering: Arc<Steering>,
}

impl SteeringInterceptor {
    /// Builds an interceptor that reads the given steering state.
    pub(crate) fn new(steering: Arc<Steering>) -> Self {
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
        // there is dropped: the queue it came from loses it. Waiting for the
        // request that follows is what a steering message does anyway.
        if matches!(context.point, ConsultationPoint::ToolCall { .. }) {
            return Ok(Interception::Continue);
        }
        let messages: Vec<Message> = self
            .steering
            .take(context.session)
            .into_iter()
            .map(|steered| {
                let mut message = Message::user(steered.text);
                message
                    .parts
                    .extend(steered.parts.into_iter().map(Part::Media));
                message
            })
            .collect();
        if messages.is_empty() {
            Ok(Interception::Continue)
        } else {
            Ok(Interception::Inject(messages))
        }
    }
}

#[cfg(test)]
#[path = "steering_tests.rs"]
mod tests;
