//! RAEP request payloads accepted by the host.

use serde_json::Value;
use std::collections::HashMap;

use super::HostError;

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryGetRequest {
    #[serde(default)]
    pub(super) before_turn_id: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<u64>,
    #[serde(default)]
    pub(super) include_rewound: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkRequest {
    /// The fork source, repeated from the envelope by clients that fill the
    /// payload the way the spec's example does. The host reads the envelope's
    /// `session_id`; the field is accepted so a client naming the session
    /// twice is not rejected for it.
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) session_id: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) from_turn_id: Option<String>,
    #[serde(default)]
    pub(super) inclusive: Option<bool>,
    #[serde(default)]
    pub(super) carry_usage: Option<bool>,
    #[serde(default)]
    pub(super) agent_id: Option<String>,
    #[serde(default)]
    pub(super) include: Option<norte_code_wire::ForkInclude>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RewindRequest {
    pub(super) to_turn_id: String,
    #[serde(default)]
    pub(super) inclusive: bool,
    #[serde(default)]
    pub(super) cancel_pending: bool,
    #[serde(default)]
    pub(super) workspace: Option<WorkspaceRewindPolicy>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WorkspaceRewindPolicy {
    Keep,
    Restore,
}

pub(crate) fn default_true() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct ExportInclude {
    #[serde(default = "default_true")]
    pub(super) children: bool,
    #[serde(default = "default_true")]
    pub(super) uploads: bool,
    #[serde(default = "default_true")]
    pub(super) job_output: bool,
    #[serde(default = "default_true")]
    pub(super) outputs: bool,
    #[serde(default)]
    pub(super) workspace: bool,
}

impl Default for ExportInclude {
    fn default() -> Self {
        Self {
            children: true,
            uploads: true,
            job_output: true,
            outputs: true,
            workspace: false,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExportRequest {
    #[serde(default)]
    pub(super) include: ExportInclude,
    #[serde(default)]
    pub(super) quiesce: Option<QuiescePolicy>,
    #[serde(default)]
    pub(super) wait_timeout_s: Option<u64>,
    #[serde(default)]
    pub(super) encrypt: Option<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QuiescePolicy {
    Reject,
    Wait,
    KillJobs,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportCommitRequest {
    #[serde(default)]
    pub(super) agent_map: HashMap<String, String>,
    #[serde(default)]
    pub(super) carry_usage: Option<bool>,
    #[serde(default)]
    pub(super) decryption_jwk: Option<Value>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct PortableHistoryItem {
    pub(super) role: PortableHistoryRole,
    pub(super) content: Value,
    #[serde(default)]
    pub(super) turn_id: Option<String>,
    #[serde(default)]
    pub(super) timestamp: Option<i64>,
    #[serde(default)]
    pub(super) origin: Option<Value>,
    #[serde(default)]
    pub(super) provenance: Option<Value>,
    #[serde(default)]
    pub(super) rewound: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PortableHistoryRole {
    User,
    Assistant,
}

/// The `SESSION_CONFIGURED` payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SessionConfiguredPayload {
    pub(super) applied: bool,
}

/// A `SESSION_CONFIGURE` payload as this host accepts it (RFC 8.1).
///
/// The payload is the standard one — `agent_id`, `agent_override`,
/// `options`, `apply` — not a serialised [`AgentCommand`]. The host
/// translates its known fields onto service requests; anything it cannot
/// express fails with `AGENT_OVERRIDE_DENIED` instead of being ignored.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub(crate) struct SessionConfigurePayload {
    /// Switch to another agent. Only honoured when it names the host's
    /// configured agent; anything else is `agent_mismatch`.
    #[serde(default)]
    pub(super) agent_id: Option<String>,
    /// Per-session configuration fragment. See [`decode_override`].
    #[serde(default)]
    pub(super) agent_override: Option<Value>,
    /// Session options supported by this host.
    #[serde(default)]
    pub(super) options: Option<Value>,
    /// When the change applies. This host supports immediate application only.
    #[serde(default)]
    pub(super) apply: Option<ConfigureApplyPolicy>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigureApplyPolicy {
    Now,
    NextTurn,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionOptions {
    #[serde(default)]
    job_turn_policy: Option<norte_code_wire::JobTurnPolicy>,
}

pub(crate) fn parse_job_turn_policy(
    options: Option<&Value>,
) -> Result<Option<norte_code_wire::JobTurnPolicy>, HostError> {
    let Some(options) = options else {
        return Ok(None);
    };
    let options: SessionOptions = serde_json::from_value(options.clone()).map_err(|error| {
        HostError::InvalidFrame(format!("SESSION_OPTIONS_UNSUPPORTED: {error}"))
    })?;
    Ok(options.job_turn_policy)
}
