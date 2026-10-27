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
    /// The provider is answering.
    Streaming,
}
