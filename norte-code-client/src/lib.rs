//! Transport-neutral client contract for Norte Code.
//!
//! The client is expressed in terms of `norte-code-wire` requests and events,
//! and it is the only place that turns RAEP frames into those events. Every
//! client — a native process or a wasm UI — decodes through [`decode_frame`]
//! and [`take_sse_events`]; only the transport differs, and that is the one
//! thing a platform has to supply.
//!
//! The implementation is split by what it is made of: the frames a server
//! sends ([`frame`]), the canonical payloads inside them ([`canonical`]), the
//! mapping onto wire events ([`decode`]), the SSE reader ([`sse`]), the error
//! type ([`error`]), the trait every adapter implements ([`agent`]), and the
//! HTTP adapter itself, compiled only with the `native` feature ([`remote`]).

#![forbid(unsafe_code)]

pub(crate) mod agent;
pub mod api;
pub(crate) mod canonical;
pub(crate) mod decode;
pub(crate) mod error;
pub(crate) mod frame;
#[cfg(feature = "native")]
pub(crate) mod remote;
pub(crate) mod sse;

pub use agent::{AgentClient, EventStream, SessionSummary, into_session_info};
pub use api::{
    AgentModelRef, AgentOverride, ApiError, CommandResponse, CreateSessionRequest, Endpoint,
    RemoteError, RenameSessionRequest, RestClient, TelemetryQuery, now_millis,
};
pub use decode::{DecodedEvent, decode_frame, history_items};
pub use error::ClientError;
#[cfg(feature = "native")]
pub use remote::{RemoteClient, RemoteSessionStats};
pub use sse::take_sse_events;

#[cfg(test)]
mod tests;
