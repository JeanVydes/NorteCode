//! Host hooks the engine consults at the safe points of a running turn.
//!
//! One seam, not several. Everything a host decides while a turn runs — what
//! the conversation gets, whether the turn goes on, whether one tool call may
//! run — is decided here, on a context that names the point being decided at.

use std::sync::Arc;

use norte::domain::message::Message;
use norte::domain::part::ToolCallPart;
use norte::error::Result;
use norte::id::{CallId, SessionId};
use serde_json::Value;

use crate::runtime::event::EventSink;

/// Where in a turn the engine is asking the host.
///
/// A host that decides at one point only ignores the others, and the engine
/// names the point rather than making the host infer it from the shape of the
/// history: the same conversation looks different at the end of a turn and
/// just before a request, and a rule that had to tell them apart from the last
/// message's role is a rule that breaks as soon as the loop grows a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsultationPoint {
    /// Before the turn's own input joins the session.
    PreInput,
    /// Before a model request is built.
    PreflightRequest,
    /// Before one tool call the model asked for runs.
    ToolCall {
        /// The call being decided about.
        call: ToolCallPart,
    },
    /// After the tool calls of one iteration ran, before the next request.
    PostTools,
    /// At the end of a turn, before the engine lets it finish.
    EndOfTurn,
}

impl ConsultationPoint {
    /// The call being decided about, when this is a tool call.
    ///
    /// `None` at every other point, so a host that decides at one place writes
    /// one `let` instead of a match over the point and a rebuilt question.
    #[must_use]
    pub fn call(&self) -> Option<&ToolCallPart> {
        match self {
            ConsultationPoint::ToolCall { call } => Some(call),
            ConsultationPoint::PreInput
            | ConsultationPoint::PreflightRequest
            | ConsultationPoint::PostTools
            | ConsultationPoint::EndOfTurn => None,
        }
    }
}

/// What an interceptor may rely on while a turn runs.
pub struct InterceptionContext<'a> {
    /// Where in the turn the engine is asking.
    pub point: ConsultationPoint,
    /// The session the turn belongs to.
    pub session: SessionId,
    /// The turn number.
    pub turn: u64,
    /// The head the next request is built from.
    pub head: Option<norte::id::NodeId>,
    /// The history the next request carries, oldest first.
    ///
    /// This is what the model will see, so an interceptor can decide on the
    /// conversation itself: the call the model just asked for, the tool that
    /// answered it, the instruction the human gave.
    pub messages: &'a [Message],
    /// The sink an interceptor uses to emit engine events.
    pub events: &'a dyn EventSink,
}

impl InterceptionContext<'_> {
    /// The call being decided about, when there is one.
    #[must_use]
    pub fn call(&self) -> Option<&ToolCallPart> {
        self.point.call()
    }

    /// The name of the tool being decided about.
    #[must_use]
    pub fn tool_name(&self) -> Option<&str> {
        self.call().map(|call| call.name.as_str())
    }

    /// The id of the call being decided about.
    #[must_use]
    pub fn call_id(&self) -> Option<CallId> {
        self.call().map(|call| call.call_id)
    }

    /// The arguments the call would run with.
    #[must_use]
    pub fn arguments(&self) -> Option<&Value> {
        self.call().map(|call| &call.arguments)
    }
}

/// What an interceptor decided about the turn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Interception {
    /// The turn goes on as it would.
    #[default]
    Continue,
    /// These messages join the session before the next request.
    Inject(Vec<Message>),
    /// The turn ends here.
    Stop {
        /// Why the turn ends. The host reports it under its own namespace.
        reason: String,
    },
    /// The call being decided about does not run; the model is told why.
    ///
    /// This is the answer at [`ConsultationPoint::ToolCall`], where the engine
    /// has not run anything yet and a refusal is a message to the model: the
    /// call the model asked for is answered with an error result it reads like
    /// any other failed call, and it can adapt. Elsewhere it is a mistake —
    /// there is no call to refuse — and the engine treats it as a stop.
    RefuseCall {
        /// Why the call does not run. The model reads this.
        reason: String,
    },
}

/// A host hook the engine consults at the safe points of a running turn.
///
/// There are four: before the turn's input, before every model request, before
/// every tool call, and at the end of the turn.
/// [`InterceptionContext::point`](ConsultationPoint) says which, and
/// [`Interception::Continue`] is the right answer at three of them for a host
/// that decides at one.
///
/// An interceptor may return messages to append to the session's history. The
/// engine appends them itself, so they are durable, part of the request
/// digest, visible to the policies, and free of a version race: while a turn
/// runs, the engine is the only writer.
///
/// This is the seam a steering message takes when it must reach the request
/// that is already in flight. A [`Layer`](norte::layer::Layer) cannot do it: a
/// layer sees the request after it was built, so what it adds is neither
/// durable nor visible to the policies.
#[async_trait::async_trait]
pub trait TurnInterceptor: Send + Sync {
    /// The interceptor's name, as it appears in the session metadata.
    fn name(&self) -> &str;

    /// Decides what the turn does next.
    ///
    /// # Errors
    ///
    /// Returns an error when the host cannot decide. The turn then fails with
    /// it and the session stays resumable.
    async fn intercept(&self, context: &InterceptionContext<'_>) -> Result<Interception>;

    /// Called when an error occurs during the turn. The host may inject messages
    /// or stop the turn; the default lets the turn fail with the error.
    ///
    /// # Errors
    ///
    /// Returns an error when the host cannot decide.
    async fn on_error(
        &self,
        _context: &InterceptionContext<'_>,
        _error: &norte::error::Error,
    ) -> Result<Interception> {
        Ok(Interception::Continue)
    }
}

/// The interceptors an engine consults, in registration order.
#[derive(Clone, Default)]
pub struct InterceptorSet {
    interceptors: Vec<Arc<dyn TurnInterceptor>>,
}

impl InterceptorSet {
    /// Creates a set from the given interceptors.
    #[must_use]
    pub fn new(interceptors: Vec<Arc<dyn TurnInterceptor>>) -> Self {
        Self { interceptors }
    }

    /// The interceptors, in registration order.
    #[must_use]
    pub fn interceptors(&self) -> &[Arc<dyn TurnInterceptor>] {
        &self.interceptors
    }

    /// The names of the interceptors, for the session metadata.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.interceptors
            .iter()
            .map(|interceptor| interceptor.name())
            .collect()
    }

    /// Whether the set holds no interceptor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.interceptors.is_empty()
    }
}
