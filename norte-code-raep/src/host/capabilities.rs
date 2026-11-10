//! Discovery: what this host offers and what it can take.

use norte_code_wire::SessionKey;

use serde_json::Value;
use std::collections::BTreeMap;

/// A server capability advertised in a handshake or attach payload.
///
/// The set is closed: the host advertises exactly these, and a client
/// reads them as variants, not strings. Each serializes to the wire name
/// the RAEP registry uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) enum Capability {
    /// WebSocket transport.
    #[serde(rename = "transport.ws")]
    TransportWs,
    /// Server-sent events transport.
    #[serde(rename = "transport.sse")]
    TransportSse,
    /// Session resume from a sequence cursor.
    #[serde(rename = "resume")]
    Resume,
    /// History pages (RFC 16).
    #[serde(rename = "history")]
    History,
    /// Session-scoped, single-shot input uploads.
    #[serde(rename = "uploads")]
    Uploads,
    /// Fetchable non-text model outputs.
    #[serde(rename = "output.parts")]
    OutputParts,
    /// `USER_INPUT.response_format` with `json_schema` (RFC 12.6).
    #[serde(rename = "structured_output")]
    StructuredOutput,
    /// `NorteCode`'s text-only compaction and context pin subset.
    #[serde(rename = "X_NORTECODE_COMPACTION_BASIC")]
    NorteCodeCompactionBasic,
    /// `NorteCode`'s text-history fork subset.
    #[serde(rename = "X_NORTECODE_FORK_TEXT")]
    NorteCodeForkText,
    /// `NorteCode`'s bounded job query/control subset, not the full RFC profile.
    #[serde(rename = "X_NORTECODE_JOBS_BASIC")]
    NorteCodeJobsBasic,
    /// `NorteCode`'s basic child-session inspection subset.
    #[serde(rename = "X_NORTECODE_DELEGATION_BASIC")]
    NorteCodeDelegationBasic,
}

/// Every capability this host advertises, in wire order.
pub(crate) fn advertised_capabilities(uploads: bool) -> Vec<Capability> {
    let mut capabilities = vec![
        Capability::TransportWs,
        Capability::TransportSse,
        Capability::Resume,
        Capability::History,
        Capability::StructuredOutput,
        Capability::NorteCodeCompactionBasic,
        Capability::NorteCodeForkText,
        Capability::NorteCodeJobsBasic,
        Capability::NorteCodeDelegationBasic,
    ];
    if uploads {
        capabilities.push(Capability::Uploads);
        capabilities.push(Capability::OutputParts);
    }
    capabilities
}

pub(crate) fn negotiated_capabilities(
    uploads: bool,
    client: &raep_types::ClientCapabilities,
) -> Vec<Capability> {
    let output_parts = client
        .features
        .contains(&raep_types::ClientFeature::OutputParts)
        && client
            .render
            .iter()
            .any(|modality| !matches!(modality, &raep_types::Modality::Text));
    advertised_capabilities(uploads)
        .into_iter()
        .filter(|capability| *capability != Capability::OutputParts || output_parts)
        .collect()
}

// The session list is the shared shape: the host writes what the clients
// decode, with no second struct and no strings leaked to give it a lifetime.

/// One agent version entry.
#[derive(Debug, serde::Serialize)]
pub(crate) struct AgentVersion<'a> {
    pub(crate) version: &'a str,
    pub(crate) created_at: i64,
    pub(crate) deprecated: bool,
}

/// An agent with its latest version.
#[derive(Debug, serde::Serialize)]
pub(crate) struct AgentDetail<'a> {
    pub(crate) agent: norte_code_wire::AgentSummary,
    pub(crate) latest_version: AgentVersion<'a>,
}

/// An agent's version list.
#[derive(Debug, serde::Serialize)]
pub(crate) struct AgentVersions<'a> {
    pub(crate) versions: Vec<AgentVersion<'a>>,
}

/// The protocol block in hello/info payloads.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ProtocolRef {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
}

/// A provider/model reference.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ModelRef<'a> {
    pub(crate) provider: &'a str,
    pub(crate) model: &'a str,
}

/// The limits block in hello/info payloads.
#[derive(Debug, serde::Serialize)]
pub(crate) struct LimitsRef {
    pub(crate) max_frame_bytes: u64,
    pub(crate) replay_buffer_events: usize,
    pub(crate) max_concurrent_jobs: usize,
    pub(crate) job_output_retention_s: u64,
    pub(crate) max_artifact_bytes: usize,
    pub(crate) max_upload_bytes: usize,
    pub(crate) max_inline_bytes: usize,
    pub(crate) max_output_part_bytes: usize,
    pub(crate) max_prompt_bytes: usize,
    pub(crate) max_parts: usize,
    pub(crate) max_queued_inputs: usize,
    pub(crate) max_export_bytes: usize,
    pub(crate) export_ttl_s: u64,
}

pub(crate) const DEFAULT_MAX_OUTPUT_PART_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const MAX_MEDIA_DURATION_MS: u64 = 600_000;

/// The hello handshake payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct HelloPayload<'a> {
    pub(crate) protocol: ProtocolRef,
    pub(crate) server_version: &'a str,
    pub(crate) wire_versions: Vec<&'static str>,
    pub(crate) capabilities: Vec<Capability>,
    pub(crate) extensions: Vec<&'a str>,
    pub(crate) limits: LimitsRef,
    pub(crate) heartbeat_s: u64,
    pub(crate) draining: bool,
    pub(crate) deprecations: Vec<Value>,
}

