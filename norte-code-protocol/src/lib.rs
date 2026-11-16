//! The Norte Code client protocol.
//!
//! Versioned serde types for the Norte Code client wire. The service maps
//! between these types and the engine, so a client does not compile the
//! engine and the wire can move on its own schedule. This crate still
//! depends on Norte for catalog capability mapping and adjacent adapters;
//! those engine types are kept out of the wire structs and mapped in one
//! place, so the crate is not engine-free until that dependency is removed.
//!
//! The two halves of the wire are [`ClientRequest`] and [`ServerEvent`]. Every
//! server event carries the request it answers and, when it is about one, the
//! session it belongs to.

#![forbid(unsafe_code)]

pub mod agent_commands;
pub mod catalog;
pub mod config;
pub mod costs;
pub mod keys;
pub mod plans;
pub mod skills;
pub mod sqlite;
pub mod store;
pub mod tools;

pub use agent_commands::AgentCommand;
pub use store::*;

/// The protocol revision. It is bumped when a type changes shape in a way an
/// older client cannot read.
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
#[path = "src_tests.rs"]
mod tests;
