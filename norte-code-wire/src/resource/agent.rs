//! Agent discovery resources.

use super::AgentRef;
use serde::{Deserialize, Serialize};

mod profile;
pub use profile::*;

/// One agent as `/agents` lists it.
///
/// The capabilities are the agent's own: which modalities it accepts and
/// produces, and what each one is bounded by, so a client can offer only what
/// the server will take instead of discovering it by failing an upload.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentSummary {
    /// The agent's id.
    #[serde(default)]
    pub id: String,
    /// The agent's version.
    #[serde(default)]
    pub version: String,
    /// What it can do, when the host reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<AgentCapabilities>,
    /// The provider that serves its default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentRef>,
    /// Its default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<AgentRef>,
    /// Safe, non-secret details from the agent's configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<AgentProfileSummary>,
}

/// What one agent accepts and produces.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentCapabilities {
    /// The modalities it accepts as input.
    #[serde(default)]
    pub input: Vec<crate::OutputModality>,
    /// The modalities it may produce.
    #[serde(default)]
    pub output: Vec<crate::OutputModality>,
    /// The bound for each modality it accepts.
    #[serde(default)]
    pub limits: std::collections::BTreeMap<crate::OutputModality, AgentModalityLimits>,
}

/// What one modality is bounded by.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AgentModalityLimits {
    /// The media types it accepts for this modality.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mime_types: Vec<String>,
    /// The largest one part may be, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// How many parts one message may carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    /// The longest audio or video it accepts, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
}
