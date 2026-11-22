//! Typed shapes shared by plugin tools and the agent host.
//!
//! The envelope lives here so neither side hand-builds JSON inside a tool
//! result. Norte keeps no knowledge of it.

#![forbid(unsafe_code)]

mod host_effects;
mod user_message_draft;

pub use host_effects::{HOST_EFFECTS_KEY, HostEffects};
pub use user_message_draft::UserMessageDraft;

#[cfg(test)]
#[path = "host_effects_tests.rs"]
mod host_effects_tests;
#[cfg(test)]
#[path = "user_message_draft_tests.rs"]
mod user_message_draft_tests;
