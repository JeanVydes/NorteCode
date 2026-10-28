//! What the client is waiting for, while a turn is in flight.
//!
//! It used to be a bar that repainted a line of its own from a thread of its
//! own. That thread was a second writer on a terminal that can only have one:
//! it cleared lines the frame was drawing and drew lines the frame then cleared,
//! which is what made the footer unreadable.
//!
//! What is left is the state and nothing else. The client works out which wait
//! it is from the events it receives — nothing arrives between the turn
//! starting and the first delta, so the absence of a delta is what "waiting for
//! the first token" means — and the frame draws it.

/// What the turn is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
    /// The request is on its way to the provider.
    Waiting,
    /// The provider is thinking: reasoning is arriving, and no answer text yet.
    ///
    /// It is its own phase because the two are different waits for the person
    /// watching: a model that is thinking has not started to answer, and calling
    /// that "streaming" says the answer is on its way when the thought is.
    Thinking,
    /// The provider is answering: answer text is arriving.
    Streaming,
}
