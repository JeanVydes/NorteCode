//! The shapes that travel on the Norte Code wire.
//!
//! Every client decodes the same JSON: the terminal, the browser and the SDK.
//! Keeping the shapes in one crate — with no socket, no database and no
//! runtime — is what stops each of them from growing its own copy of
//! `JobState`, of `SessionSummary`, or of the event enum, and from disagreeing
//! about a state the moment the server adds one.
//!
//! What lives here is *only* the data: serde derives, defaults, and the closed
//! sets the protocol is made of. The server's own machinery (store, config,
//! catalog files) stays in `norte-code-protocol`.

pub mod catalog;
pub mod event;
pub mod payload;
pub mod resource;
pub mod types;
pub mod wire;

pub use event::*;
pub use payload::*;
pub use resource::*;
pub use types::*;
pub use wire::*;

use serde::{Deserialize, Serialize};

/// What a new input does when the session already has a turn running.
///
/// RAEP's `USER_INPUT.queue_policy` is the wire name for this. RAEP calls it
/// `QueuePolicy` and spells the values `reject`, `queue` and `interrupt`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
pub enum QueueMode {
    /// Refuse the input while a turn is active (RAEP's default).
    #[default]
    Reject,
    /// Enqueue the input for the turn already in flight.
    Queue,
    /// Cancel the running turn, then enqueue the input.
    Interrupt,
}

/// The lifecycle state of one plan.
///
/// It lives here and not with the plan's Markdown parser: it is a value that
/// travels on the wire, while the parser is the server's file access.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
pub enum PlanStatus {
    /// The user approved this plan.
    Active,
    /// The plan is awaiting the user's decision.
    #[default]
    Pending,
    /// The user rejected or canceled this plan.
    Canceled,
}
