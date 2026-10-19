//! The Norte Code service.
//!
//! Owns the Norte engines, the agent loop, policies, layers, the session
//! manager, compaction and project instructions. The API is async: a client
//! submits [`norte_code_protocol`] requests and receives a stream of events.
//! It is transport-agnostic; v1 ships only the in-process tokio transport.

#![forbid(unsafe_code)]

pub mod error;
pub mod provider;
pub mod service;

pub use error::ServiceError;
pub use provider::{ModelFactory, ModelSpec, VendorModels, capabilities};
pub use service::{EventStream, Service};

/// The system prompt a session starts with until project instructions
/// (milestone 8) replace it.
pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are Norte Code, a coding agent working in the user's project. Use the \
tools available to you instead of describing what you would do. Read a file \
before you change it. Keep changes minimal and consistent with the code \
around them. When you are done, say what you changed.";