/// The HTTP discovery payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct InfoPayload<'a> {
    pub(crate) protocol: ProtocolRef,
    pub(crate) server_version: &'a str,
    pub(crate) wire_versions: Vec<&'static str>,
    pub(crate) capabilities: Vec<Capability>,
    pub(crate) extensions: Vec<&'a str>,
    pub(crate) limits: LimitsRef,
    pub(crate) agents: Vec<String>,
    pub(crate) default_agent: &'a str,
    pub(crate) default_model: Value,
}

/// The readiness payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ReadinessPayload<'a> {
    pub(crate) status: ReadinessStatus,
    pub(crate) agent_id: &'a str,
    pub(crate) agents: usize,
    pub(crate) replay: bool,
    pub(crate) artifacts: bool,
    pub(crate) limits: ReadinessLimits,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ReadinessStatus {
    Ready,
}

/// The limits block in the readiness payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ReadinessLimits {
    pub(crate) max_connections: usize,
    pub(crate) command_timeout_ms: u64,
}
/// The agent block inside a `SESSION_ATTACHED` payload.
///
/// The effective agent for the session: the host's configured agent, with
/// the provider/model reference of the session's spec when one exists.
pub(crate) fn session_agent_summary(
    service: &norte_code_agent::Service,
    id: &str,
    version: &str,
    session: &SessionKey,
    media_storage: bool,
) -> norte_code_wire::AgentSummary {
    let spec = service.session_spec(session);
    norte_code_wire::AgentSummary {
        id: id.to_owned(),
        version: version.to_owned(),
        capabilities: agent_capabilities(spec.as_ref(), media_storage),
        provider: spec.as_ref().map(|spec| norte_code_wire::AgentRef {
            id: spec.provider.clone(),
            version: String::new(),
        }),
        model: spec.as_ref().map(|spec| norte_code_wire::AgentRef {
            id: spec.model.clone(),
            version: String::new(),
        }),
    }
}

/// The wire's name for a modality, in the catalogue's spelling.
pub(crate) fn to_wire_modality(
    modality: norte::Modality,
) -> Option<norte_code_wire::OutputModality> {
    match modality {
        norte::Modality::Text => Some(norte_code_wire::OutputModality::Text),
        norte::Modality::Image => Some(norte_code_wire::OutputModality::Image),
        norte::Modality::Document => Some(norte_code_wire::OutputModality::Document),
        norte::Modality::Audio => Some(norte_code_wire::OutputModality::Audio),
        norte::Modality::Video => Some(norte_code_wire::OutputModality::Video),
        _ => None,
    }
}

pub(crate) fn agent_capabilities(
    spec: Option<&norte_code_agent::ModelSpec>,
    media_storage: bool,
) -> Option<norte_code_wire::AgentCapabilities> {
    let spec = spec?;
    let input: Vec<_> = [
        norte::Modality::Text,
        norte::Modality::Image,
        norte::Modality::Document,
        norte::Modality::Audio,
        norte::Modality::Video,
    ]
    .into_iter()
    .filter(|modality| {
        spec.capabilities.modalities.supports_input(*modality)
            && (media_storage || *modality == norte::Modality::Text)
    })
    .filter_map(to_wire_modality)
    .collect::<Vec<_>>();
    let output: Vec<_> = [
        norte::Modality::Text,
        norte::Modality::Image,
        norte::Modality::Document,
        norte::Modality::Audio,
        norte::Modality::Video,
    ]
    .into_iter()
    .filter(|modality| {
        spec.capabilities.modalities.supports_output(*modality)
            && (media_storage || *modality == norte::Modality::Text)
    })
    .filter_map(to_wire_modality)
    .collect::<Vec<_>>();
    let mut limits = BTreeMap::new();
    for modality in [
        norte::Modality::Text,
        norte::Modality::Image,
        norte::Modality::Document,
        norte::Modality::Audio,
        norte::Modality::Video,
    ]
    .into_iter()
    .filter(|modality| {
        spec.capabilities.modalities.supports_input(*modality)
            && (media_storage || *modality == norte::Modality::Text)
    }) {
        let configured = spec.capabilities.modalities.limits(modality);
        let mut modality_limits = norte_code_wire::AgentModalityLimits::default();
        if let Some(configured) = configured {
            if !configured.mime_types.is_empty() {
                modality_limits.mime_types = configured
                    .mime_types
                    .iter()
                    .map(ToString::to_string)
                    .collect();
            }
            modality_limits.max_bytes = configured.max_bytes;
            modality_limits.max_items = configured.max_count;
        }
        if matches!(modality, norte::Modality::Audio | norte::Modality::Video) {
            modality_limits.max_duration_ms = Some(MAX_MEDIA_DURATION_MS);
        }
        if modality_limits.mime_types.is_empty()
            && modality_limits.max_bytes.is_none()
            && modality_limits.max_items.is_none()
            && modality_limits.max_duration_ms.is_none()
        {
            continue;
        }
        let Some(modality_key) = to_wire_modality(modality) else {
            continue;
        };
        limits.insert(modality_key, modality_limits);
    }

    Some(norte_code_wire::AgentCapabilities {
        input,
        output,
        limits,
    })
}
