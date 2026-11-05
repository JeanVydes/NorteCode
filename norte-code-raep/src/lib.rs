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

pub use host::{FileReplayStore, HostConfig, HostError, NorteCodeRaepHost, ReplayStore};
pub use http::router;

/// Extensions owned by the `NorteCode` server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NorteCodeExtension {
    /// Carries a complete `NorteCode` [`norte_code_protocol::ClientRequest`].
    Command,
}

impl NorteCodeExtension {
    /// Returns the namespaced wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Command => "norte_code.command",
        }
    }
}
