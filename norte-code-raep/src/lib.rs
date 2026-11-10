//! RAEP server integration for Norte Code.
//!
//! This crate is the server-side migration boundary. It owns no model,
//! tool, session or persistence policy: those remain in
//! [`norte_code_agent::Service`]. It translates RAEP frames into the existing
//! Norte Code service protocol and translates service events back into RAEP
//! envelopes for any client transport.

#![forbid(unsafe_code)]

pub mod bundle;
mod commands;
mod host;
mod http;
mod payloads;
mod plugin_manager;

pub use commands::AgentCommand;
pub use host::replay::{FileReplayStore, ReplayStore, SqliteReplayStore};
pub use host::stores::{
    ArtifactRef, ArtifactStore, BlobArtifactStore, FilePinStore, IdempotencyRecord, PinStore,
};
pub use host::uploads::{
    MemoryUploadIndex, SqliteUploadIndex, UploadIndex, UploadReceipt, UploadRecord,
};
pub use host::{HostConfig, HostError, NorteCodeRaepHost};
pub use http::router;

/// Extensions owned by the `NorteCode` server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NorteCodeExtension {
    /// Carries a complete `NorteCode` [`norte_code_wire::ClientRequest`].
    Command,
    /// Carries service telemetry.
    Telemetry,
    /// Carries service tool and rule state.
    ToolList,
    /// Carries plugin lifecycle data.
    Plugin,
}

impl NorteCodeExtension {
    /// Returns the namespaced wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Command => "norte_code.command",
            Self::Telemetry => "norte_code.telemetry",
            Self::ToolList => "norte_code.tool_list",
            Self::Plugin => "norte_code.plugin",
        }
    }
}
