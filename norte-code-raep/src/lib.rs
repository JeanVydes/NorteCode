//! RAEP server integration for Norte Code.
//!
//! This crate is the server-side migration boundary. It owns no model,
//! tool, session or persistence policy: those remain in
//! [`norte_code_agent::Service`]. It translates RAEP frames into the existing
//! Norte Code service protocol and translates service events back into RAEP
//! envelopes for any client transport.

#![forbid(unsafe_code)]

mod host;
mod http;

pub use host::{HostConfig, HostError, NorteCodeRaepHost};
pub use http::router;
