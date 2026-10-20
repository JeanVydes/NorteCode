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
pub mod telemetry;

pub use error::ServiceError;
pub use provider::{ModelFactory, ModelSpec, VendorModels, Wire};
pub use service::{EventStream, PromptSize, Service, effective_reasoning, model_list};

/// The system prompt a session starts with until project instructions
/// (milestone 8) replace it.
pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are Norte Code, a coding agent in the user's repository.

Act with tools; do not only describe intended actions. Prefer read, glob, \
and grep over shell when they are enough. Batch independent reads. Do not \
re-read a file or re-run a command unless the result was incomplete, wrong, \
or the file may have changed. Read a file before editing it. Keep diffs \
minimal and consistent with surrounding style.

Match the user's requested length and format. If they ask for a short \
answer, stay short. When finished, state what changed (or that nothing \
changed) in a few sentences unless they ask for more.";
