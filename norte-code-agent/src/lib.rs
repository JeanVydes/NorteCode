//! The Norte Code service.
//!
//! Owns the Norte engines, the agent loop, policies, layers, the session
//! manager, compaction and project instructions. The API is async: a client
//! submits [`norte_code_protocol`] requests and receives a stream of events.
//! It is transport-agnostic; v1 ships only the in-process tokio transport.

#![forbid(unsafe_code)]
