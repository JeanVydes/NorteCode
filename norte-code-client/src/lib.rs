//! Transport-neutral client contract for Norte Code.
//!
//! The client is deliberately expressed in terms of `norte-code-wire` requests
//! and events. An in-process adapter and a remote RAEP adapter can therefore be
//! used by the same CLI without moving the agent loop into the client.
//!
//! The implementation is split by what it is made of: the frames a server
//! sends ([`frame`]), the canonical payloads inside them ([`canonical`]), the
//! mapping onto wire events ([`decode`]), the SSE sequence bookkeeping
//! ([`sse`]), the error type ([`error`]), the trait every adapter implements
//! ([`agent`]), and the HTTP adapter itself ([`remote`]).

#![forbid(unsafe_code)]

pub(crate) mod agent;
pub(crate) mod canonical;
pub(crate) mod decode;
pub(crate) mod error;
pub(crate) mod frame;
pub(crate) mod remote;
pub(crate) mod sse;

pub use agent::{AgentClient, EventStream, SessionSummary};
pub use error::ClientError;
pub use remote::{RemoteClient, RemoteSessionStats};

#[allow(clippy::wildcard_imports)]
pub(crate) use canonical::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use decode::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use frame::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use sse::*;

#[cfg(test)]
mod tests;
