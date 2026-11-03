//! The runtime's durable data model: the session, the branch graph it grows
//! into, and the document that exports one.
//!
//! These live here rather than in the core because they exist *because* there
//! is a turn loop. A provider receives a `ModelRequest` — the conversation as a
//! list of messages, already flattened — and never sees a branch head, a
//! version, a phase or a grant.

/// One atomic change to a session.
pub mod commit;
/// The portable document used by export and import.
pub mod conversation;
/// The durable nodes a session's branch is made of.
pub mod node;
/// Durable conversation state.
pub mod session;
