//! Runtime-independent RAEP host adapter.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use norte_code_agent::Service;
use norte_code_protocol::{
    ClientRequest, ClientRequestKind, Delta, ServerEvent, ServerEventKind, SessionKey,
};
use raep_types::{
    AttachMode, Envelope, FrameId, FrameType, SessionAttach, StandardFrameType, TurnId,
    WIRE_VERSION,
};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore, broadcast};

use crate::NorteCodeExtension;
use crate::payloads::{canonical_event_payload, payload, standard_event_payload};

/// A server capability advertised in a handshake or attach payload.
///
/// The set is closed: the host advertises exactly these, and a client
/// reads them as variants, not strings. Each serializes to the wire name
/// the RAEP registry uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
enum Capability {
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
fn advertised_capabilities(uploads: bool) -> Vec<Capability> {
    let mut capabilities = vec![
        Capability::TransportWs,
        Capability::TransportSse,
        Capability::Resume,
        Capability::History,
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

fn negotiated_capabilities(
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

/// The agent block inside a session or agent summary.
#[derive(Debug, serde::Serialize)]
struct AgentRef<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    version: &'a str,
}

/// A session summary as the management API returns it.
#[derive(Debug, serde::Serialize)]
struct SessionSummary<'a> {
    session_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    title_source: SessionTitleSource,
    labels: BTreeMap<String, Value>,
    pinned: bool,
    archived: bool,
    status: SessionSummaryStatus,
    agent: AgentRef<'a>,
    created_at: i64,
    updated_at: i64,
    last_activity_at: i64,
    turn_count: u32,
    has_pending: bool,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum SessionTitleSource {
    User,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum SessionSummaryStatus {
    Active,
    Suspended,
}

/// An agent summary as the management API returns it.
#[derive(Debug, serde::Serialize)]
struct AgentSummary<'a> {
    id: &'a str,
    version: &'a str,
    #[serde(skip_serializing_if = "Value::is_null")]
    capabilities: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<AgentRef<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<AgentRef<'a>>,
}

#[derive(Debug, serde::Serialize)]
struct AgentCapabilities {
    input: Vec<raep_types::Modality>,
    output: Vec<raep_types::Modality>,
    limits: BTreeMap<AgentModality, AgentModalityLimits>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum AgentModality {
    Text,
    Image,
    Document,
    Audio,
    Video,
}

#[derive(Debug, Default, serde::Serialize)]
struct AgentModalityLimits {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mime_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_items: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_duration_ms: Option<u64>,
}

/// One agent version entry.
#[derive(Debug, serde::Serialize)]
struct AgentVersion<'a> {
    version: &'a str,
    created_at: i64,
    deprecated: bool,
}

/// An agent with its latest version.
#[derive(Debug, serde::Serialize)]
struct AgentDetail<'a> {
    agent: AgentSummary<'a>,
    latest_version: AgentVersion<'a>,
}

/// An agent's version list.
#[derive(Debug, serde::Serialize)]
struct AgentVersions<'a> {
    versions: Vec<AgentVersion<'a>>,
}

/// The protocol block in hello/info payloads.
#[derive(Debug, serde::Serialize)]
struct ProtocolRef {
    name: &'static str,
    version: &'static str,
}

/// A provider/model reference.
#[derive(Debug, serde::Serialize)]
struct ModelRef<'a> {
    provider: &'a str,
    model: &'a str,
}

/// The limits block in hello/info payloads.
#[derive(Debug, serde::Serialize)]
struct LimitsRef {
    max_frame_bytes: u64,
    replay_buffer_events: usize,
    max_concurrent_jobs: usize,
    job_output_retention_s: u64,
    max_artifact_bytes: usize,
    max_upload_bytes: usize,
    max_inline_bytes: usize,
    max_output_part_bytes: usize,
    max_prompt_bytes: usize,
    max_parts: usize,
    max_queued_inputs: usize,
    max_export_bytes: usize,
    export_ttl_s: u64,
}

const DEFAULT_MAX_OUTPUT_PART_BYTES: usize = 10 * 1024 * 1024;
const MAX_MEDIA_DURATION_MS: u64 = 600_000;

/// The hello handshake payload.
#[derive(Debug, serde::Serialize)]
struct HelloPayload<'a> {
    protocol: ProtocolRef,
    server_version: &'a str,
    wire_versions: Vec<&'static str>,
    capabilities: Vec<Capability>,
    extensions: Vec<&'a str>,
    limits: LimitsRef,
    heartbeat_s: u64,
    draining: bool,
    deprecations: Vec<Value>,
}

/// The HTTP discovery payload.
#[derive(Debug, serde::Serialize)]
struct InfoPayload<'a> {
    protocol: ProtocolRef,
    server_version: &'a str,
    wire_versions: Vec<&'static str>,
    capabilities: Vec<Capability>,
    extensions: Vec<&'a str>,
    limits: LimitsRef,
    agents: Vec<String>,
    default_agent: &'a str,
    default_model: Value,
}

/// The readiness payload.
#[derive(Debug, serde::Serialize)]
struct ReadinessPayload<'a> {
    status: ReadinessStatus,
    agent_id: &'a str,
    agents: usize,
    replay: bool,
    artifacts: bool,
    limits: ReadinessLimits,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum ReadinessStatus {
    Ready,
}

/// The limits block in the readiness payload.
#[derive(Debug, serde::Serialize)]
struct ReadinessLimits {
    max_connections: usize,
    command_timeout_ms: u64,
}

/// The kind of a canonical `service_data` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ServiceDataKind {
    Telemetry,
    ToolList,
    McpList,
}

/// A canonical `service_data` payload, decoded to find the telemetry rows.
#[derive(Debug, serde::Deserialize)]
struct CanonicalServiceData {
    kind: ServiceDataKind,
    data: Value,
}

/// The telemetry report payload.
#[derive(Debug, serde::Serialize)]
struct TelemetryReport {
    session: Option<String>,
    records: Vec<norte_code_protocol::sqlite::RequestRecord>,
    summary: TelemetrySummary,
}

/// The counters inside a telemetry report.
#[derive(Debug, serde::Serialize)]
struct TelemetrySummary {
    requests: usize,
    errors: u64,
    cancelled: u64,
    duration_ms: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
}

/// A replay descriptor in an attach payload.
#[derive(Debug, serde::Serialize)]
struct ReplayMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    from_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_seq: Option<u64>,
    truncated: bool,
}

/// The session-attached payload.
#[derive(Debug, serde::Serialize)]
struct SessionAttachedPayload<'a> {
    created: bool,
    mode: AttachMode,
    last_seq: u64,
    replay: ReplayMeta,
    negotiated_capabilities: Vec<Capability>,
    /// The non-secret summary of the effective agent (RFC 7). This host
    /// only runs one configured agent, so it is that agent.
    agent: AgentSummary<'a>,
}

/// The agent block inside a `SESSION_ATTACHED` payload.
///
/// The effective agent for the session: the host's configured agent, with
/// the provider/model reference of the session's spec when one exists.
fn session_agent_summary<'a>(
    service: &norte_code_agent::Service,
    id: &'a str,
    version: &'a str,
    session: &SessionKey,
    media_storage: bool,
) -> AgentSummary<'a> {
    let spec = service.session_spec(session);
    AgentSummary {
        id,
        version,
        capabilities: agent_capabilities(spec.as_ref(), media_storage),
        provider: spec.as_ref().map(|spec| AgentRef {
            id: Box::leak(spec.provider.clone().into_boxed_str()) as &'static str,
            version: "",
        }),
        model: spec.as_ref().map(|spec| AgentRef {
            id: Box::leak(spec.model.clone().into_boxed_str()) as &'static str,
            version: "",
        }),
    }
}

fn to_raep_modality(modality: norte::Modality) -> Option<raep_types::Modality> {
    match modality {
        norte::Modality::Text => Some(raep_types::Modality::Text),
        norte::Modality::Image => Some(raep_types::Modality::Image),
        norte::Modality::Document => Some(raep_types::Modality::Document),
        norte::Modality::Audio => Some(raep_types::Modality::Audio),
        norte::Modality::Video => Some(raep_types::Modality::Video),
        _ => None,
    }
}

fn to_agent_modality(modality: norte::Modality) -> Option<AgentModality> {
    match modality {
        norte::Modality::Text => Some(AgentModality::Text),
        norte::Modality::Image => Some(AgentModality::Image),
        norte::Modality::Document => Some(AgentModality::Document),
        norte::Modality::Audio => Some(AgentModality::Audio),
        norte::Modality::Video => Some(AgentModality::Video),
        _ => None,
    }
}

fn agent_capabilities(spec: Option<&norte_code_agent::ModelSpec>, media_storage: bool) -> Value {
    let Some(spec) = spec else {
        return Value::Null;
    };
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
    .filter_map(to_raep_modality)
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
    .filter_map(to_raep_modality)
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
        let mut modality_limits = AgentModalityLimits::default();
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
        let Some(modality_key) = to_agent_modality(modality) else {
            continue;
        };
        limits.insert(modality_key, modality_limits);
    }

    payload(AgentCapabilities {
        input,
        output,
        limits,
    })
}

/// The session-state payload.
#[derive(Debug, serde::Serialize)]
struct SessionStatePayload<'a> {
    status: SessionStateStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    history_summary: Vec<HistorySummaryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    imported_from: Option<&'a Value>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum SessionStateStatus {
    Active,
}

#[derive(Debug, serde::Serialize)]
struct HistorySummaryItem {
    role: HistoryRole,
    kind: HistoryItemKind,
    content: String,
    turn_id: Option<String>,
    timestamp: i64,
    rewound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<Value>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum HistoryRole {
    Tool,
    SystemEvent,
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum HistoryItemKind {
    Tool,
    SystemEvent,
    User,
    Assistant,
    Reasoning,
    ToolCall,
    ToolResult,
    Refusal,
}

struct HistoryItemView {
    turn: Option<u64>,
    role: norte_code_protocol::MessageRole,
    kind: norte_code_protocol::ReplayedKind,
    text: String,
    timestamp: i64,
    rewound: bool,
    origin: Option<Value>,
    max_content_bytes: Option<usize>,
}

fn history_item_value(item: HistoryItemView) -> HistorySummaryItem {
    let role = match item.role {
        norte_code_protocol::MessageRole::Tool => HistoryRole::Tool,
        norte_code_protocol::MessageRole::System
        | norte_code_protocol::MessageRole::Developer
        | norte_code_protocol::MessageRole::Unknown => HistoryRole::SystemEvent,
        norte_code_protocol::MessageRole::User => HistoryRole::User,
        norte_code_protocol::MessageRole::Assistant => HistoryRole::Assistant,
    };
    let kind = match item.kind {
        norte_code_protocol::ReplayedKind::Text => match role {
            HistoryRole::Tool => HistoryItemKind::Tool,
            HistoryRole::SystemEvent => HistoryItemKind::SystemEvent,
            HistoryRole::User => HistoryItemKind::User,
            HistoryRole::Assistant => HistoryItemKind::Assistant,
        },
        norte_code_protocol::ReplayedKind::Reasoning => HistoryItemKind::Reasoning,
        norte_code_protocol::ReplayedKind::ToolCall => HistoryItemKind::ToolCall,
        norte_code_protocol::ReplayedKind::ToolResult => HistoryItemKind::ToolResult,
        norte_code_protocol::ReplayedKind::Refusal => HistoryItemKind::Refusal,
    };
    let (content, truncated) = if let Some(max) = item.max_content_bytes {
        let mut end = item.text.len().min(max);
        while !item.text.is_char_boundary(end) {
            end -= 1;
        }
        (item.text[..end].to_owned(), end < item.text.len())
    } else {
        (item.text, false)
    };
    HistorySummaryItem {
        role,
        kind,
        content,
        turn_id: item.turn.map(|turn| format!("turn_{turn}")),
        timestamp: item.timestamp,
        rewound: item.rewound,
        truncated: truncated.then_some(true),
        origin: item.origin,
    }
}

#[derive(Debug, serde::Serialize)]
struct HistoryPagePayload {
    items: Vec<HistorySummaryItem>,
    has_more: bool,
}

#[derive(Debug, serde::Serialize)]
struct HistoryRewoundPayload {
    to_turn_id: String,
    inclusive: bool,
    rewound_turn_ids: Vec<String>,
    side_effects: Vec<HistorySideEffect>,
    workspace_restored: bool,
}

#[derive(Debug, serde::Serialize)]
struct HistorySideEffect {
    tool_call_id: String,
    tool_name: String,
    side_effects: raep_types::ToolSideEffects,
}

#[derive(Debug, serde::Serialize)]
struct JobStatePayload<'a> {
    job_id: &'a str,
    status: JobStatus,
    output: &'a str,
    next_index: u64,
}

#[derive(Debug, serde::Serialize)]
struct JobFinishedPayload<'a> {
    status: JobStatus,
    exit_code: Option<i32>,
    signal: Option<&'a str>,
    summary: &'a str,
    output_snapshot: &'a str,
    output_truncated: bool,
    output_bytes_total: u64,
    duration_ms: u64,
    triggered_new_turn: bool,
    new_turn_id: Option<&'a str>,
    injected_into_turn_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum JobStatus {
    Running,
    Completed,
    Failed,
    Killed,
    TimedOut,
}

#[derive(Debug, serde::Deserialize)]
struct JobListRequest {
    status: Option<Vec<JobStatus>>,
    limit: Option<u64>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct JobListEntry {
    status: JobStatus,
    #[serde(flatten)]
    fields: BTreeMap<String, Value>,
}

#[derive(Debug, serde::Serialize)]
struct JobListResultPayload {
    jobs: Vec<JobListEntry>,
}

#[derive(Debug, serde::Serialize)]
struct ContextUpdatedPayload {
    pinned_turn_ids: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct ExportReadyPayload<'a> {
    export_id: &'a str,
    download_url: String,
    bytes: usize,
    sha256: &'a str,
    expires_at: i64,
    sessions: u32,
    encrypted: bool,
    signed: bool,
}

#[derive(Debug, serde::Serialize)]
struct ImportAgentAvailability<'a> {
    agent_id: &'a str,
    version: &'a str,
    available: bool,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum ImportSignatureStatus {
    Absent,
}

#[derive(Debug, serde::Serialize)]
struct ImportPreviewPayload<'a> {
    import_id: &'a str,
    bundle_version: &'a str,
    sessions: usize,
    signature: ImportSignatureStatus,
    agents: Vec<ImportAgentAvailability<'a>>,
    warnings: [&'static str; 2],
    expires_at: i64,
}

#[derive(Debug, serde::Serialize)]
struct ImportedFromPayload<'a> {
    exporter: &'a Option<Value>,
    session_id: &'a str,
    exported_at: u64,
}

#[derive(Debug, serde::Serialize)]
struct ImportCommittedPayload<'a> {
    session_id: &'a str,
    id_map: BTreeMap<String, String>,
    warnings: [&'static str; 2],
}

#[derive(Debug, serde::Serialize)]
struct QuotaPeriod {
    start: u64,
    end: u64,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum QuotaDimensionName {
    Sessions,
    ReplayBufferEvents,
}

#[derive(Debug, serde::Serialize)]
struct QuotaDimension {
    name: QuotaDimensionName,
    limit: u64,
    used: u64,
    remaining: u64,
}

#[derive(Debug, serde::Serialize)]
struct QuotaPayload<'a> {
    tenant: &'a str,
    period: QuotaPeriod,
    dimensions: [QuotaDimension; 2],
}

#[derive(Debug, serde::Serialize)]
struct HistoryExportItem<'a> {
    role: ExportHistoryRole,
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<Value>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum ExportHistoryRole {
    User,
    Assistant,
}

#[derive(Debug, serde::Serialize)]
struct BundleAgent<'a> {
    id: &'a str,
}

#[derive(Debug, serde::Serialize)]
struct PortableSessionFile<'a> {
    agent: BundleAgent<'a>,
    title: &'a Option<String>,
    options: BTreeMap<String, Value>,
}

#[derive(Debug, serde::Serialize)]
struct BundleExporter<'a> {
    server: &'a str,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum SessionClosedReason {
    Client,
    Purged,
}

#[derive(Debug, serde::Serialize)]
struct SessionClosedPayload {
    reason: SessionClosedReason,
}

#[derive(Debug, serde::Serialize)]
struct DelegatorStateChangedPayload<'a> {
    child_session_id: &'a str,
    suspended: bool,
    by: DelegatorActor,
    queued_messages: usize,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum DelegatorActor {
    Client,
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum OutputPartKind {
    Image,
    Document,
    Audio,
    Video,
}

#[derive(Debug, serde::Serialize)]
struct OutputPartStartedPayload<'a> {
    part_id: &'a str,
    index: u32,
    #[serde(rename = "type")]
    kind: OutputPartKind,
    mime_type: &'a str,
    streamed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    bytes_expected: usize,
}

#[derive(Debug, serde::Serialize)]
enum OutputPartStatus {
    #[serde(rename = "OK")]
    Ok,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum OutputPartSource<'a> {
    Output { output_id: &'a str },
}

#[derive(Debug, serde::Serialize)]
struct OutputPartFinishedPayload<'a> {
    part_id: &'a str,
    index: u32,
    status: OutputPartStatus,
    source: OutputPartSource<'a>,
    bytes: usize,
    sha256: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript: Option<&'a str>,
}

#[derive(Debug, serde::Serialize)]
struct NorteCodeEventExtension<'a> {
    norte_code: &'a ServerEvent,
}

#[derive(Debug, serde::Serialize)]
struct UserInputAcceptedPayload {
    turn_id: TurnId,
    origin: UserInputOrigin,
    content: Vec<AcceptedContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replaces_turn_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct UserInputOrigin {
    kind: UserInputOriginKind,
}

#[derive(Debug, serde::Serialize)]
struct QueuedTurnStatePayload {
    state: raep_types::TurnState,
}

#[derive(Debug, serde::Serialize)]
enum UserInputOriginKind {
    #[serde(rename = "user")]
    User,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
struct AcceptedContentPart {
    #[serde(rename = "type")]
    kind: AcceptedPartKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<AcceptedSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<ImageDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pages: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum AcceptedPartKind {
    #[default]
    Text,
    Image,
    Document,
    Audio,
    Video,
}

impl AcceptedPartKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Document => "document",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum ImageDetail {
    Auto,
    Low,
    High,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum AcceptedSource {
    #[serde(rename = "upload")]
    Upload { upload_id: String },
    #[serde(rename = "output")]
    Output { output_id: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AssetSourceKind {
    Upload,
    Output,
}

/// A `USER_INPUT` payload as this host accepts it.
///
/// The spec (RFC 12.4) allows two more regions that this host does not
/// implement yet: `replaces_turn_id` (rewind-and-re-run in one step) and any
/// `response_format` other than plain `text`. Both are still decoded here,
/// so a client that sends them gets a clear rejection from
/// [`validate_user_input`] instead of silently ignoring them.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
struct AcceptedUserInput {
    /// Plain text input. Exactly one of `prompt` and `content` is required.
    #[serde(default)]
    prompt: Option<String>,
    /// Multimodal input. Exactly one of `prompt` and `content` is required.
    #[serde(default)]
    content: Option<Vec<AcceptedContentPart>>,
    /// Behavior when another turn is active.
    #[serde(default)]
    queue_policy: Option<raep_types::QueuePolicy>,
    /// Per-turn configuration overrides. See [`decode_override`]: the known
    /// fields are applied to this turn; unknown fields fail instead of being
    /// silently dropped.
    #[serde(default)]
    overrides: Option<Value>,
    /// Output modalities the client wants back. This host only produces text:
    /// anything other than `["text"]` is rejected.
    #[serde(default)]
    output: Option<Vec<raep_types::Modality>>,
    /// The shape the final answer must take. Hardcoded to `text` for this
    /// host: `json_schema` is rejected until structured output exists.
    #[serde(default)]
    response_format: Option<AcceptedResponseFormat>,
    /// Rewind-and-re-run in one step (RFC 18.2). Not supported yet.
    #[serde(default)]
    replaces_turn_id: Option<String>,
    /// Arbitrary caller metadata: an opaque object of at most 4 KiB, stored
    /// with the turn and never shown to the model (RFC 12.4). Validated here
    /// for size; the service's turn record has no slot for it yet, so it is
    /// not persisted.
    #[serde(default)]
    #[allow(dead_code)]
    metadata: Option<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResponseFormatKind {
    Text,
    JsonSchema,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
struct AcceptedResponseFormat {
    #[serde(rename = "type")]
    kind: ResponseFormatKind,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryGetRequest {
    #[serde(default)]
    before_turn_id: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    include_rewound: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    from_turn_id: Option<String>,
    #[serde(default)]
    inclusive: Option<bool>,
    #[serde(default)]
    carry_usage: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RewindRequest {
    to_turn_id: String,
    #[serde(default)]
    inclusive: bool,
    #[serde(default)]
    cancel_pending: bool,
    #[serde(default)]
    workspace: Option<WorkspaceRewindPolicy>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum WorkspaceRewindPolicy {
    Keep,
    Restore,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
struct ExportInclude {
    #[serde(default = "default_true")]
    children: bool,
    #[serde(default = "default_true")]
    uploads: bool,
    #[serde(default = "default_true")]
    job_output: bool,
    #[serde(default = "default_true")]
    outputs: bool,
    #[serde(default)]
    workspace: bool,
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
struct ExportRequest {
    #[serde(default)]
    include: ExportInclude,
    #[serde(default)]
    quiesce: Option<QuiescePolicy>,
    #[serde(default)]
    wait_timeout_s: Option<u64>,
    #[serde(default)]
    encrypt: Option<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum QuiescePolicy {
    Reject,
    Wait,
    KillJobs,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportCommitRequest {
    #[serde(default)]
    agent_map: HashMap<String, String>,
    #[serde(default)]
    carry_usage: Option<bool>,
    #[serde(default)]
    decryption_jwk: Option<Value>,
}

#[derive(Debug, serde::Deserialize)]
struct PortableHistoryItem {
    role: PortableHistoryRole,
    content: Value,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    origin: Option<Value>,
    #[serde(default)]
    provenance: Option<Value>,
    #[serde(default)]
    rewound: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum PortableHistoryRole {
    User,
    Assistant,
}

impl PortableHistoryRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// The `SESSION_CONFIGURED` payload.
#[derive(Debug, serde::Serialize)]
struct SessionConfiguredPayload {
    applied: bool,
}

/// Rejects the `UserInput` field values this host does not support.
///
/// `replaces_turn_id` (rewind-and-re-run) is not implemented. Non-text output
/// is accepted only when both the model and the attached client support it;
/// `metadata` over 4 KiB violates the spec's bound for opaque caller metadata.
fn validate_user_input(input: &AcceptedUserInput) -> Result<(), HostError> {
    match (input.prompt.as_deref(), input.content.as_deref()) {
        (Some(prompt), None) if !prompt.trim().is_empty() => {
            if prompt.len() > 256 * 1024 {
                return Err(HostError::InvalidFrame(
                    "prompt exceeds max_prompt_bytes".to_owned(),
                ));
            }
        }
        (None, Some(parts)) if !parts.is_empty() => {
            validate_content_parts(parts)?;
        }
        (Some(_), Some(_)) => {
            return Err(HostError::InvalidFrame(
                "exactly one of prompt and content is required".to_owned(),
            ));
        }
        _ => {
            return Err(HostError::InvalidFrame(
                "prompt or non-empty content is required".to_owned(),
            ));
        }
    }
    if let Some(id) = &input.replaces_turn_id {
        return Err(HostError::InvalidFrame(format!(
            "replaces_turn_id `{id}` is not supported by this host"
        )));
    }
    if let Some(format) = &input.response_format
        && format.kind != ResponseFormatKind::Text
    {
        return Err(HostError::InvalidFrame(
            "response_format other than text is not supported by this host".to_owned(),
        ));
    }
    if let Some(output) = &input.output
        && (output.is_empty()
            || output
                .iter()
                .enumerate()
                .any(|(index, modality)| output[..index].contains(modality)))
    {
        return Err(HostError::InvalidFrame(
            "output must contain unique modalities".to_owned(),
        ));
    }
    if let Some(metadata) = &input.metadata {
        let bytes = serde_json::to_vec(metadata)?.len();
        if bytes > 4 * 1024 {
            return Err(HostError::InvalidFrame("metadata exceeds 4 KiB".to_owned()));
        }
    }
    Ok(())
}

fn validate_content_parts(parts: &[AcceptedContentPart]) -> Result<(), HostError> {
    if parts.len() > 16 {
        return Err(HostError::InvalidFrame(
            "content exceeds max_parts".to_owned(),
        ));
    }
    let mut text_bytes = 0_usize;
    for (index, part) in parts.iter().enumerate() {
        text_bytes = text_bytes.saturating_add(validate_content_part(part, index)?);
    }
    if text_bytes > 256 * 1024 {
        return Err(HostError::InvalidFrame(
            "content text exceeds max_prompt_bytes".to_owned(),
        ));
    }
    Ok(())
}

fn validate_content_part(part: &AcceptedContentPart, index: usize) -> Result<usize, HostError> {
    match part.kind {
        AcceptedPartKind::Text => {
            if part.text.as_deref().is_none_or(str::is_empty)
                || part.source.is_some()
                || part.mime_type.is_some()
            {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: text parts require text only"
                )));
            }
            Ok(part.text.as_deref().map_or(0, str::len))
        }
        AcceptedPartKind::Image => {
            if part.text.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: image parts cannot contain text"
                )));
            }
            if part
                .detail
                .is_some_and(|detail| detail != ImageDetail::Auto)
            {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: image detail overrides are not supported"
                )));
            }
            validate_upload_source(part, index)?;
            Ok(0)
        }
        AcceptedPartKind::Document => {
            if part.text.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: document parts cannot contain text"
                )));
            }
            if part.pages.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: document page ranges are not supported"
                )));
            }
            validate_upload_source(part, index)?;
            Ok(0)
        }
        AcceptedPartKind::Audio | AcceptedPartKind::Video => {
            if part.text.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: media parts cannot contain text"
                )));
            }
            if part
                .duration_ms
                .is_some_and(|duration| duration > MAX_MEDIA_DURATION_MS)
            {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: duration exceeds max_media_duration_ms"
                )));
            }
            if part
                .transcript
                .as_ref()
                .is_some_and(|text| text.len() > 16 * 1024)
            {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: transcript exceeds 16 KiB"
                )));
            }
            if part.kind == AcceptedPartKind::Video && part.transcript.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: transcript is only valid for audio"
                )));
            }
            validate_upload_source(part, index)?;
            Ok(0)
        }
    }
}

fn validate_upload_source(part: &AcceptedContentPart, index: usize) -> Result<(), HostError> {
    let mime = part.mime_type.as_deref().ok_or_else(|| {
        HostError::InvalidFrame(format!("content part {index}: mime_type is required"))
    })?;
    if part.source.is_none() {
        return Err(HostError::InvalidFrame(format!(
            "content part {index}: source is required"
        )));
    }
    validate_media_mime(part.kind, mime)
        .map_err(|message| HostError::InvalidFrame(format!("content part {index}: {message}")))
}

fn validate_media_mime(kind: AcceptedPartKind, mime: &str) -> Result<(), String> {
    norte::MimeType::new(mime.to_owned())
        .map_err(|_| format!("`{mime}` is not a valid MIME type"))?;
    let supported = match kind {
        AcceptedPartKind::Image => mime.starts_with("image/"),
        AcceptedPartKind::Document => {
            matches!(mime, "application/pdf" | "text/plain" | "text/markdown")
        }
        AcceptedPartKind::Audio => mime.starts_with("audio/"),
        AcceptedPartKind::Video => mime.starts_with("video/"),
        AcceptedPartKind::Text => false,
    };
    if supported {
        Ok(())
    } else {
        Err(format!(
            "MIME type `{mime}` is not supported for {} input",
            kind.as_str()
        ))
    }
}

fn supported_input_modalities(service: &Service, session: &SessionKey) -> Vec<norte::Modality> {
    let Some(spec) = service.session_spec(session) else {
        return Vec::new();
    };
    [
        norte::Modality::Text,
        norte::Modality::Image,
        norte::Modality::Document,
        norte::Modality::Audio,
        norte::Modality::Video,
    ]
    .into_iter()
    .filter(|modality| spec.capabilities.modalities.supports_input(*modality))
    .collect()
}

fn to_norte_modality(modality: &raep_types::Modality) -> norte::Modality {
    match modality {
        raep_types::Modality::Text => norte::Modality::Text,
        raep_types::Modality::Image => norte::Modality::Image,
        raep_types::Modality::Document => norte::Modality::Document,
        raep_types::Modality::Audio => norte::Modality::Audio,
        raep_types::Modality::Video => norte::Modality::Video,
    }
}

fn raep_modality_for_media(kind: norte::MediaKind) -> Option<raep_types::Modality> {
    match kind {
        norte::MediaKind::Image => Some(raep_types::Modality::Image),
        norte::MediaKind::Document => Some(raep_types::Modality::Document),
        norte::MediaKind::Audio => Some(raep_types::Modality::Audio),
        norte::MediaKind::Video => Some(raep_types::Modality::Video),
        _ => None,
    }
}

/// Extracts the reasoning, model and budget overrides from one override
/// object.
///
/// Only the fields in the closed set below are accepted; any other key
/// fails with `AGENT_OVERRIDE_DENIED`, so a client that sends a field this
/// host cannot honour gets an error naming it rather than a silent no-op.
type OverrideParts = (
    Option<norte_code_protocol::Reasoning>,
    Option<(String, String)>,
    Option<norte_code_protocol::Budget>,
);

fn decode_override(value: &Value) -> Result<OverrideParts, HostError> {
    let Value::Object(fields) = value else {
        return Err(HostError::InvalidFrame(
            "overrides must be an object".to_owned(),
        ));
    };
    let mut reasoning = None;
    let mut model = None;
    let mut budget = None;
    for (key, field) in fields {
        match key.as_str() {
            "reasoning" => {
                reasoning = Some(decode_reasoning_override(field)?);
            }
            "model" => {
                let id = field.get("id").and_then(Value::as_str).ok_or_else(|| {
                    HostError::InvalidFrame("model override requires `id`".to_owned())
                })?;
                let provider = field
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                model = Some((provider, id.to_owned()));
            }
            "provider" => {
                let id = field.as_str().ok_or_else(|| {
                    HostError::InvalidFrame("provider override must be a string".to_owned())
                })?;
                model = Some((id.to_owned(), model.map(|(_, m)| m).unwrap_or_default()));
            }
            "budgets" | "budget" => {
                budget = Some(decode_budget_override(field)?);
            }
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "override field `{other}` is not supported by this host"
                )));
            }
        }
    }
    Ok((reasoning, model, budget))
}

/// Rejects override values this host cannot honour.
///
/// This is the `AGENT_OVERRIDE_DENIED` check the spec requires (RFC 7):
/// narrowing is allowed, widening scopes, tool sets or budgets is not. This
/// host has no scopes or tool sets to widen, and budgets can only be set —
/// there is no raise — so the check is that the value decodes at all. A
/// malformed value fails here, naming the field, instead of deep inside the
/// service.
fn validate_override(
    reasoning: Option<&norte_code_protocol::Reasoning>,
    model: Option<&(String, String)>,
    budget: Option<&norte_code_protocol::Budget>,
) -> Result<(), HostError> {
    if let Some(norte_code_protocol::Reasoning::Effort { effort }) = reasoning
        && effort.is_empty()
    {
        return Err(HostError::InvalidFrame(
            "reasoning.effort must not be empty".to_owned(),
        ));
    }
    if let Some((_, model_id)) = model
        && model_id.is_empty()
    {
        return Err(HostError::InvalidFrame(
            "model.id must not be empty".to_owned(),
        ));
    }
    if let Some(budget) = budget
        && budget.limit <= 0.0
    {
        return Err(HostError::InvalidFrame(
            "budget limit must be positive".to_owned(),
        ));
    }
    Ok(())
}

/// Decodes a `reasoning` override object into a [`norte_code_protocol::Reasoning`].
///
/// The wire shape follows the `AgentSpec` fragment (RFC 7): `mode`
/// (`off`/`auto`/`on`), `effort` (a name) and `budget_tokens`. `mode: on`
/// without an effort maps to `Effort` with the service's default effort;
/// `mode: auto` maps to `Automatic` and `mode: off` to `Disabled`.
fn decode_reasoning_override(value: &Value) -> Result<norte_code_protocol::Reasoning, HostError> {
    let Some(fields) = value.as_object() else {
        return Err(HostError::InvalidFrame(
            "reasoning override must be an object".to_owned(),
        ));
    };
    for key in fields.keys() {
        match key.as_str() {
            "mode" | "effort" | "budget_tokens" => {}
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "reasoning override field `{other}` is not supported"
                )));
            }
        }
    }
    let mode = fields.get("mode").and_then(Value::as_str).unwrap_or("auto");
    let effort = fields.get("effort").and_then(Value::as_str);
    let budget_tokens = fields.get("budget_tokens").and_then(Value::as_u64);
    Ok(match mode {
        "off" | "disabled" => norte_code_protocol::Reasoning::Disabled,
        "auto" | "automatic" => norte_code_protocol::Reasoning::Automatic,
        "on" => {
            if let Some(tokens) = budget_tokens {
                norte_code_protocol::Reasoning::Budget {
                    tokens: u32::try_from(tokens).map_err(|_| {
                        HostError::InvalidFrame(
                            "reasoning.budget_tokens is out of range".to_owned(),
                        )
                    })?,
                }
            } else if let Some(effort) = effort {
                norte_code_protocol::Reasoning::Effort {
                    effort: effort.to_owned(),
                }
            } else {
                norte_code_protocol::Reasoning::Effort {
                    effort: "medium".to_owned(),
                }
            }
        }
        other => {
            return Err(HostError::InvalidFrame(format!(
                "reasoning.mode `{other}` is not supported"
            )));
        }
    })
}

/// Decodes a `budgets` override object into a [`norte_code_protocol::Budget`].
///
/// The wire shape follows the `AgentSpec` fragment: `max_turn_cost` /
/// `max_session_cost` (with `currency`, default `USD`) and `max_turn_tokens`
/// map to `BudgetUnit::Usd` / `BudgetUnit::Tokens`. The service's current
/// budget semantics are a single per-session limit, so the first field
/// present wins; a field this host does not map fails instead of being
/// silently dropped.
fn decode_budget_override(value: &Value) -> Result<norte_code_protocol::Budget, HostError> {
    let Some(fields) = value.as_object() else {
        return Err(HostError::InvalidFrame(
            "budget override must be an object".to_owned(),
        ));
    };
    for key in fields.keys() {
        match key.as_str() {
            "max_turn_cost" | "max_session_cost" | "currency" | "max_turn_tokens"
            | "max_session_tokens" | "on_exceed" => {}
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "budget override field `{other}` is not supported"
                )));
            }
        }
    }
    let action = match fields
        .get("on_exceed")
        .and_then(Value::as_str)
        .unwrap_or("stop")
    {
        "ask" => norte_code_protocol::BudgetAction::Ask,
        "degrade" | "warn" => norte_code_protocol::BudgetAction::Warn,
        _ => norte_code_protocol::BudgetAction::Stop,
    };
    if let Some(cost) = fields
        .get("max_turn_cost")
        .or_else(|| fields.get("max_session_cost"))
        .and_then(Value::as_f64)
        .or_else(|| {
            fields
                .get("max_turn_cost")
                .or_else(|| fields.get("max_session_cost"))
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok().map(f64::from))
        })
    {
        return Ok(norte_code_protocol::Budget {
            unit: norte_code_protocol::BudgetUnit::Usd,
            limit: cost,
            action,
        });
    }
    if let Some(tokens) = fields
        .get("max_turn_tokens")
        .or_else(|| fields.get("max_session_tokens"))
        .and_then(Value::as_u64)
    {
        return Ok(norte_code_protocol::Budget {
            unit: norte_code_protocol::BudgetUnit::Tokens,
            limit: f64::from(u32::try_from(tokens).unwrap_or(u32::MAX)),
            action,
        });
    }
    Err(HostError::InvalidFrame(
        "budget override requires a cost or token limit".to_owned(),
    ))
}

/// A `SESSION_CONFIGURE` payload as this host accepts it (RFC 8.1).
///
/// The payload is the standard one — `agent_id`, `agent_override`,
/// `options`, `apply` — not a serialised [`AgentCommand`]. The host
/// translates its known fields onto service requests; anything it cannot
/// express fails with `AGENT_OVERRIDE_DENIED` instead of being ignored.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
struct SessionConfigurePayload {
    /// Switch to another agent. Only honoured when it names the host's
    /// configured agent; anything else is `agent_mismatch`.
    #[serde(default)]
    agent_id: Option<String>,
    /// Per-session configuration fragment. See [`decode_override`].
    #[serde(default)]
    agent_override: Option<Value>,
    /// Session options supported by this host.
    #[serde(default)]
    options: Option<Value>,
    /// When the change applies. This host supports immediate application only.
    #[serde(default)]
    apply: Option<ConfigureApplyPolicy>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConfigureApplyPolicy {
    Now,
    NextTurn,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionOptions {
    #[serde(default)]
    job_turn_policy: Option<norte_code_protocol::JobTurnPolicy>,
}

fn parse_job_turn_policy(
    options: Option<&Value>,
) -> Result<Option<norte_code_protocol::JobTurnPolicy>, HostError> {
    let Some(options) = options else {
        return Ok(None);
    };
    let options: SessionOptions = serde_json::from_value(options.clone()).map_err(|error| {
        HostError::InvalidFrame(format!("SESSION_OPTIONS_UNSUPPORTED: {error}"))
    })?;
    Ok(options.job_turn_policy)
}

/// Host configuration independent of the HTTP listener.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Agent identifier accepted during attachment.
    pub agent_id: String,
    /// Advertised server version.
    pub server_version: String,
    /// Maximum number of emitted frames retained per session.
    pub replay_capacity: usize,
    /// Maximum number of concurrently running jobs per session.
    pub max_concurrent_jobs: usize,
    /// Duration for which terminal job output remains queryable.
    pub job_output_retention_s: u64,
    /// Maximum number of command results retained for idempotent retries.
    pub idempotency_capacity: usize,
    /// Maximum user inputs queued behind an active turn.
    pub max_queued_inputs: usize,
    /// Maximum frame size advertised to clients.
    pub max_frame_bytes: u64,
    /// Optional bearer token required by operational HTTP routes.
    pub auth_token: Option<String>,
    /// Optional bearer-token to principal mappings for multi-user ownership.
    pub auth_principals: HashMap<String, String>,
    /// Browser origins allowed to call the HTTP API.
    ///
    /// A request that carries `Origin` is rejected unless the value is listed
    /// here. A non-browser client sends no `Origin` and is unaffected.
    pub allowed_origins: Vec<String>,
    /// Maximum size accepted for one uploaded artifact.
    pub max_artifact_bytes: usize,
    /// Maximum size of one portable session bundle.
    pub max_export_bytes: usize,
    /// Lifetime of generated portable bundles.
    pub export_ttl_s: u64,
    /// Maximum concurrent WebSocket and SSE connections.
    pub max_connections: usize,
    /// Maximum time spent handling one command, in milliseconds.
    pub command_timeout_ms: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            agent_id: "norte-code".to_owned(),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            replay_capacity: 10_000,
            max_concurrent_jobs: 8,
            job_output_retention_s: 86_400,
            idempotency_capacity: 4_096,
            max_queued_inputs: 8,
            max_frame_bytes: 1024 * 1024,
            auth_token: None,
            auth_principals: HashMap::new(),
            allowed_origins: vec![
                "http://localhost:8080".to_owned(),
                "http://127.0.0.1:8080".to_owned(),
            ],
            max_artifact_bytes: 16 * 1024 * 1024,
            max_export_bytes: 64 * 1024 * 1024,
            export_ttl_s: 3600,
            max_connections: 256,
            command_timeout_ms: 120_000,
        }
    }
}

/// Errors raised while translating or executing a RAEP frame.
#[derive(Debug, Error)]
pub enum HostError {
    /// The frame is invalid or cannot be used in the current connection state.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    /// The client sent a frame type this host does not implement.
    #[error("unknown frame type: {0}")]
    UnknownType(String),
    /// A frame id or turn id was reused for a different request.
    #[error("idempotency conflict: {0}")]
    IdempotencyConflict(String),
    /// A turn was requested while the session was occupied under reject policy.
    #[error("session is busy")]
    SessionBusy,
    /// A cancellation targeted a turn that is not active.
    #[error("turn is not active")]
    TurnNotActive,
    /// The Norte Code service rejected a request before streaming it.
    #[error("service request failed: {0}")]
    Service(String),
    /// Replay is unavailable for the requested sequence.
    #[error("replay unavailable: {0}")]
    Replay(String),
    /// A frame could not be serialized.
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The durable replay journal could not be read or written.
    #[error("replay journal failed: {0}")]
    ReplayJournal(String),
    /// An upload could not be stored or resolved.
    #[error("artifact failed: {0}")]
    Artifact(String),
}

/// Durable storage for RAEP frames.
#[async_trait]
pub trait ReplayStore: Send + Sync {
    /// Appends one emitted frame.
    async fn append(&self, frame: &Envelope<Value>) -> Result<(), HostError>;

    /// Loads frames for one session, including frames retained before restart.
    async fn load(&self, session_id: &str) -> Result<Vec<Envelope<Value>>, HostError>;

    /// Persists one request's idempotency record.
    async fn save_idempotency(&self, _record: &IdempotencyRecord) -> Result<(), HostError> {
        Ok(())
    }

    /// Loads idempotency records retained for one session.
    async fn load_idempotency(
        &self,
        _session_id: &str,
    ) -> Result<Vec<IdempotencyRecord>, HostError> {
        Ok(Vec::new())
    }
}

/// A durable request fingerprint and the response to return for a retry.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct IdempotencyRecord {
    /// Session scope (empty for connection-level commands).
    pub session_id: String,
    /// Client-provided request identifier.
    pub frame_id: FrameId,
    /// SHA-256 fingerprint of the complete request envelope.
    pub fingerprint: String,
    /// Optional client turn identifier.
    pub turn_id: Option<TurnId>,
    /// Fingerprint of the `USER_INPUT` payload, when applicable.
    pub turn_fingerprint: Option<String>,
    /// Norte Code's numeric turn key, used to restore client-ID correlation.
    pub service_turn_id: Option<u64>,
    /// Frames returned for the request.
    pub response: Vec<Envelope<Value>>,
}

/// A stored upload that can be referenced by a user input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactRef {
    /// Opaque upload identifier.
    pub id: String,
    /// MIME type supplied by the client.
    pub mime_type: String,
    /// Number of stored bytes.
    pub bytes: u64,
}

/// An upload receipt returned by the session-scoped uploads endpoint.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct UploadReceipt {
    /// Opaque upload identifier.
    pub upload_id: String,
    /// Stored media type.
    pub mime_type: String,
    /// Number of stored bytes.
    pub bytes: u64,
    /// SHA-256 digest in lowercase hexadecimal.
    pub sha256: String,
    /// Expiration time in Unix milliseconds.
    pub expires_at: i64,
}

/// Durable pin storage for `CONTEXT_PIN` (RFC 18.5).
///
/// Pins are session-scoped sets of turn ids. The host persists them here
/// so they survive a restart; the in-memory map in `HostState` is only a
/// cache.
#[async_trait]
pub trait PinStore: Send + Sync {
    /// Reads all pins for one session.
    async fn load(&self, session_id: &str) -> Result<Vec<String>, HostError>;
    /// Replaces all pins for one session.
    async fn save(&self, session_id: &str, pins: &[String]) -> Result<(), HostError>;
}

/// JSON-file pin storage for the RAEP server.
///
/// One file per session: `pins/<session_id>.json` with a JSON array of
/// turn ids. Writes are atomic (tmp + rename); reads tolerate a missing
/// file as `[]`.
#[derive(Clone, Debug)]
pub struct FilePinStore {
    dir: PathBuf,
}

impl FilePinStore {
    /// Opens a pin store rooted at `dir` (created on first write).
    #[must_use]
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_owned(),
        }
    }

    fn path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.json"))
    }
}

#[async_trait]
impl PinStore for FilePinStore {
    async fn load(&self, session_id: &str) -> Result<Vec<String>, HostError> {
        let path = self.path(session_id);
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(HostError::ReplayJournal(error.to_string())),
        };
        serde_json::from_str::<Vec<String>>(&content)
            .map_err(|error| HostError::ReplayJournal(format!("pin file: {error}")))
    }

    async fn save(&self, session_id: &str, pins: &[String]) -> Result<(), HostError> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        let path = self.path(session_id);
        let tmp = path.with_extension("json.tmp");
        let content = serde_json::to_string(pins).map_err(HostError::Serialization)?;
        tokio::fs::write(&tmp, content)
            .await
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        Ok(())
    }
}

/// Durable storage for client-uploaded artifacts.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Stores bytes and returns their opaque reference.
    async fn put(&self, mime_type: &str, bytes: &[u8]) -> Result<ArtifactRef, HostError>;

    /// Reads an uploaded artifact by its content-addressed identifier.
    async fn get(&self, id: &str) -> Result<Vec<u8>, HostError>;
}

/// Blob-backed artifact storage for the RAEP server.
pub struct BlobArtifactStore {
    blobs: Arc<dyn norte::BlobStore>,
}

impl BlobArtifactStore {
    /// Creates an artifact adapter over Norte's content-addressed blob store.
    #[must_use]
    pub fn new(blobs: Arc<dyn norte::BlobStore>) -> Self {
        Self { blobs }
    }
}

#[async_trait]
impl ArtifactStore for BlobArtifactStore {
    async fn put(&self, mime_type: &str, bytes: &[u8]) -> Result<ArtifactRef, HostError> {
        let id = self
            .blobs
            .put(bytes)
            .await
            .map_err(|error| HostError::Artifact(error.to_string()))?;
        Ok(ArtifactRef {
            id: id.to_string(),
            mime_type: mime_type.to_owned(),
            bytes: bytes.len() as u64,
        })
    }

    async fn get(&self, id: &str) -> Result<Vec<u8>, HostError> {
        let id = id
            .parse()
            .map_err(|_| HostError::Artifact("invalid artifact id".to_owned()))?;
        self.blobs
            .get(id)
            .await
            .map_err(|error| HostError::Artifact(error.to_string()))
    }
}

/// Metadata for a session-scoped upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadRecord {
    /// Opaque upload identifier returned to the client.
    pub id: String,
    /// Owning session.
    pub session_id: String,
    /// Owning authenticated principal, when authentication is enabled.
    pub owner: Option<String>,
    /// Content-addressed blob identifier in the Norte store.
    pub blob_id: String,
    /// Validated MIME type.
    pub mime_type: String,
    /// Number of uploaded bytes.
    pub bytes: u64,
    /// SHA-256 digest in lowercase hexadecimal.
    pub sha256: String,
    /// Expiration time in Unix milliseconds; referenced uploads do not expire.
    pub expires_at: i64,
    /// Whether a turn has referenced this upload.
    pub referenced: bool,
}

/// Durable or in-memory index for session-scoped upload metadata.
#[async_trait]
pub trait UploadIndex: Send + Sync {
    /// Inserts a new upload record.
    async fn insert(&self, record: UploadRecord) -> Result<(), HostError>;

    /// Looks up an upload by its opaque identifier.
    async fn get(&self, id: &str) -> Result<Option<UploadRecord>, HostError>;

    /// Keeps uploads referenced by an accepted turn for the session lifetime.
    async fn mark_referenced(
        &self,
        session_id: &str,
        owner: Option<&str>,
        ids: &[String],
    ) -> Result<(), HostError>;
}

/// In-memory upload index, useful for embedded hosts and tests.
#[derive(Default)]
pub struct MemoryUploadIndex {
    uploads: Mutex<HashMap<String, UploadRecord>>,
}

#[async_trait]
impl UploadIndex for MemoryUploadIndex {
    async fn insert(&self, record: UploadRecord) -> Result<(), HostError> {
        self.uploads.lock().await.insert(record.id.clone(), record);
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<UploadRecord>, HostError> {
        Ok(self.uploads.lock().await.get(id).cloned())
    }

    async fn mark_referenced(
        &self,
        session_id: &str,
        owner: Option<&str>,
        ids: &[String],
    ) -> Result<(), HostError> {
        let mut uploads = self.uploads.lock().await;
        for id in ids {
            let record = uploads
                .get_mut(id)
                .filter(|record| {
                    record.session_id == session_id && record.owner.as_deref() == owner
                })
                .ok_or_else(|| HostError::Artifact("upload not found".to_owned()))?;
            record.referenced = true;
        }
        Ok(())
    }
}

/// SQLite-backed upload metadata that survives server restarts.
pub struct SqliteUploadIndex {
    path: PathBuf,
}

impl SqliteUploadIndex {
    /// Opens or creates the upload metadata table.
    ///
    /// # Errors
    ///
    /// Returns an error when SQLite cannot open the database or create its
    /// schema.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, HostError> {
        let path = path.into();
        let connection = rusqlite::Connection::open(&path)
            .map_err(|error| HostError::Artifact(error.to_string()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| HostError::Artifact(error.to_string()))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS raep_uploads (
                    upload_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    owner TEXT,
                    blob_id TEXT NOT NULL,
                    mime_type TEXT NOT NULL,
                    bytes INTEGER NOT NULL,
                    sha256 TEXT NOT NULL,
                    expires_at INTEGER NOT NULL,
                    referenced INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_raep_uploads_session
                    ON raep_uploads(session_id, upload_id);",
            )
            .map_err(|error| HostError::Artifact(error.to_string()))?;
        Ok(Self { path })
    }
}

#[async_trait]
impl UploadIndex for SqliteUploadIndex {
    async fn insert(&self, record: UploadRecord) -> Result<(), HostError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .execute(
                    "DELETE FROM raep_uploads WHERE referenced = 0 AND expires_at <= ?1",
                    rusqlite::params![now_millis()],
                )
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .execute(
                    "INSERT INTO raep_uploads
                     (upload_id, session_id, owner, blob_id, mime_type, bytes, sha256, expires_at, referenced)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        record.id,
                        record.session_id,
                        record.owner,
                        record.blob_id,
                        record.mime_type,
                        i64::try_from(record.bytes)
                            .map_err(|_| HostError::Artifact("upload is too large".to_owned()))?,
                        record.sha256,
                        record.expires_at,
                        record.referenced,
                    ],
                )
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|error| HostError::Artifact(error.to_string()))?
    }

    async fn get(&self, id: &str) -> Result<Option<UploadRecord>, HostError> {
        let path = self.path.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            use rusqlite::OptionalExtension;
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .query_row(
                    "SELECT upload_id, session_id, owner, blob_id, mime_type, bytes, sha256, expires_at, referenced
                     FROM raep_uploads WHERE upload_id = ?1",
                    rusqlite::params![id],
                    |row| {
                        let bytes: i64 = row.get(5)?;
                        let bytes = u64::try_from(bytes).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                5,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?;
                        Ok(UploadRecord {
                            id: row.get(0)?,
                            session_id: row.get(1)?,
                            owner: row.get(2)?,
                            blob_id: row.get(3)?,
                            mime_type: row.get(4)?,
                            bytes,
                            sha256: row.get(6)?,
                            expires_at: row.get(7)?,
                            referenced: row.get(8)?,
                        })
                    },
                )
                .optional()
                .map_err(|error| HostError::Artifact(error.to_string()))
        })
        .await
        .map_err(|error| HostError::Artifact(error.to_string()))?
    }

    async fn mark_referenced(
        &self,
        session_id: &str,
        owner: Option<&str>,
        ids: &[String],
    ) -> Result<(), HostError> {
        let path = self.path.clone();
        let session_id = session_id.to_owned();
        let owner = owner.map(str::to_owned);
        let ids = ids.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            let transaction = connection
                .transaction()
                .map_err(|error| HostError::Artifact(error.to_string()))?;
            for id in ids {
                let updated = transaction
                    .execute(
                        "UPDATE raep_uploads SET referenced = 1
                         WHERE upload_id = ?1 AND session_id = ?2 AND owner IS ?3
                           AND (referenced = 1 OR expires_at > ?4)",
                        rusqlite::params![id, session_id, owner, now_millis()],
                    )
                    .map_err(|error| HostError::Artifact(error.to_string()))?;
                if updated != 1 {
                    return Err(HostError::Artifact("upload not found".to_owned()));
                }
            }
            transaction
                .commit()
                .map_err(|error| HostError::Artifact(error.to_string()))
        })
        .await
        .map_err(|error| HostError::Artifact(error.to_string()))?
    }
}

/// A newline-delimited JSON replay journal.
pub struct FileReplayStore {
    path: PathBuf,
    max_frames: usize,
    lock: tokio::sync::Mutex<()>,
}

/// SQLite-backed replay storage safe for multiple server processes.
pub struct SqliteReplayStore {
    path: PathBuf,
    max_frames: usize,
}

impl SqliteReplayStore {
    /// Opens or creates the replay table in the supplied database.
    ///
    /// # Errors
    ///
    /// Returns an error when SQLite cannot open the database or create its
    /// schema.
    pub fn open(path: impl Into<PathBuf>, max_frames: usize) -> Result<Self, HostError> {
        if max_frames == 0 {
            return Err(HostError::ReplayJournal(
                "replay journal capacity must be positive".to_owned(),
            ));
        }
        let path = path.into();
        let connection = rusqlite::Connection::open(&path)
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS raep_replay (
                    session_id TEXT NOT NULL,
                    seq INTEGER NOT NULL,
                    frame TEXT NOT NULL,
                    PRIMARY KEY (session_id, seq)
                );
                CREATE INDEX IF NOT EXISTS idx_raep_replay_session_seq
                    ON raep_replay(session_id, seq);
                CREATE TABLE IF NOT EXISTS raep_idempotency (
                    session_id TEXT NOT NULL,
                    frame_id TEXT NOT NULL,
                    turn_id TEXT,
                    fingerprint TEXT NOT NULL,
                    turn_fingerprint TEXT,
                    service_turn_id INTEGER,
                    response TEXT NOT NULL,
                    PRIMARY KEY (session_id, frame_id)
                );
                CREATE INDEX IF NOT EXISTS idx_raep_idempotency_session
                    ON raep_idempotency(session_id);",
            )
            .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        Ok(Self { path, max_frames })
    }
}

#[async_trait]
impl ReplayStore for SqliteReplayStore {
    async fn append(&self, frame: &Envelope<Value>) -> Result<(), HostError> {
        let path = self.path.clone();
        let max_frames = i64::try_from(self.max_frames)
            .map_err(|_| HostError::ReplayJournal("replay capacity is too large".to_owned()))?;
        let frame = frame.clone();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let encoded = serde_json::to_string(&frame)?;
            let sequence = frame.seq.ok_or_else(|| {
                HostError::ReplayJournal("replay frame has no sequence".to_owned())
            })?;
            let session_id = frame.session_id.clone();
            connection
                .execute(
                    "INSERT OR REPLACE INTO raep_replay(session_id, seq, frame)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![session_id, sequence, encoded],
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .execute(
                    "DELETE FROM raep_replay
                     WHERE session_id = ?1
                       AND seq NOT IN (
                           SELECT seq FROM raep_replay
                           WHERE session_id = ?1
                           ORDER BY seq DESC LIMIT ?2
                       )",
                    rusqlite::params![frame.session_id, max_frames],
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn load(&self, session_id: &str) -> Result<Vec<Envelope<Value>>, HostError> {
        let path = self.path.clone();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let mut statement = connection
                .prepare(
                    "SELECT frame FROM raep_replay
                     WHERE session_id = ?1 ORDER BY seq ASC",
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let rows = statement
                .query_map(rusqlite::params![session_id], |row| row.get::<_, String>(0))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            rows.map(|row| {
                let encoded = row.map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                serde_json::from_str(&encoded).map_err(HostError::from)
            })
            .collect()
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn save_idempotency(&self, record: &IdempotencyRecord) -> Result<(), HostError> {
        let path = self.path.clone();
        let max_entries = i64::try_from(self.max_frames).map_err(|_| {
            HostError::ReplayJournal("idempotency capacity is too large".to_owned())
        })?;
        let record = record.clone();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let response = serde_json::to_string(&record.response)?;
            connection
                .execute(
                    "INSERT OR REPLACE INTO raep_idempotency
                     (session_id, frame_id, turn_id, fingerprint, turn_fingerprint,
                      service_turn_id, response)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        record.session_id,
                        record.frame_id.as_str(),
                        record.turn_id.as_ref().map(TurnId::as_str),
                        record.fingerprint,
                        record.turn_fingerprint,
                        record.service_turn_id,
                        response
                    ],
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .execute(
                    "DELETE FROM raep_idempotency
                     WHERE session_id = ?1
                       AND frame_id NOT IN (
                           SELECT frame_id FROM raep_idempotency
                           WHERE session_id = ?1
                           ORDER BY rowid DESC LIMIT ?2
                       )",
                    rusqlite::params![record.session_id, max_entries],
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn load_idempotency(
        &self,
        session_id: &str,
    ) -> Result<Vec<IdempotencyRecord>, HostError> {
        let path = self.path.clone();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let connection = rusqlite::Connection::open(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let mut statement = connection
                .prepare(
                    "SELECT frame_id, turn_id, fingerprint, turn_fingerprint,
                            service_turn_id, response
                     FROM raep_idempotency
                     WHERE session_id = ?1 ORDER BY rowid ASC",
                )
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            let rows = statement
                .query_map(rusqlite::params![session_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<u64>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            rows.map(|row| {
                let (frame_id, turn_id, fingerprint, turn_fingerprint, service_turn_id, response) =
                    row.map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                let frame_id = FrameId::new(frame_id)
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                let turn_id = turn_id
                    .map(TurnId::new)
                    .transpose()
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                let response = serde_json::from_str(&response)?;
                Ok(IdempotencyRecord {
                    session_id: session_id.clone(),
                    frame_id,
                    fingerprint,
                    turn_id,
                    turn_fingerprint,
                    service_turn_id,
                    response,
                })
            })
            .collect()
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }
}

impl FileReplayStore {
    /// Opens a journal at `path`, creating its parent directory when needed.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory cannot be created.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, HostError> {
        let path = path.into();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
        }
        Ok(Self {
            path,
            max_frames: 10_000,
            lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Opens a journal with a bounded number of frames retained per session.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory cannot be created or the
    /// capacity is zero.
    pub fn open_with_capacity(
        path: impl Into<PathBuf>,
        max_frames: usize,
    ) -> Result<Self, HostError> {
        if max_frames == 0 {
            return Err(HostError::ReplayJournal(
                "replay journal capacity must be positive".to_owned(),
            ));
        }
        let mut store = Self::open(path)?;
        store.max_frames = max_frames;
        Ok(store)
    }
}

#[async_trait]
impl ReplayStore for FileReplayStore {
    async fn append(&self, frame: &Envelope<Value>) -> Result<(), HostError> {
        let _guard = self.lock.lock().await;
        let line = serde_json::to_string(frame)?;
        let path = self.path.clone();
        let max_frames = self.max_frames;
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let mut by_session: HashMap<String, Vec<Envelope<Value>>> = HashMap::new();
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                for existing in content.lines().filter(|line| !line.trim().is_empty()) {
                    let frame = serde_json::from_str::<Envelope<Value>>(existing)
                        .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                    by_session
                        .entry(frame.session_id.clone())
                        .or_default()
                        .push(frame);
                }
            }
            let appended = serde_json::from_str::<Envelope<Value>>(&line)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            by_session
                .entry(appended.session_id.clone())
                .or_default()
                .push(appended);
            let mut retained = by_session
                .into_values()
                .flat_map(|mut frames| {
                    if frames.len() > max_frames {
                        frames.drain(..frames.len() - max_frames);
                    }
                    frames
                })
                .collect::<Vec<_>>();
            retained.sort_by_key(|frame| frame.seq.unwrap_or_default());
            let temporary = path.with_extension("jsonl.tmp");
            let mut file = std::fs::File::create(&temporary)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            for frame in retained {
                let line = serde_json::to_string(&frame)?;
                writeln!(file, "{line}")
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            }
            std::fs::rename(temporary, path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn load(&self, session_id: &str) -> Result<Vec<Envelope<Value>>, HostError> {
        let _guard = self.lock.lock().await;
        let path = self.path.clone();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            if !Path::new(&path).exists() {
                return Ok(Vec::new());
            }
            let content = std::fs::read_to_string(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str::<Envelope<Value>>(line)
                        .map_err(|error| HostError::ReplayJournal(error.to_string()))
                })
                .filter_map(|result| match result {
                    Ok(frame) if frame.session_id == session_id => Some(Ok(frame)),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect()
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn save_idempotency(&self, record: &IdempotencyRecord) -> Result<(), HostError> {
        let _guard = self.lock.lock().await;
        let path = self.path.with_extension("idempotency.jsonl");
        let record = record.clone();
        let max_entries = self.max_frames;
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let mut records = Vec::new();
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                for line in content.lines().filter(|line| !line.trim().is_empty()) {
                    let existing = serde_json::from_str::<IdempotencyRecord>(line)
                        .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
                    if existing.session_id != record.session_id
                        || existing.frame_id != record.frame_id
                    {
                        records.push(existing);
                    }
                }
            }
            records.push(record);
            let mut seen = HashMap::<String, usize>::new();
            let mut retained = records
                .into_iter()
                .rev()
                .filter(|entry| {
                    let count = seen.entry(entry.session_id.clone()).or_default();
                    let keep = *count < max_entries;
                    *count += 1;
                    keep
                })
                .collect::<Vec<_>>();
            retained.reverse();
            let temporary = path.with_extension("idempotency.jsonl.tmp");
            let mut file = std::fs::File::create(&temporary)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            for entry in retained {
                writeln!(file, "{}", serde_json::to_string(&entry)?)
                    .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            }
            std::fs::rename(temporary, path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }

    async fn load_idempotency(
        &self,
        session_id: &str,
    ) -> Result<Vec<IdempotencyRecord>, HostError> {
        let _guard = self.lock.lock().await;
        let path = self.path.with_extension("idempotency.jsonl");
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            if !path.exists() {
                return Ok(Vec::new());
            }
            let content = std::fs::read_to_string(path)
                .map_err(|error| HostError::ReplayJournal(error.to_string()))?;
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str::<IdempotencyRecord>(line)
                        .map_err(|error| HostError::ReplayJournal(error.to_string()))
                })
                .filter_map(|record| match record {
                    Ok(record) if record.session_id == session_id => Some(Ok(record)),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect()
        })
        .await
        .map_err(|error| HostError::ReplayJournal(error.to_string()))?
    }
}

#[derive(Default)]
struct HostState {
    attached: HashSet<String>,
    attachment_modes: HashMap<String, AttachMode>,
    sequences: HashMap<String, u64>,
    replay: HashMap<String, VecDeque<Envelope<Value>>>,
    idempotency: HashMap<(String, FrameId), CachedRequest>,
    idempotency_order: VecDeque<(String, FrameId)>,
    loaded_replay: HashSet<String>,
    session_owners: HashMap<String, String>,
    artifact_owners: HashMap<String, String>,
    exports: HashMap<String, StoredExport>,
    imports: HashMap<String, StoredImport>,
    session_agents: HashMap<String, String>,
    session_create_keys: HashMap<String, String>,
    session_create_order: VecDeque<String>,
    client_capabilities: HashMap<String, raep_types::ClientCapabilities>,
    pending_output: HashMap<String, Vec<raep_types::Modality>>,
    turn_output: HashMap<(String, u64), Vec<raep_types::Modality>>,
    pending_turn_ids: HashMap<String, VecDeque<TurnId>>,
    turn_aliases: HashMap<(String, u64), TurnId>,
    turn_alias_order: VecDeque<(String, u64)>,
    active_turns: HashMap<String, TurnId>,
    turn_reservations: HashMap<String, TurnId>,
    turn_request_fingerprints: HashMap<(String, TurnId), String>,
    turn_request_frames: HashMap<(String, TurnId), Envelope<Value>>,
    turn_request_order: VecDeque<(String, TurnId)>,
    loaded_idempotency: HashSet<String>,
    queued_turns: HashMap<String, VecDeque<QueuedTurn>>,
    /// Pinned turns per session (RFC 18.5). Held in-memory; the service
    /// has no pin table yet, so pins do not survive a restart.
    pinned_turns: HashMap<String, Vec<String>>,
}

#[derive(Clone)]
struct QueuedTurn {
    turn_id: TurnId,
    text: String,
    attachments: Vec<norte_code_protocol::Attachment>,
    budget: Option<norte_code_protocol::Budget>,
    reasoning: Option<norte_code_protocol::Reasoning>,
    metadata: Option<Value>,
    output: Vec<raep_types::Modality>,
}

struct CachedRequest {
    fingerprint: String,
    response: Vec<Envelope<Value>>,
}

struct StoredExport {
    bytes: Vec<u8>,
    owner: Option<String>,
    expires_at: i64,
}

#[derive(Clone)]
struct StoredImport {
    manifest: crate::bundle::BundleManifest,
    history: Vec<norte_code_agent::service::ImportedHistoryItem>,
    title: Option<String>,
    owner: String,
    expires_at: i64,
}

/// A Norte Code service exposed through the RAEP wire.
pub struct NorteCodeRaepHost {
    service: Arc<Service>,
    services: HashMap<String, Arc<Service>>,
    config: HostConfig,
    state: Mutex<HostState>,
    events: broadcast::Sender<Envelope<Value>>,
    replay_store: Option<Arc<dyn ReplayStore>>,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    upload_index: Option<Arc<dyn UploadIndex>>,
    pin_store: Option<Arc<dyn PinStore>>,
    connections: Arc<Semaphore>,
}

impl NorteCodeRaepHost {
    /// Creates a host around the server-owned Norte Code service.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured agent identifier is empty or the
    /// replay capacity is zero.
    pub fn new(service: Arc<Service>, config: HostConfig) -> Result<Self, HostError> {
        if config.agent_id.is_empty() {
            return Err(HostError::InvalidFrame(
                "agent_id must not be empty".to_owned(),
            ));
        }
        if config.replay_capacity == 0 {
            return Err(HostError::InvalidFrame(
                "replay_capacity must be positive".to_owned(),
            ));
        }
        if config.max_queued_inputs == 0 {
            return Err(HostError::InvalidFrame(
                "max_queued_inputs must be positive".to_owned(),
            ));
        }
        if config.max_connections == 0 {
            return Err(HostError::InvalidFrame(
                "max_connections must be positive".to_owned(),
            ));
        }
        if config.max_export_bytes == 0 || config.export_ttl_s == 0 {
            return Err(HostError::InvalidFrame(
                "export size and TTL limits must be positive".to_owned(),
            ));
        }
        service.configure_job_limits(config.max_concurrent_jobs, config.job_output_retention_s);
        let max_connections = config.max_connections;
        let (events, _) = broadcast::channel(config.replay_capacity.max(16));
        Ok(Self {
            services: HashMap::from([(config.agent_id.clone(), Arc::clone(&service))]),
            service,
            config,
            state: Mutex::new(HostState::default()),
            events,
            replay_store: None,
            artifact_store: None,
            upload_index: None,
            pin_store: None,
            connections: Arc::new(Semaphore::new(max_connections)),
        })
    }

    /// Creates a host exposing several named agent services.
    ///
    /// The entry named by `config.agent_id` is the default used by
    /// connection-scoped requests and by clients that do not name an agent.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured default agent is not present.
    pub fn with_agents(
        services: HashMap<String, Arc<Service>>,
        config: HostConfig,
    ) -> Result<Self, HostError> {
        let service = services.get(&config.agent_id).cloned().ok_or_else(|| {
            HostError::InvalidFrame(format!(
                "no service configured for agent `{}`",
                config.agent_id
            ))
        })?;
        let mut host = Self::new(service, config)?;
        for service in services.values() {
            service.configure_job_limits(
                host.config.max_concurrent_jobs,
                host.config.job_output_retention_s,
            );
        }
        host.services = services;
        Ok(host)
    }

    /// Creates a host with a durable RAEP replay journal.
    ///
    /// # Errors
    ///
    /// Returns an error when the host configuration or journal path is invalid.
    pub fn with_replay_store(
        service: Arc<Service>,
        config: HostConfig,
        replay_store: Arc<dyn ReplayStore>,
    ) -> Result<Self, HostError> {
        let mut host = Self::new(service, config)?;
        host.replay_store = Some(replay_store);
        Ok(host)
    }

    /// Enables a durable replay journal on an already configured host.
    #[must_use]
    pub fn with_replay_store_instance(mut self, replay_store: Arc<dyn ReplayStore>) -> Self {
        self.replay_store = Some(replay_store);
        self
    }

    /// Enables durable storage for client-uploaded artifacts.
    #[must_use]
    pub fn with_artifact_store(mut self, artifact_store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(artifact_store);
        self
    }

    /// Enables session-scoped uploads with durable metadata.
    #[must_use]
    pub fn with_upload_index(mut self, upload_index: Arc<dyn UploadIndex>) -> Self {
        self.upload_index = Some(upload_index);
        self
    }

    /// Enables durable pin storage for `CONTEXT_PIN`.
    #[must_use]
    pub fn with_pin_store(mut self, pin_store: Arc<dyn PinStore>) -> Self {
        self.pin_store = Some(pin_store);
        self
    }

    /// Stores an uploaded artifact.
    ///
    /// # Errors
    ///
    /// Returns an error when artifact storage is disabled or the upload fails.
    pub async fn put_artifact(
        &self,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<ArtifactRef, HostError> {
        self.put_artifact_as(mime_type, bytes, None).await
    }

    /// Stores an artifact and records ownership for an authenticated principal.
    ///
    /// # Errors
    ///
    /// Returns an error when storage is disabled, the artifact exceeds the
    /// configured limit, or the backing store rejects the write.
    pub async fn put_artifact_as(
        &self,
        mime_type: &str,
        bytes: &[u8],
        principal: Option<&str>,
    ) -> Result<ArtifactRef, HostError> {
        if bytes.len() > self.config.max_artifact_bytes {
            return Err(HostError::Artifact(format!(
                "artifact exceeds {} byte limit",
                self.config.max_artifact_bytes
            )));
        }
        let reference = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("artifact storage is disabled".to_owned()))?
            .put(mime_type, bytes)
            .await?;
        if let Some(principal) = principal {
            self.state
                .lock()
                .await
                .artifact_owners
                .insert(reference.id.clone(), principal.to_owned());
        }
        Ok(reference)
    }

    /// Stores an authenticated upload scoped to one session.
    ///
    /// # Errors
    ///
    /// Returns an error when uploads are disabled, the MIME type is not
    /// supported, the upload exceeds the configured limit, or storage fails.
    pub async fn put_session_upload_as(
        &self,
        session_id: &str,
        mime_type: &str,
        bytes: &[u8],
        owner: Option<&str>,
    ) -> Result<UploadReceipt, HostError> {
        if self
            .session_summary(session_id, owner.unwrap_or_default())
            .await
            .is_none()
        {
            return Err(HostError::Artifact("session not found".to_owned()));
        }
        if bytes.is_empty() {
            return Err(HostError::Artifact("upload must not be empty".to_owned()));
        }
        if bytes.len() > self.config.max_artifact_bytes {
            return Err(HostError::Artifact(format!(
                "upload exceeds {} byte limit",
                self.config.max_artifact_bytes
            )));
        }
        let kind = if mime_type.starts_with("image/") {
            AcceptedPartKind::Image
        } else if mime_type.starts_with("audio/") {
            AcceptedPartKind::Audio
        } else if mime_type.starts_with("video/") {
            AcceptedPartKind::Video
        } else {
            AcceptedPartKind::Document
        };
        validate_media_mime(kind, mime_type).map_err(HostError::Artifact)?;
        let artifact_store = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("artifact storage is disabled".to_owned()))?;
        let upload_index = self
            .upload_index
            .as_ref()
            .ok_or_else(|| HostError::Artifact("upload metadata storage is disabled".to_owned()))?;
        let artifact = artifact_store.put(mime_type, bytes).await?;
        let upload_id = format!("upl_{}", uuid::Uuid::new_v4());
        let expires_at = now_millis().saturating_add(60 * 60 * 1000);
        let sha256 = crate::bundle::sha256(bytes);
        upload_index
            .insert(UploadRecord {
                id: upload_id.clone(),
                session_id: session_id.to_owned(),
                owner: owner.map(str::to_owned),
                blob_id: artifact.id,
                mime_type: mime_type.to_owned(),
                bytes: bytes.len() as u64,
                sha256: sha256.clone(),
                expires_at,
                referenced: false,
            })
            .await?;
        Ok(UploadReceipt {
            upload_id,
            mime_type: mime_type.to_owned(),
            bytes: bytes.len() as u64,
            sha256,
            expires_at,
        })
    }

    async fn resolve_session_asset(
        &self,
        session_id: &str,
        asset_id: &str,
        owner: Option<&str>,
        output: bool,
    ) -> Result<UploadRecord, HostError> {
        let index = self
            .upload_index
            .as_ref()
            .ok_or_else(|| HostError::Artifact("upload metadata storage is disabled".to_owned()))?;
        let expected_prefix = if output { "out_" } else { "upl_" };
        let record = index
            .get(asset_id)
            .await?
            .filter(|record| {
                record.session_id == session_id
                    && record.owner.as_deref() == owner
                    && (record.referenced || record.expires_at > now_millis())
                    && record.id.starts_with(expected_prefix)
                    && (!output || record.referenced)
            })
            .ok_or_else(|| HostError::Artifact("upload not found".to_owned()))?;
        Ok(record)
    }

    /// Fetches one generated output after checking its session and owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the output is unknown, expired, foreign, or the
    /// backing blob cannot be read.
    pub async fn get_session_output_as(
        &self,
        session_id: &str,
        output_id: &str,
        owner: Option<&str>,
    ) -> Result<(Vec<u8>, String), HostError> {
        let record = self
            .resolve_session_asset(session_id, output_id, owner, true)
            .await?;
        let bytes = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("artifact storage is disabled".to_owned()))?
            .get(&record.blob_id)
            .await?;
        Ok((bytes, record.mime_type))
    }

    /// Reads an uploaded artifact.
    ///
    /// # Errors
    ///
    /// Returns an error when artifact storage is disabled or the identifier is
    /// invalid or missing.
    pub async fn get_artifact(&self, id: &str) -> Result<Vec<u8>, HostError> {
        self.get_artifact_as(id, None).await
    }

    /// Reads an artifact after checking its authenticated owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the artifact is not owned by the principal,
    /// storage is disabled, or the backing store cannot resolve the id.
    pub async fn get_artifact_as(
        &self,
        id: &str,
        principal: Option<&str>,
    ) -> Result<Vec<u8>, HostError> {
        if let Some(principal) = principal
            && let Some(owner) = self.state.lock().await.artifact_owners.get(id)
            && owner != principal
        {
            return Err(HostError::Artifact(
                "artifact belongs to another principal".to_owned(),
            ));
        }
        self.artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("artifact storage is disabled".to_owned()))?
            .get(id)
            .await
    }

    /// Reads an unexpired portable bundle after checking its owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the export is missing, expired, or owned by a
    /// different principal.
    pub async fn get_export_as(
        &self,
        id: &str,
        principal: &str,
    ) -> Result<(Vec<u8>, i64), HostError> {
        let mut state = self.state.lock().await;
        if state
            .exports
            .get(id)
            .is_some_and(|export| export.expires_at <= now_millis())
        {
            state.exports.remove(id);
        }
        let export = state
            .exports
            .get(id)
            .ok_or_else(|| HostError::Artifact("export not found or expired".to_owned()))?;
        if export
            .owner
            .as_deref()
            .is_some_and(|owner| owner != principal)
        {
            return Err(HostError::Artifact(
                "export not found or expired".to_owned(),
            ));
        }
        Ok((export.bytes.clone(), export.expires_at))
    }

    /// Validates a portable bundle and stores a one-hour import preview.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid/oversized bundle, unsupported history
    /// content, or a preview store failure.
    pub async fn preview_import_as(
        &self,
        bytes: &[u8],
        principal: &str,
    ) -> Result<Value, HostError> {
        if bytes.len() > self.config.max_export_bytes {
            return Err(HostError::InvalidFrame(
                "PAYLOAD_TOO_LARGE: import exceeds max_export_bytes".to_owned(),
            ));
        }
        let (manifest, files) = crate::bundle::read_bundle(bytes, self.config.max_export_bytes)
            .map_err(|error| HostError::InvalidFrame(format!("BUNDLE_INVALID: {error}")))?;
        if manifest.sessions.len() != 1
            || manifest.sessions[0].session_id != manifest.root_session_id
            || manifest.sessions[0].parent_session_id.is_some()
            || files.len() != 2
        {
            return Err(HostError::InvalidFrame(
                "BUNDLE_UNSUPPORTED: this host imports a single text-only root session".to_owned(),
            ));
        }
        let session = &manifest.sessions[0];
        if let Some(usage) = session.usage_total.as_ref() {
            validate_bundle_usage(usage)?;
        }
        let items = parse_portable_history(files.get(&session.history_file).ok_or_else(|| {
            HostError::InvalidFrame("BUNDLE_INVALID: history file is missing".to_owned())
        })?)?;
        let session_json_path = format!("sessions/{}/session.json", session.session_id);
        let session_json: Value =
            serde_json::from_slice(files.get(&session_json_path).ok_or_else(|| {
                HostError::InvalidFrame("BUNDLE_INVALID: session.json is missing".to_owned())
            })?)?;
        let title = session_json
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if title.as_ref().is_some_and(|title| title.len() > 512) {
            return Err(HostError::InvalidFrame(
                "BUNDLE_INVALID: session title exceeds 512 bytes".to_owned(),
            ));
        }
        let agent_id = session
            .agent
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                HostError::InvalidFrame("BUNDLE_INVALID: session agent id is missing".to_owned())
            })?;
        let version = session
            .agent
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let available = self.services.contains_key(agent_id)
            && (version.is_empty() || version == self.config.server_version);
        let expires_at = now_millis().saturating_add(60 * 60 * 1000);
        let import_id = format!("imp_{}", uuid::Uuid::new_v4().simple());
        {
            let mut state = self.state.lock().await;
            state
                .imports
                .retain(|_, import| import.expires_at > now_millis());
            if state.imports.len() >= 4 {
                return Err(HostError::InvalidFrame(
                    "IMPORT_STORE_FULL: active preview limit reached".to_owned(),
                ));
            }
            state.imports.insert(
                import_id.clone(),
                StoredImport {
                    manifest: manifest.clone(),
                    history: items,
                    title,
                    owner: principal.to_owned(),
                    expires_at,
                },
            );
        }
        Ok(payload(ImportPreviewPayload {
            import_id: &import_id,
            bundle_version: &manifest.bundle_version,
            sessions: manifest.sessions.len(),
            signature: ImportSignatureStatus::Absent,
            agents: vec![ImportAgentAvailability {
                agent_id,
                version,
                available,
            }],
            warnings: [
                "Only plain-text user and assistant history is restored.",
                "Children, uploads, job output, generated outputs, and workspace data are not imported.",
            ],
            expires_at,
        }))
    }

    /// Commits a previously validated portable bundle preview.
    ///
    /// # Errors
    ///
    /// Returns an error for an expired/foreign preview, unavailable agent,
    /// unsupported usage carry-over, or a failed session creation.
    pub(crate) async fn commit_import_as(
        &self,
        import_id: &str,
        request: ImportCommitRequest,
        principal: &str,
    ) -> Result<Value, HostError> {
        if request.decryption_jwk.is_some() {
            return Err(HostError::InvalidFrame(
                "encrypted imports are not supported".to_owned(),
            ));
        }
        let imported = {
            let mut state = self.state.lock().await;
            if state
                .imports
                .get(import_id)
                .is_some_and(|import| import.expires_at <= now_millis())
            {
                state.imports.remove(import_id);
            }
            let imported = state
                .imports
                .get(import_id)
                .ok_or_else(|| HostError::InvalidFrame("IMPORT_NOT_FOUND".to_owned()))?;
            if imported.owner != principal {
                return Err(HostError::InvalidFrame("IMPORT_NOT_FOUND".to_owned()));
            }
            imported.clone()
        };
        let source = &imported.manifest.sessions[0];
        let usage_total = request
            .carry_usage
            .unwrap_or(true)
            .then(|| source.usage_total.clone())
            .flatten();
        let agent_id = source
            .agent
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                HostError::InvalidFrame("BUNDLE_INVALID: missing agent id".to_owned())
            })?;
        let version = source
            .agent
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mapped = request
            .agent_map
            .get(&format!("{agent_id}@{version}"))
            .or_else(|| request.agent_map.get(agent_id))
            .map_or(agent_id, String::as_str);
        let service = self.services.get(mapped).cloned().ok_or_else(|| {
            HostError::InvalidFrame(format!("AGENT_NOT_FOUND: `{mapped}` is unavailable"))
        })?;
        {
            let mut state = self.state.lock().await;
            let can_consume = state.imports.get(import_id).is_some_and(|import| {
                import.owner == principal && import.expires_at > now_millis()
            });
            if !can_consume {
                return Err(HostError::InvalidFrame("IMPORT_NOT_FOUND".to_owned()));
            }
            state.imports.remove(import_id);
        }
        let imported_from = payload(ImportedFromPayload {
            exporter: &imported.manifest.exporter,
            session_id: &source.session_id,
            exported_at: imported.manifest.exported_at,
        });
        let key = service
            .import_plain_history(imported.title, imported.history, imported_from, usage_total)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        {
            let mut state = self.state.lock().await;
            state
                .session_agents
                .insert(key.to_string(), mapped.to_owned());
            state
                .session_owners
                .insert(key.to_string(), principal.to_owned());
        }
        let id_map = BTreeMap::from([(source.session_id.clone(), key.to_string())]);
        Ok(payload(ImportCommittedPayload {
            session_id: key.as_str(),
            id_map,
            warnings: [
                "Only plain-text user and assistant history was restored.",
                "The imported session is suspended until its first controller attachment.",
            ],
        }))
    }

    /// Removes an import preview owned by `principal`.
    pub async fn delete_import_as(&self, import_id: &str, principal: &str) -> bool {
        let mut state = self.state.lock().await;
        if state
            .imports
            .get(import_id)
            .is_some_and(|import| import.owner == principal)
        {
            state.imports.remove(import_id);
            true
        } else {
            false
        }
    }

    /// Returns the server-owned agent service.
    #[must_use]
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }

    /// Returns the configured agent identifier.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.config.agent_id
    }

    /// Maximum configured upload size.
    #[must_use]
    pub fn max_artifact_bytes(&self) -> usize {
        self.config.max_artifact_bytes
    }

    fn output_part_bytes_limit(&self) -> usize {
        self.config
            .max_artifact_bytes
            .min(DEFAULT_MAX_OUTPUT_PART_BYTES)
    }

    /// Whether authenticated single-shot uploads are configured.
    #[must_use]
    pub fn uploads_enabled(&self) -> bool {
        self.artifact_store.is_some() && self.upload_index.is_some()
    }

    /// Maximum configured portable bundle size.
    #[must_use]
    pub fn max_export_bytes(&self) -> usize {
        self.config.max_export_bytes
    }

    fn service_for_agent(&self, agent_id: &str) -> Result<Arc<Service>, HostError> {
        let requested = if agent_id.is_empty() {
            &self.config.agent_id
        } else {
            agent_id
        };
        self.services
            .get(requested)
            .cloned()
            .ok_or_else(|| HostError::InvalidFrame(format!("unknown agent `{requested}`")))
    }

    async fn service_for_session(&self, session_id: &str) -> Result<Arc<Service>, HostError> {
        if let Some(agent_id) = self.agent_for_session(session_id).await {
            return self.service_for_agent(&agent_id);
        }
        Ok(Arc::clone(&self.service))
    }

    async fn agent_for_session(&self, session_id: &str) -> Option<String> {
        if let Some(agent_id) = self
            .state
            .lock()
            .await
            .session_agents
            .get(session_id)
            .cloned()
        {
            return Some(agent_id);
        }
        self.services
            .iter()
            .find(|(_, service)| {
                service
                    .sessions()
                    .iter()
                    .any(|session| session.session.as_str() == session_id)
            })
            .map(|(agent_id, _)| agent_id.clone())
    }

    /// Returns the RAEP `session_summary` for every session `principal` may see.
    ///
    /// A session with no recorded owner is visible to everyone; a session with
    /// an owner is visible only to that principal.
    pub async fn session_summaries(&self, principal: &str) -> Vec<Value> {
        let owners = {
            let state = self.state.lock().await;
            state.session_owners.clone()
        };
        let now = now_millis();
        self.services
            .values()
            .flat_map(|service| service.sessions())
            .filter(|info| {
                owners
                    .get(info.session.as_str())
                    .is_none_or(|owner| owner == principal)
            })
            .map(|info| self.session_summary_value(&info, now))
            .collect()
    }

    /// Returns one `session_summary`.
    ///
    /// `None` means the session does not exist or belongs to another principal,
    /// which the HTTP resource answers as `404` rather than leaking existence.
    pub async fn session_summary(&self, session_id: &str, principal: &str) -> Option<Value> {
        let owner = self
            .state
            .lock()
            .await
            .session_owners
            .get(session_id)
            .cloned();
        if owner.is_some_and(|owner| owner != principal) {
            return None;
        }
        let service = self.service_for_session(session_id).await.ok()?;
        let info = service
            .sessions()
            .into_iter()
            .find(|info| info.session.as_str() == session_id)?;
        Some(self.session_summary_value(&info, now_millis()))
    }

    /// Returns child sessions only when the principal owns the parent session.
    pub async fn child_sessions(
        &self,
        parent_session_id: &str,
        principal: &str,
    ) -> Option<Vec<norte_code_agent::service::ChildSessionSummary>> {
        self.session_summary(parent_session_id, principal).await?;
        let service = self.service_for_session(parent_session_id).await.ok()?;
        Some(service.children_of(&SessionKey::new(parent_session_id)))
    }

    fn session_summary_value(&self, info: &norte_code_protocol::SessionInfo, now: i64) -> Value {
        let created_at = info
            .created_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map_or(now, |value| value.timestamp_millis());
        let agent_id = self
            .services
            .iter()
            .find(|(_, service)| {
                service
                    .sessions()
                    .iter()
                    .any(|session| session.session == info.session)
            })
            .map_or(self.config.agent_id.as_str(), |(agent_id, _)| {
                agent_id.as_str()
            });
        payload(SessionSummary {
            session_id: info.session.as_str(),
            title: info.title.as_deref(),
            title_source: SessionTitleSource::User,
            labels: BTreeMap::new(),
            pinned: false,
            archived: false,
            status: if self
                .services
                .values()
                .any(|service| service.session_imported_suspended(&info.session))
            {
                SessionSummaryStatus::Suspended
            } else {
                SessionSummaryStatus::Active
            },
            agent: AgentRef {
                id: agent_id,
                version: &self.config.server_version,
            },
            created_at,
            updated_at: now,
            last_activity_at: now,
            turn_count: 0,
            has_pending: false,
        })
    }

    /// Creates a session through the management API and records its owner.
    ///
    /// # Errors
    ///
    /// Returns a service error when the session cannot be created.
    pub async fn create_session(
        &self,
        agent_id: &str,
        title: Option<String>,
        principal: &str,
    ) -> Result<String, HostError> {
        let key = self
            .service_for_agent(agent_id)?
            .start_session(title)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.claim_session(key.as_str(), principal).await?;
        Ok(key.to_string())
    }

    /// Renames a session through the management API.
    ///
    /// Returns `false` when the session is unknown or belongs to another
    /// principal.
    ///
    /// # Errors
    ///
    /// Returns a service error when the rename cannot be persisted.
    pub async fn rename_session(
        &self,
        session_id: &str,
        title: &str,
        principal: &str,
    ) -> Result<bool, HostError> {
        if self.session_summary(session_id, principal).await.is_none() {
            return Ok(false);
        }
        self.service_for_session(session_id)
            .await?
            .rename(&SessionKey::new(session_id), title)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        Ok(true)
    }

    /// Deletes a session through the management API.
    ///
    /// Returns `false` when the session is unknown or belongs to another
    /// principal.
    ///
    /// # Errors
    ///
    /// Returns a service error when the delete is rejected.
    pub async fn delete_session(
        &self,
        session_id: &str,
        principal: &str,
    ) -> Result<bool, HostError> {
        if self.session_summary(session_id, principal).await.is_none() {
            return Ok(false);
        }
        self.submit_service(
            session_id,
            ClientRequestKind::DeleteSession {
                session: SessionKey::new(session_id),
            },
        )
        .await?;
        self.state.lock().await.attached.remove(session_id);
        Ok(true)
    }

    /// Returns the session a previous `Idempotency-Key` created.
    pub async fn session_for_create_key(&self, key: &str) -> Option<String> {
        self.state
            .lock()
            .await
            .session_create_keys
            .get(key)
            .cloned()
    }

    /// Remembers the session created for an `Idempotency-Key`.
    ///
    /// The map keeps the same bound as the command idempotency cache so a
    /// client that sends a fresh key on every create cannot grow it without
    /// limit.
    pub async fn remember_create_key(&self, key: &str, session_id: &str) {
        let mut state = self.state.lock().await;
        state
            .session_create_keys
            .insert(key.to_owned(), session_id.to_owned());
        state.session_create_order.push_back(key.to_owned());
        while state.session_create_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.session_create_order.pop_front() {
                state.session_create_keys.remove(&oldest);
            }
        }
    }

    /// Returns the `agent_summary` for every agent this host serves.
    #[must_use]
    pub fn agents(&self) -> Vec<Value> {
        let mut ids = self.services.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let spec = self
                    .services
                    .get(&id)
                    .and_then(|service| service.default_spec());
                payload(AgentSummary {
                    id: &id,
                    version: &self.config.server_version,
                    capabilities: agent_capabilities(spec.as_ref(), self.uploads_enabled()),
                    provider: spec.as_ref().map(|spec| AgentRef {
                        id: &spec.provider,
                        version: "",
                    }),
                    model: spec.as_ref().map(|spec| AgentRef {
                        id: &spec.model,
                        version: "",
                    }),
                })
            })
            .collect()
    }

    /// Returns one `agent_summary` and its latest version, or `None` when the
    /// agent is unknown.
    #[must_use]
    pub fn agent(&self, agent_id: &str) -> Option<Value> {
        let service = self.services.get(agent_id)?;
        let spec = service.default_spec();
        Some(payload(AgentDetail {
            agent: AgentSummary {
                id: agent_id,
                version: &self.config.server_version,
                capabilities: agent_capabilities(spec.as_ref(), self.uploads_enabled()),
                provider: spec.as_ref().map(|spec| AgentRef {
                    id: &spec.provider,
                    version: "",
                }),
                model: spec.as_ref().map(|spec| AgentRef {
                    id: &spec.model,
                    version: "",
                }),
            },
            latest_version: AgentVersion {
                version: &self.config.server_version,
                created_at: 0,
                deprecated: false,
            },
        }))
    }

    /// Returns the versions an agent has, or `None` when the agent is unknown.
    #[must_use]
    pub fn agent_versions(&self, agent_id: &str) -> Option<Value> {
        self.services.get(agent_id)?;
        Some(payload(AgentVersions {
            versions: vec![AgentVersion {
                version: &self.config.server_version,
                created_at: 0,
                deprecated: false,
            }],
        }))
    }

    /// Returns the configured bearer token, when transport authentication is enabled.
    #[must_use]
    pub fn auth_token(&self) -> Option<&str> {
        self.config.auth_token.as_deref()
    }

    /// Resolves a bearer token to its configured principal.
    ///
    /// When authentication is disabled, all requests use the anonymous
    /// principal. A single legacy token maps to `default` unless an explicit
    /// principal mapping exists.
    #[must_use]
    pub fn authenticate_bearer(&self, token: &str) -> Option<String> {
        let configured = self.config.auth_token.as_deref();
        if configured.is_none() && self.config.auth_principals.is_empty() {
            return None;
        }
        if configured != Some(token) && !self.config.auth_principals.contains_key(token) {
            return None;
        }
        Some(
            self.config
                .auth_principals
                .get(token)
                .cloned()
                .unwrap_or_else(|| "default".to_owned()),
        )
    }

    /// Returns the principal used for unauthenticated local operation.
    #[must_use]
    pub fn anonymous_principal(&self) -> &'static str {
        "anonymous"
    }

    /// Returns whether this host requires transport authentication.
    #[must_use]
    pub fn authentication_enabled(&self) -> bool {
        self.config.auth_token.is_some() || !self.config.auth_principals.is_empty()
    }

    /// Returns whether a browser origin may call the HTTP API.
    #[must_use]
    pub fn origin_allowed(&self, origin: &str) -> bool {
        self.config
            .allowed_origins
            .iter()
            .any(|allowed| allowed == origin)
    }

    /// Attempts to reserve one long-lived transport connection.
    pub fn try_connection(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(&self.connections).try_acquire_owned().ok()
    }

    /// Returns the configured command timeout.
    #[must_use]
    pub fn command_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.config.command_timeout_ms)
    }

    /// Returns the maximum accepted RAEP frame size.
    #[must_use]
    pub fn max_frame_bytes(&self) -> usize {
        usize::try_from(self.config.max_frame_bytes).unwrap_or(usize::MAX)
    }

    /// Returns quota for one principal.
    ///
    /// Norte Code has no tenant quota store yet, so the answer is derived
    /// from what the host knows: sessions the principal owns, turns, and
    /// replay frames. The limits are the host's own configured limits;
    /// `used`/`remaining` come from in-memory counters.
    #[must_use]
    pub async fn quota(&self, principal: &str) -> Value {
        let (sessions, replay_frames) = {
            let state = self.state.lock().await;
            let sessions = state
                .session_owners
                .values()
                .filter(|owner| *owner == principal)
                .count() as u64;
            let frames = state.replay.values().map(VecDeque::len).sum::<usize>() as u64;
            (sessions, frames)
        };
        let session_limit = 100_u64;
        let frame_limit = self.config.replay_capacity as u64;
        payload(QuotaPayload {
            tenant: principal,
            period: QuotaPeriod { start: 0, end: 0 },
            dimensions: [
                QuotaDimension {
                    name: QuotaDimensionName::Sessions,
                    limit: session_limit,
                    used: sessions,
                    remaining: session_limit.saturating_sub(sessions),
                },
                QuotaDimension {
                    name: QuotaDimensionName::ReplayBufferEvents,
                    limit: frame_limit,
                    used: replay_frames,
                    remaining: frame_limit.saturating_sub(replay_frames),
                },
            ],
        })
    }

    /// Returns a receiver for live frames emitted by any attached connection.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope<Value>> {
        self.events.subscribe()
    }

    /// Spawns the drain from the service's delegation events into this
    /// host's frame broadcast (RFC 8.2.10).
    ///
    /// The service emits `ChildSessionStarted/Updated` and
    /// `DelegatorStateChanged` on its own broadcast when a child is spawned
    /// or its delegator is suspended; the host turns each one into the
    /// standard frame on the parent's stream, with the same `seq` ordering
    /// as every other frame of that session. Without this drain the events
    /// would sit in the service's channel with no reader.
    ///
    /// # Errors
    ///
    /// Returns an error when the delegation receiver has already been taken:
    /// the host takes it exactly once, at startup.
    pub fn spawn_delegation_drain(self: &Arc<Self>) -> Result<(), HostError> {
        let receiver = self.service.delegation_rx_take().ok_or_else(|| {
            HostError::InvalidFrame("delegation receiver already taken".to_owned())
        })?;
        let host = Arc::clone(self);
        tokio::spawn(async move {
            let mut receiver = receiver;
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        let session_id = event
                            .session
                            .as_ref()
                            .map_or_else(String::new, |key| key.as_str().to_owned());
                        if session_id.is_empty() {
                            continue;
                        }
                        if let Err(error) = host.emit_event(&session_id, event).await {
                            tracing::warn!(%error, session = %session_id, "delegation event could not be emitted");
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(lost)) => {
                        tracing::warn!(lost, "delegation event stream lagged; frames were dropped");
                    }
                }
            }
        });
        Ok(())
    }

    async fn current_seq(&self, session_id: &str) -> u64 {
        self.state
            .lock()
            .await
            .sequences
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }

    /// Returns the RAEP handshake frame.
    #[must_use]
    pub fn hello(&self) -> Envelope<Value> {
        Envelope::new(
            standard(StandardFrameType::Hello),
            payload(HelloPayload {
                protocol: ProtocolRef {
                    name: "RAEP",
                    version: WIRE_VERSION,
                },
                server_version: &self.config.server_version,
                wire_versions: vec![WIRE_VERSION],
                capabilities: advertised_capabilities(self.uploads_enabled()),
                extensions: vec![NorteCodeExtension::Command.as_str()],
                limits: LimitsRef {
                    max_frame_bytes: self.config.max_frame_bytes,
                    replay_buffer_events: self.config.replay_capacity,
                    max_concurrent_jobs: self.config.max_concurrent_jobs,
                    job_output_retention_s: self.config.job_output_retention_s,
                    max_artifact_bytes: self.config.max_artifact_bytes,
                    max_upload_bytes: self.config.max_artifact_bytes,
                    max_inline_bytes: 0,
                    max_output_part_bytes: self.output_part_bytes_limit(),
                    max_prompt_bytes: 256 * 1024,
                    max_parts: 16,
                    max_queued_inputs: self.config.max_queued_inputs,
                    max_export_bytes: self.config.max_export_bytes,
                    export_ttl_s: self.config.export_ttl_s,
                },
                heartbeat_s: 20,
                draining: false,
                deprecations: Vec::new(),
            }),
            now_millis(),
        )
    }

    /// Returns discovery information for an HTTP info route.
    #[must_use]
    pub fn info(&self) -> Value {
        let default_model = match self
            .services
            .get(&self.config.agent_id)
            .and_then(|service| service.default_spec())
        {
            Some(spec) => payload(ModelRef {
                provider: &spec.provider,
                model: &spec.model,
            }),
            None => Value::Null,
        };
        payload(InfoPayload {
            protocol: ProtocolRef {
                name: "RAEP",
                version: WIRE_VERSION,
            },
            server_version: &self.config.server_version,
            wire_versions: vec![WIRE_VERSION],
            capabilities: advertised_capabilities(self.uploads_enabled()),
            extensions: vec![NorteCodeExtension::Command.as_str()],
            limits: LimitsRef {
                max_frame_bytes: self.config.max_frame_bytes,
                replay_buffer_events: self.config.replay_capacity,
                max_concurrent_jobs: self.config.max_concurrent_jobs,
                job_output_retention_s: self.config.job_output_retention_s,
                max_artifact_bytes: self.config.max_artifact_bytes,
                max_upload_bytes: self.config.max_artifact_bytes,
                max_inline_bytes: 0,
                max_output_part_bytes: self.output_part_bytes_limit(),
                max_prompt_bytes: 256 * 1024,
                max_parts: 16,
                max_queued_inputs: self.config.max_queued_inputs,
                max_export_bytes: self.config.max_export_bytes,
                export_ttl_s: self.config.export_ttl_s,
            },
            agents: self.services.keys().cloned().collect::<Vec<_>>(),
            default_agent: &self.config.agent_id,
            default_model,
        })
    }

    /// Returns readiness and basic operational counters for health probes.
    #[must_use]
    pub fn readiness(&self) -> Value {
        payload(ReadinessPayload {
            status: ReadinessStatus::Ready,
            agent_id: &self.config.agent_id,
            agents: self.services.len(),
            replay: self.replay_store.is_some(),
            artifacts: self.artifact_store.is_some(),
            limits: ReadinessLimits {
                max_connections: self.config.max_connections,
                command_timeout_ms: self.config.command_timeout_ms,
            },
        })
    }

    /// Returns a structured telemetry report for operational dashboards.
    ///
    /// The command remains the source of truth for the records. This method
    /// adds stable aggregate counters without changing the streaming wire
    /// event consumed by existing clients.
    ///
    /// # Errors
    ///
    /// Returns an error when the telemetry command cannot be routed or its
    /// response cannot be serialized.
    pub async fn telemetry_report(
        &self,
        session: Option<&str>,
        principal: &str,
    ) -> Result<Value, HostError> {
        let session_key = session.map(SessionKey::new);
        let request = ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::ListTelemetry {
                session: session_key,
            },
        };
        let frame = Envelope::new(
            FrameType::Extension(NorteCodeExtension::Command.as_str().to_owned()),
            serde_json::to_value(request)?,
            now_millis(),
        );
        let frames = self.handle_as(frame, Some(principal)).await?;
        // The telemetry records are the service's own `RequestRecord` struct
        // inside the canonical `service_data` payload: decode them through
        // the type instead of reading token counters by string key.
        let records: Vec<norte_code_protocol::sqlite::RequestRecord> = frames
            .iter()
            .find_map(|frame| {
                let event: CanonicalServiceData =
                    serde_json::from_value(frame.payload.clone()).ok()?;
                (event.kind == ServiceDataKind::Telemetry).then_some(event.data)
            })
            .and_then(|data| serde_json::from_value(data).ok())
            .unwrap_or_default();
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut cache_read_tokens = 0_u64;
        let mut cache_write_tokens = 0_u64;
        let mut errors = 0_u64;
        let mut cancelled = 0_u64;
        let mut duration_ms = 0_u64;
        for row in &records {
            input_tokens += row.input_tokens.unwrap_or(0);
            output_tokens += row.output_tokens.unwrap_or(0);
            cache_read_tokens += row.cache_read_tokens.unwrap_or(0);
            cache_write_tokens += row.cache_write_tokens.unwrap_or(0);
            duration_ms += row.duration_ms;
            match row.status {
                norte_code_protocol::sqlite::RequestStatus::Error => errors += 1,
                norte_code_protocol::sqlite::RequestStatus::Cancelled => cancelled += 1,
                norte_code_protocol::sqlite::RequestStatus::Ok => {}
            }
        }
        let request_count = records.len();
        Ok(payload(TelemetryReport {
            session: session.map(str::to_owned),
            records,
            summary: TelemetrySummary {
                requests: request_count,
                errors,
                cancelled,
                duration_ms,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
            },
        }))
    }

    /// Handles one decoded RAEP frame.
    ///
    /// Service events are published as soon as they arrive, while the
    /// returned vector provides the same frames to a request/response
    /// transport. This keeps transports independent from the agent loop.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame is malformed, unsupported, unauthenticated
    /// for the current attachment, or rejected by the Norte Code service.
    ///
    pub async fn handle(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.handle_as(frame, None).await
    }

    /// Handles one frame while enforcing ownership for `principal`.
    ///
    /// `None` is reserved for in-process callers and keeps the existing local
    /// behavior. HTTP and WebSocket adapters pass the authenticated principal.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame is invalid or the principal does not
    /// own the requested session.
    #[allow(clippy::too_many_lines)]
    pub async fn handle_as(
        &self,
        frame: Envelope<Value>,
        principal: Option<&str>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        if frame.version != WIRE_VERSION {
            return Err(HostError::InvalidFrame(
                "unsupported RAEP wire version".to_owned(),
            ));
        }
        if frame.seq.is_some() {
            return Err(HostError::InvalidFrame(
                "client frames must not include seq".to_owned(),
            ));
        }
        let frame_fingerprint = crate::bundle::sha256(&serde_json::to_vec(&frame)?);
        let idempotency_key = (frame.session_id.clone(), frame.id.clone());
        self.load_persisted_idempotency(&frame.session_id).await?;
        let is_user_input = matches!(
            &frame.kind,
            FrameType::Standard(StandardFrameType::UserInput)
        );
        let turn_id = is_user_input.then(|| frame.turn_id.clone()).flatten();
        let turn_fingerprint = if is_user_input {
            Some(crate::bundle::sha256(&serde_json::to_vec(&frame.payload)?))
        } else {
            None
        };
        if let Some(principal) = principal
            && !frame.session_id.is_empty()
        {
            self.claim_session(&frame.session_id, principal).await?;
        }
        if !matches!(
            &frame.kind,
            FrameType::Standard(
                StandardFrameType::SessionAttach | StandardFrameType::SessionDetach,
            )
        ) && self
            .state
            .lock()
            .await
            .attachment_modes
            .get(&frame.session_id)
            .is_some_and(|mode| *mode == AttachMode::Observer)
            && !observer_read_only_frame(&frame.kind)
        {
            return Err(HostError::InvalidFrame(
                "FORBIDDEN: observer attachments are read-only".to_owned(),
            ));
        }
        {
            let state = self.state.lock().await;
            if let Some(previous) = state.idempotency.get(&idempotency_key) {
                if previous.fingerprint == frame_fingerprint {
                    if matches!(
                        &frame.kind,
                        FrameType::Standard(StandardFrameType::UserInput)
                    ) && let Some(turn_id) = &frame.turn_id
                        && let Some(latest) = state
                            .turn_request_frames
                            .get(&(frame.session_id.clone(), turn_id.clone()))
                    {
                        return Ok(vec![latest.clone()]);
                    }
                    return Ok(previous.response.clone());
                }
                return Err(HostError::IdempotencyConflict(
                    "frame id was reused with a different request".to_owned(),
                ));
            }
        }
        let is_attach = matches!(
            &frame.kind,
            FrameType::Standard(StandardFrameType::SessionAttach)
        );
        let result = match frame.kind {
            FrameType::Standard(StandardFrameType::SessionAttach) => self.attach(frame).await,
            FrameType::Standard(StandardFrameType::UserInput) => {
                self.user_input(frame, principal).await
            }
            FrameType::Standard(StandardFrameType::CancelTurn) => self.cancel(frame).await,
            FrameType::Standard(StandardFrameType::SessionDetach) => self.detach(frame).await,
            FrameType::Standard(StandardFrameType::SessionClose) => self.close(frame).await,
            FrameType::Standard(StandardFrameType::Ack) => self.ack(frame).await,
            FrameType::Standard(StandardFrameType::SessionConfigure) => {
                self.agent_command(frame).await
            }
            FrameType::Standard(StandardFrameType::HistoryGet) => self.history_get(frame).await,
            FrameType::Standard(StandardFrameType::CancelJob) => self.cancel_job(frame).await,
            FrameType::Standard(StandardFrameType::JobList) => self.job_list(frame).await,
            FrameType::Standard(StandardFrameType::JobGet) => self.job_get(frame).await,
            FrameType::Standard(StandardFrameType::SessionFork) => self.fork(frame).await,
            FrameType::Standard(StandardFrameType::SessionCompact) => self.compact(frame).await,
            FrameType::Standard(StandardFrameType::ContextPin) => self.context_pin(frame).await,
            FrameType::Standard(StandardFrameType::SessionExport) => {
                self.export(frame, principal).await
            }
            FrameType::Standard(StandardFrameType::SessionImport) => Self::import(),
            FrameType::Standard(StandardFrameType::ToolApprovalResponse) => {
                Self::approval_response(&frame)
            }
            FrameType::Standard(StandardFrameType::SessionRewind) => self.rewind(frame).await,
            FrameType::Standard(StandardFrameType::InputStreamStart) => {
                Self::input_stream(&frame, "start")
            }
            FrameType::Standard(StandardFrameType::InputStreamData) => {
                Self::input_stream(&frame, "data")
            }
            FrameType::Standard(StandardFrameType::InputStreamEnd) => {
                Self::input_stream(&frame, "end")
            }
            FrameType::Standard(StandardFrameType::ClientToolResult) => {
                Self::client_tool_result(&frame)
            }
            FrameType::Standard(StandardFrameType::PresenceUpdate) => Self::presence_update(&frame),
            FrameType::Standard(StandardFrameType::DelegatorControl) => {
                self.delegator_control(frame).await
            }
            FrameType::Standard(StandardFrameType::Handoff) => Self::handoff(&frame),
            FrameType::Standard(StandardFrameType::AuthRefresh) => Self::auth_refresh(&frame),
            FrameType::Extension(ref name) if name == NorteCodeExtension::Command.as_str() => {
                self.extension_command(frame).await
            }
            kind => Err(HostError::UnknownType(format!("{kind:?}"))),
        }?;
        if is_attach && let Some(principal) = principal {
            for attached in &result {
                if !attached.session_id.is_empty() {
                    self.claim_session(&attached.session_id, principal).await?;
                }
            }
        }
        let service_turn_id = if let Some(turn_id) = &turn_id {
            self.state
                .lock()
                .await
                .turn_aliases
                .iter()
                .find(|((session_id, _), alias)| {
                    session_id == &idempotency_key.0 && *alias == turn_id
                })
                .map(|((_, service_turn), _)| *service_turn)
        } else {
            None
        };
        if let Some(store) = &self.replay_store {
            store
                .save_idempotency(&IdempotencyRecord {
                    session_id: idempotency_key.0.clone(),
                    frame_id: idempotency_key.1.clone(),
                    fingerprint: frame_fingerprint.clone(),
                    turn_id,
                    turn_fingerprint,
                    service_turn_id,
                    response: result.clone(),
                })
                .await?;
        }
        let mut state = self.state.lock().await;
        state.idempotency.insert(
            idempotency_key.clone(),
            CachedRequest {
                fingerprint: frame_fingerprint,
                response: result.clone(),
            },
        );
        state.idempotency_order.push_back(idempotency_key);
        while state.idempotency_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.idempotency_order.pop_front() {
                state.idempotency.remove(&oldest);
            }
        }
        Ok(result)
    }

    async fn claim_session(&self, session_id: &str, principal: &str) -> Result<(), HostError> {
        let mut state = self.state.lock().await;
        match state.session_owners.get(session_id) {
            Some(owner) if owner != principal => Err(HostError::InvalidFrame(
                "session belongs to another principal".to_owned(),
            )),
            Some(_) => Ok(()),
            None => {
                state
                    .session_owners
                    .insert(session_id.to_owned(), principal.to_owned());
                Ok(())
            }
        }
    }

    /// Handles a standard `SESSION_CONFIGURE` frame (RFC 8.1).
    ///
    /// The payload is the standard one — `agent_id`, `agent_override`,
    /// `options`, `apply` — not a serialised [`AgentCommand`]. Known
    /// override fields are translated onto service requests; anything the
    /// host cannot express fails with `AGENT_OVERRIDE_DENIED` instead of
    /// being silently dropped.
    async fn agent_command(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let configure: SessionConfigurePayload = serde_json::from_value(frame.payload)?;
        let job_turn_policy = parse_job_turn_policy(configure.options.as_ref())?;
        if job_turn_policy.is_some()
            && configure
                .apply
                .is_some_and(|apply| apply != ConfigureApplyPolicy::Now)
        {
            return Err(HostError::InvalidFrame(
                "SESSION_OPTIONS_UNSUPPORTED: job_turn_policy only supports apply=now".to_owned(),
            ));
        }
        let session_id = frame.session_id;
        let session = SessionKey::new(session_id.clone());
        if let Some(agent_id) = &configure.agent_id
            && agent_id != &self.config.agent_id
        {
            return Err(HostError::InvalidFrame(format!(
                "agent_mismatch: session runs agent `{}`",
                self.config.agent_id
            )));
        }
        let override_value = configure.agent_override.unwrap_or(Value::Null);
        let (reasoning, model, budget) = if override_value.is_null() {
            (None, None, None)
        } else {
            let decoded = decode_override(&override_value)?;
            validate_override(decoded.0.as_ref(), decoded.1.as_ref(), decoded.2.as_ref())?;
            decoded
        };
        let mut output = Vec::new();
        if let Some((provider, model_id)) = model {
            output.extend(
                self.submit_service(
                    &session_id,
                    ClientRequestKind::SetModel {
                        session: session.clone(),
                        provider,
                        model: model_id,
                    },
                )
                .await?,
            );
        }
        if let Some(reasoning) = reasoning {
            output.extend(
                self.submit_service(
                    &session_id,
                    ClientRequestKind::SetReasoning {
                        session: session.clone(),
                        scope: norte_code_protocol::ReasoningScope::Session,
                        reasoning,
                    },
                )
                .await?,
            );
        }
        if let Some(budget) = budget {
            output.extend(
                self.submit_service(
                    &session_id,
                    ClientRequestKind::SetBudget {
                        session: session.clone(),
                        budget,
                    },
                )
                .await?,
            );
        }
        if let Some(policy) = job_turn_policy {
            let service = self.service_for_session(&session_id).await?;
            if !service.set_job_turn_policy(&session, policy) {
                return Err(HostError::InvalidFrame(
                    "session not found while applying job_turn_policy".to_owned(),
                ));
            }
        }
        if output.is_empty() {
            output.push(
                self.emit(
                    &session_id,
                    StandardFrameType::SessionConfigured,
                    payload(SessionConfiguredPayload { applied: true }),
                )
                .await?,
            );
        }
        Ok(output)
    }

    /// Returns replay frames strictly after `last_seq`.
    ///
    /// # Errors
    ///
    /// Returns an error when the session has no retained replay window or the
    /// requested sequence predates the retained window.
    pub async fn replay_after(
        &self,
        session_id: &str,
        last_seq: u64,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let state = self.state.lock().await;
        let frames = state
            .replay
            .get(session_id)
            .ok_or_else(|| HostError::Replay("session replay window is missing".to_owned()))?;
        if let Some(first) = frames.front().and_then(|frame| frame.seq)
            && last_seq.saturating_add(1) < first
        {
            return Err(HostError::Replay(
                "requested sequence is older than the retained window".to_owned(),
            ));
        }
        Ok(frames
            .iter()
            .filter(|frame| frame.seq.is_some_and(|seq| seq > last_seq))
            .cloned()
            .collect())
    }

    #[allow(clippy::too_many_lines)]
    async fn attach(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        let attach: SessionAttach = serde_json::from_value(frame.payload)?;
        let client_capabilities = attach.capabilities.clone().unwrap_or_default();
        let job_turn_policy = parse_job_turn_policy(attach.options.as_ref())?;
        let mode = attach.mode.unwrap_or(AttachMode::Controller);
        if mode == AttachMode::Observer && job_turn_policy.is_some() {
            return Err(HostError::InvalidFrame(
                "FORBIDDEN: observer attachments cannot configure job_turn_policy".to_owned(),
            ));
        }
        let agent_id = if attach.agent_id.is_empty() {
            self.config.agent_id.clone()
        } else {
            attach.agent_id.clone()
        };
        let service = self.service_for_agent(&agent_id)?;
        let (session_id, created) = if frame.session_id.is_empty() {
            let key = service
                .start_session(None)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?;
            (key.to_string(), true)
        } else {
            (frame.session_id.clone(), false)
        };
        let exists = created
            || self
                .service_for_session(&session_id)
                .await
                .ok()
                .and_then(|service| service.session_title(&SessionKey::new(session_id.clone())))
                .is_some()
            || service
                .session_title(&SessionKey::new(session_id.clone()))
                .is_some()
            || service
                .session_spec(&SessionKey::new(session_id.clone()))
                .is_some()
            || service
                .sessions()
                .iter()
                .any(|session| session.session.as_str() == session_id);
        if !exists && attach.create_if_missing == Some(false) {
            return Err(HostError::InvalidFrame("session not found".to_owned()));
        }
        // This v1 host permits one attachment at a time. Takeover needs a
        // connection identity so it can reliably displace the prior controller.
        let already_attached = self.state.lock().await.attached.contains(&session_id);
        if already_attached {
            return Err(HostError::InvalidFrame(if attach.takeover == Some(true) {
                "SESSION_BUSY: takeover is unavailable on this host; detach first".to_owned()
            } else {
                "SESSION_BUSY: another client is already attached to this session".to_owned()
            }));
        }
        // `agent_mismatch` (RFC 6): for an existing session, the requested
        // agent must equal the session's current agent.
        if exists && !created {
            let current_agent = self
                .agent_for_session(&session_id)
                .await
                .unwrap_or_else(|| self.config.agent_id.clone());
            if current_agent != agent_id {
                return Err(HostError::InvalidFrame(format!(
                    "agent_mismatch: session runs agent `{current_agent}`"
                )));
            }
        }
        // `agent_override` on attach (RFC 6/7): same fragment as in
        // `SESSION_CONFIGURE`. Applied through the same translation, after
        // the session is bound but before `SESSION_ATTACHED` is emitted.
        let mut override_frames = Vec::new();
        if let Some(ref override_value) = attach.agent_override {
            let decoded = decode_override(override_value)?;
            validate_override(decoded.0.as_ref(), decoded.1.as_ref(), decoded.2.as_ref())?;
            let session = SessionKey::new(session_id.clone());
            if let Some((provider, model_id)) = decoded.1 {
                override_frames.extend(
                    self.submit_service(
                        &session_id,
                        ClientRequestKind::SetModel {
                            session: session.clone(),
                            provider,
                            model: model_id,
                        },
                    )
                    .await?,
                );
            }
            if let Some(reasoning) = decoded.0 {
                override_frames.extend(
                    self.submit_service(
                        &session_id,
                        ClientRequestKind::SetReasoning {
                            session: session.clone(),
                            scope: norte_code_protocol::ReasoningScope::Session,
                            reasoning,
                        },
                    )
                    .await?,
                );
            }
            if let Some(budget) = decoded.2 {
                override_frames.extend(
                    self.submit_service(
                        &session_id,
                        ClientRequestKind::SetBudget {
                            session: session.clone(),
                            budget,
                        },
                    )
                    .await?,
                );
            }
        }
        self.state
            .lock()
            .await
            .session_agents
            .insert(session_id.clone(), agent_id);
        self.load_persisted_replay(&session_id).await?;
        {
            let mut state = self.state.lock().await;
            state.attached.insert(session_id.clone());
            state
                .attachment_modes
                .insert(session_id.clone(), mode.clone());
            state
                .client_capabilities
                .insert(session_id.clone(), client_capabilities.clone());
            state.sequences.entry(session_id.clone()).or_insert(0);
            state.replay.entry(session_id.clone()).or_default();
        }
        let (replayed, replay_meta) = match attach.resume_from_seq {
            Some(after) => match self.replay_after(&session_id, after).await {
                Ok(frames) => {
                    let to_seq = frames
                        .iter()
                        .filter_map(|frame| frame.seq)
                        .max()
                        .unwrap_or(after);
                    (
                        frames,
                        ReplayMeta {
                            from_seq: Some(after.saturating_add(1)),
                            to_seq: Some(to_seq),
                            truncated: false,
                        },
                    )
                }
                Err(HostError::Replay(_)) => (
                    Vec::new(),
                    ReplayMeta {
                        from_seq: Some(after.saturating_add(1)),
                        to_seq: Some(after),
                        truncated: true,
                    },
                ),
                Err(error) => return Err(error),
            },
            None => (
                Vec::new(),
                ReplayMeta {
                    from_seq: None,
                    to_seq: None,
                    truncated: false,
                },
            ),
        };
        let last_seq = self.current_seq(&session_id).await;
        let attached = self
            .emit(
                &session_id,
                StandardFrameType::SessionAttached,
                payload(SessionAttachedPayload {
                    created,
                    mode,
                    last_seq,
                    replay: replay_meta,
                    negotiated_capabilities: negotiated_capabilities(
                        self.uploads_enabled(),
                        &client_capabilities,
                    ),
                    agent: session_agent_summary(
                        &service,
                        &self.config.agent_id,
                        &self.config.server_version,
                        &SessionKey::new(session_id.clone()),
                        self.uploads_enabled(),
                    ),
                }),
            )
            .await?;
        let session_key = SessionKey::new(session_id.clone());
        let imported_from = service.session_imported_from(&session_key);
        service
            .activate_imported_session(&session_key)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        if !service.set_job_turn_policy(
            &session_key,
            job_turn_policy.unwrap_or(norte_code_protocol::JobTurnPolicy::Always),
        ) {
            return Err(HostError::InvalidFrame(
                "session not found while applying job_turn_policy".to_owned(),
            ));
        }
        let title = service.session_title(&SessionKey::new(session_id.clone()));
        let history = service
            .session_history_with_provenance(&SessionKey::new(session_id.clone()))
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        let history_start = history.len().saturating_sub(20);
        let history_summary = history
            .into_iter()
            .skip(history_start)
            .map(|(turn, role, kind, text, timestamp, origin)| {
                history_item_value(HistoryItemView {
                    turn,
                    role,
                    kind,
                    text,
                    timestamp,
                    rewound: false,
                    origin,
                    max_content_bytes: Some(2 * 1024),
                })
            })
            .collect();
        let state = self
            .emit(
                &session_id,
                StandardFrameType::SessionState,
                payload(SessionStatePayload {
                    status: SessionStateStatus::Active,
                    title: title.as_deref(),
                    history_summary,
                    imported_from: imported_from.as_ref(),
                }),
            )
            .await?;
        let mut output = vec![attached];
        output.extend(replayed);
        output.push(state);
        output.extend(override_frames);
        Ok(output)
    }

    async fn load_persisted_replay(&self, session_id: &str) -> Result<(), HostError> {
        let Some(store) = &self.replay_store else {
            return Ok(());
        };
        if self.state.lock().await.loaded_replay.contains(session_id) {
            return Ok(());
        }
        let persisted = store.load(session_id).await?;
        let mut state = self.state.lock().await;
        let mut max_seq = 0;
        let replayed = persisted
            .into_iter()
            .filter(|frame| {
                if let Some(seq) = frame.seq {
                    max_seq = max_seq.max(seq);
                    true
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        if max_seq > 0 {
            state
                .sequences
                .entry(session_id.to_owned())
                .and_modify(|current| *current = (*current).max(max_seq))
                .or_insert(max_seq);
        }
        let replay = state.replay.entry(session_id.to_owned()).or_default();
        replay.extend(replayed);
        while replay.len() > self.config.replay_capacity {
            replay.pop_front();
        }
        let turn_states = replay
            .iter()
            .filter(|frame| {
                matches!(
                    frame.kind,
                    FrameType::Standard(StandardFrameType::TurnStateChanged)
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        for frame in turn_states {
            if let Some(turn_id) = &frame.turn_id {
                let key = (session_id.to_owned(), turn_id.clone());
                if state.turn_request_fingerprints.contains_key(&key) {
                    state.turn_request_frames.insert(key, frame);
                }
            }
        }
        state.loaded_replay.insert(session_id.to_owned());
        Ok(())
    }

    async fn load_persisted_idempotency(&self, session_id: &str) -> Result<(), HostError> {
        let Some(store) = &self.replay_store else {
            return Ok(());
        };
        if self
            .state
            .lock()
            .await
            .loaded_idempotency
            .contains(session_id)
        {
            return Ok(());
        }
        let records = store.load_idempotency(session_id).await?;
        let mut state = self.state.lock().await;
        if !state.loaded_idempotency.insert(session_id.to_owned()) {
            return Ok(());
        }
        for record in records {
            let frame_key = (session_id.to_owned(), record.frame_id);
            state.idempotency.insert(
                frame_key.clone(),
                CachedRequest {
                    fingerprint: record.fingerprint,
                    response: record.response.clone(),
                },
            );
            state.idempotency_order.push_back(frame_key);
            if let (Some(turn_id), Some(turn_fingerprint)) =
                (record.turn_id, record.turn_fingerprint)
            {
                let turn_key = (session_id.to_owned(), turn_id.clone());
                state
                    .turn_request_fingerprints
                    .insert(turn_key.clone(), turn_fingerprint);
                if let Some(frame) = record
                    .response
                    .iter()
                    .rev()
                    .find(|frame| frame.turn_id.as_ref() == Some(&turn_id))
                {
                    state
                        .turn_request_frames
                        .insert(turn_key.clone(), frame.clone());
                }
                state.turn_request_order.push_back(turn_key);
                if let Some(service_turn_id) = record.service_turn_id {
                    state
                        .turn_aliases
                        .insert((session_id.to_owned(), service_turn_id), turn_id);
                    state
                        .turn_alias_order
                        .push_back((session_id.to_owned(), service_turn_id));
                }
            }
        }
        while state.idempotency_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.idempotency_order.pop_front() {
                state.idempotency.remove(&oldest);
            }
        }
        while state.turn_request_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.turn_request_order.pop_front() {
                state.turn_request_fingerprints.remove(&oldest);
                state.turn_request_frames.remove(&oldest);
            }
        }
        while state.turn_alias_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.turn_alias_order.pop_front() {
                state.turn_aliases.remove(&oldest);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn user_input(
        &self,
        frame: Envelope<Value>,
        principal: Option<&str>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let turn_id = frame.turn_id.clone().ok_or_else(|| {
            HostError::InvalidFrame("USER_INPUT requires envelope.turn_id".to_owned())
        })?;
        let turn_fingerprint = crate::bundle::sha256(&serde_json::to_vec(&frame.payload)?);
        let turn_key = (frame.session_id.clone(), turn_id.clone());
        {
            let state = self.state.lock().await;
            if let Some(previous_fingerprint) = state.turn_request_fingerprints.get(&turn_key) {
                if previous_fingerprint != &turn_fingerprint {
                    return Err(HostError::IdempotencyConflict(
                        "turn_id was reused with different input".to_owned(),
                    ));
                }
                return Ok(state
                    .turn_request_frames
                    .get(&turn_key)
                    .cloned()
                    .into_iter()
                    .collect());
            }
        }
        let input: AcceptedUserInput = serde_json::from_value(frame.payload)?;
        validate_user_input(&input)?;
        let accepted_content = match (&input.prompt, &input.content) {
            (Some(text), None) => vec![AcceptedContentPart {
                kind: AcceptedPartKind::Text,
                text: Some(text.clone()),
                ..AcceptedContentPart::default()
            }],
            (None, Some(content)) => content.clone(),
            _ => {
                return Err(HostError::InvalidFrame(
                    "prompt or non-empty content is required".to_owned(),
                ));
            }
        };
        let session = SessionKey::new(frame.session_id.clone());
        let service = self.service_for_session(&frame.session_id).await?;
        let owner = if let Some(principal) = principal {
            Some(principal.to_owned())
        } else {
            self.state
                .lock()
                .await
                .session_owners
                .get(&frame.session_id)
                .cloned()
        };
        let owner = owner.as_deref();
        let session_spec = service.session_spec(&session);
        let supported = supported_input_modalities(&service, &session);
        let client_capabilities = self
            .state
            .lock()
            .await
            .client_capabilities
            .get(&frame.session_id)
            .cloned()
            .unwrap_or_default();
        let requested_output = input
            .output
            .clone()
            .unwrap_or_else(|| vec![raep_types::Modality::Text]);
        for modality in &requested_output {
            if modality == &raep_types::Modality::Text {
                continue;
            }
            if !self.uploads_enabled()
                || !client_capabilities
                    .features
                    .contains(&raep_types::ClientFeature::OutputParts)
                || !client_capabilities.render.contains(modality)
            {
                return Err(HostError::InvalidFrame(format!(
                    "UNSUPPORTED_MODALITY: client has not negotiated output.parts and {modality:?} rendering"
                )));
            }
            let supported_by_model = session_spec.as_ref().is_some_and(|spec| {
                spec.capabilities
                    .modalities
                    .supports_output(to_norte_modality(modality))
            });
            if !supported_by_model {
                return Err(HostError::InvalidFrame(format!(
                    "UNSUPPORTED_MODALITY: the effective agent cannot produce {modality:?}"
                )));
            }
        }
        let mut upload_ids = Vec::new();
        let (text, attachments) = match (input.prompt, input.content) {
            (Some(text), None) => (text, Vec::new()),
            (None, Some(parts)) => {
                let mut text = String::new();
                let mut attachments = Vec::new();
                let mut modality_counts = HashMap::<norte::Modality, u32>::new();
                for (part_index, part) in parts.into_iter().enumerate() {
                    if part.kind == AcceptedPartKind::Text {
                        text.push_str(part.text.as_deref().ok_or_else(|| {
                            HostError::InvalidFrame("text content part is missing text".to_owned())
                        })?);
                        continue;
                    }
                    let modality = match part.kind {
                        AcceptedPartKind::Image => norte::Modality::Image,
                        AcceptedPartKind::Document => norte::Modality::Document,
                        AcceptedPartKind::Audio => norte::Modality::Audio,
                        AcceptedPartKind::Video => norte::Modality::Video,
                        AcceptedPartKind::Text => continue,
                    };
                    if !supported.contains(&modality) {
                        return Err(HostError::InvalidFrame(format!(
                            "UNSUPPORTED_MODALITY: part {part_index} requires {modality:?}; supported: {supported:?}"
                        )));
                    }
                    let modality_count = modality_counts.entry(modality).or_default();
                    *modality_count = modality_count.saturating_add(1);
                    let modality_limits = session_spec
                        .as_ref()
                        .and_then(|spec| spec.capabilities.modalities.limits(modality));
                    if modality_limits
                        .and_then(|limits| limits.max_count)
                        .is_some_and(|max| *modality_count > max)
                    {
                        return Err(HostError::InvalidFrame(format!(
                            "UNSUPPORTED_MODALITY: too many {modality:?} parts"
                        )));
                    }
                    let mime = part.mime_type.clone().ok_or_else(|| {
                        HostError::InvalidFrame("media content requires mime_type".to_owned())
                    })?;
                    validate_media_mime(part.kind, &mime).map_err(HostError::InvalidFrame)?;
                    let mime_type = norte::MimeType::new(mime.clone())
                        .map_err(|error| HostError::InvalidFrame(error.to_string()))?;
                    if modality_limits.is_some_and(|limits| !limits.accepts_mime(&mime_type)) {
                        return Err(HostError::InvalidFrame(format!(
                            "UNSUPPORTED_MIME_TYPE: part {part_index} does not accept `{mime}`"
                        )));
                    }
                    let (source_kind, asset_id) = match part.source.as_ref() {
                        Some(AcceptedSource::Upload { upload_id }) => {
                            (AssetSourceKind::Upload, upload_id.as_str())
                        }
                        Some(AcceptedSource::Output { output_id }) => {
                            (AssetSourceKind::Output, output_id.as_str())
                        }
                        None => {
                            return Err(HostError::InvalidFrame("source is required".to_owned()));
                        }
                    };
                    let upload = self
                        .resolve_session_asset(
                            &frame.session_id,
                            asset_id,
                            owner,
                            source_kind == AssetSourceKind::Output,
                        )
                        .await?;
                    if modality_limits
                        .and_then(|limits| limits.max_bytes)
                        .is_some_and(|max| upload.bytes > max)
                    {
                        return Err(HostError::InvalidFrame(format!(
                            "MEDIA_TOO_LARGE: part {part_index} exceeds the {modality:?} byte limit"
                        )));
                    }
                    if upload.mime_type != mime {
                        return Err(HostError::InvalidFrame(format!(
                            "upload MIME type `{}` does not match content MIME type `{mime}`",
                            upload.mime_type
                        )));
                    }
                    let blob = upload.blob_id.parse().map_err(|_| {
                        HostError::Artifact("stored upload has an invalid blob id".to_owned())
                    })?;
                    let name = part
                        .name
                        .as_deref()
                        .or(part.alt.as_deref())
                        .unwrap_or(asset_id)
                        .to_owned();
                    if let Some(alt) = &part.alt {
                        text.push_str("\n[Media description: ");
                        text.push_str(alt);
                        text.push(']');
                    }
                    if source_kind == AssetSourceKind::Upload {
                        upload_ids.push(asset_id.to_owned());
                    }
                    attachments.push(norte_code_protocol::Attachment {
                        path: String::new(),
                        blob: Some(blob),
                        mime,
                        name,
                        duration_ms: part.duration_ms,
                        transcript: part.transcript,
                    });
                }
                (text, attachments)
            }
            _ => {
                return Err(HostError::InvalidFrame(
                    "prompt or non-empty content is required".to_owned(),
                ));
            }
        };
        let queue_mode = match input.queue_policy.clone() {
            Some(raep_types::QueuePolicy::Queue) => norte_code_protocol::QueueMode::Queue,
            Some(raep_types::QueuePolicy::Interrupt) => norte_code_protocol::QueueMode::Interrupt,
            Some(raep_types::QueuePolicy::Reject) | None => norte_code_protocol::QueueMode::Reject,
        };
        // Per-turn `overrides` (RFC 12.4): translate the known fields onto
        // the turn's own reasoning/budget. Model changes are not a per-turn
        // concept in the service, so a model override here is rejected.
        let (override_reasoning, override_model, override_budget) = match input.overrides.clone() {
            Some(ref value) => {
                let decoded = decode_override(value)?;
                validate_override(decoded.0.as_ref(), decoded.1.as_ref(), decoded.2.as_ref())?;
                decoded
            }
            None => (None, None, None),
        };
        if override_model.is_some() {
            return Err(HostError::InvalidFrame(
                "model override is not supported on USER_INPUT; use SESSION_CONFIGURE".to_owned(),
            ));
        }
        let queued_turn = QueuedTurn {
            turn_id: turn_id.clone(),
            text,
            attachments,
            budget: override_budget,
            reasoning: override_reasoning,
            metadata: input.metadata.clone(),
            output: requested_output,
        };
        let queued_position = {
            let mut state = self.state.lock().await;
            if let Some(previous_fingerprint) = state.turn_request_fingerprints.get(&turn_key) {
                if previous_fingerprint != &turn_fingerprint {
                    return Err(HostError::IdempotencyConflict(
                        "turn_id was reused with different input".to_owned(),
                    ));
                }
                return Ok(state
                    .turn_request_frames
                    .get(&turn_key)
                    .cloned()
                    .into_iter()
                    .collect());
            }
            let occupied = state.active_turns.contains_key(&frame.session_id)
                || state.turn_reservations.contains_key(&frame.session_id);
            if occupied && queue_mode == norte_code_protocol::QueueMode::Reject {
                return Err(HostError::SessionBusy);
            }
            let queued = state
                .queued_turns
                .get(&frame.session_id)
                .map_or(0, VecDeque::len);
            if occupied && queued >= self.config.max_queued_inputs {
                return Err(HostError::SessionBusy);
            }
            state
                .turn_request_fingerprints
                .insert(turn_key.clone(), turn_fingerprint);
            state.turn_request_order.push_back(turn_key.clone());
            while state.turn_request_order.len() > self.config.idempotency_capacity {
                if let Some(oldest) = state.turn_request_order.pop_front() {
                    state.turn_request_fingerprints.remove(&oldest);
                    state.turn_request_frames.remove(&oldest);
                }
            }
            if occupied {
                let queue = state
                    .queued_turns
                    .entry(frame.session_id.clone())
                    .or_default();
                if queue_mode == norte_code_protocol::QueueMode::Interrupt {
                    queue.push_front(queued_turn.clone());
                    Some(1)
                } else {
                    queue.push_back(queued_turn.clone());
                    Some(u64::try_from(queue.len()).unwrap_or(u64::MAX))
                }
            } else {
                state
                    .turn_reservations
                    .insert(frame.session_id.clone(), turn_id.clone());
                state
                    .pending_turn_ids
                    .entry(frame.session_id.clone())
                    .or_default()
                    .push_back(turn_id.clone());
                if queued_turn
                    .output
                    .iter()
                    .any(|modality| !matches!(modality, raep_types::Modality::Text))
                {
                    state
                        .pending_output
                        .insert(frame.session_id.clone(), queued_turn.output.clone());
                }
                None
            }
        };
        if !upload_ids.is_empty()
            && let Some(index) = &self.upload_index
            && let Err(error) = index
                .mark_referenced(&frame.session_id, owner, &upload_ids)
                .await
        {
            self.remove_turn_request(&frame.session_id, &turn_id).await;
            return Err(error);
        }
        let accepted = match self
            .emit_with_turn_id(
                &frame.session_id,
                StandardFrameType::UserInputAccepted,
                payload(UserInputAcceptedPayload {
                    turn_id: turn_id.clone(),
                    origin: UserInputOrigin {
                        kind: UserInputOriginKind::User,
                    },
                    content: accepted_content,
                    position: queued_position,
                    replaces_turn_id: None,
                }),
                turn_id.clone(),
            )
            .await
        {
            Ok(frame) => frame,
            Err(error) => {
                self.remove_turn_request(&frame.session_id, &turn_id).await;
                return Err(error);
            }
        };
        if queued_position.is_some() {
            let mut output = vec![accepted];
            output.push(
                self.emit_with_turn_id(
                    &frame.session_id,
                    StandardFrameType::TurnStateChanged,
                    payload(QueuedTurnStatePayload {
                        state: raep_types::TurnState::Queued,
                    }),
                    turn_id.clone(),
                )
                .await?,
            );
            if queue_mode == norte_code_protocol::QueueMode::Interrupt
                && let Err(error) = self
                    .submit_service(
                        &frame.session_id,
                        ClientRequestKind::CancelTurn {
                            session: SessionKey::new(frame.session_id.clone()),
                        },
                    )
                    .await
            {
                let mut state = self.state.lock().await;
                if let Some(queue) = state.queued_turns.get_mut(&frame.session_id) {
                    queue.retain(|queued| queued.turn_id != turn_id);
                }
                drop(state);
                self.remove_turn_request(&frame.session_id, &turn_id).await;
                return Err(error);
            }
            return Ok(output);
        }
        let submission = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::SendMessage {
                    session: Some(SessionKey::new(frame.session_id.clone())),
                    text: queued_turn.text,
                    attachments: queued_turn.attachments,
                    budget: queued_turn.budget,
                    reasoning: queued_turn.reasoning,
                    metadata: queued_turn.metadata,
                    replay: false,
                },
            )
            .await;
        let mut output = vec![accepted];
        match submission {
            Ok(events) => output.extend(events),
            Err(error) => {
                let mut state = self.state.lock().await;
                state.pending_output.remove(&frame.session_id);
                state.pending_turn_ids.remove(&frame.session_id);
                state.turn_reservations.remove(&frame.session_id);
                drop(state);
                self.remove_turn_request(&frame.session_id, &turn_id).await;
                return Err(error);
            }
        }
        Ok(output)
    }

    /// History page request (RFC 16): one page of the session's history.
    ///
    /// The service answers history reads as `MessageReplayed` events; the
    /// host gathers those into a stable, cursor-paginated history page.
    async fn history_get(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: HistoryGetRequest = serde_json::from_value(frame.payload.clone())?;
        let before_turn = request
            .before_turn_id
            .as_deref()
            .map(|id| id.strip_prefix("turn_").unwrap_or(id).parse::<u64>())
            .transpose()
            .map_err(|_| HostError::InvalidFrame("before_turn_id must name a turn".to_owned()))?;
        let limit = request.limit.unwrap_or(50);
        if !(1..=100).contains(&limit) {
            return Err(HostError::InvalidFrame(
                "HISTORY_GET limit must be between 1 and 100".to_owned(),
            ));
        }
        let active = self
            .service
            .session_history_with_provenance(&SessionKey::new(frame.session_id.clone()))
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        let mut history: Vec<_> = active
            .into_iter()
            .map(|(turn, role, kind, text, timestamp, origin)| {
                ((turn, role, kind, text, timestamp), false, origin)
            })
            .collect();
        if request.include_rewound {
            let rewound = self
                .service
                .session_rewound_history_with_turns(&SessionKey::new(frame.session_id.clone()))
                .map_err(|error| HostError::Service(error.to_string()))?;
            history.extend(
                rewound
                    .into_iter()
                    .map(|item| (item, true, Option::<Value>::None)),
            );
        }
        history.sort_by_key(|((turn, _, _, _, timestamp), rewound, _)| {
            (turn.unwrap_or_default(), *timestamp, *rewound)
        });
        if let Some(before) = before_turn
            && !history
                .iter()
                .any(|((turn, _, _, _, _), _, _)| *turn == Some(before))
        {
            return Err(HostError::InvalidFrame(format!(
                "TURN_NOT_FOUND: `{}` is not in this session's indexed history",
                request.before_turn_id.as_deref().unwrap_or_default()
            )));
        }
        let mut eligible: Vec<_> = history
            .into_iter()
            .filter(|((turn, _, _, _, _), _, _)| match (before_turn, turn) {
                (Some(before), Some(turn)) => *turn < before,
                (Some(_), None) => false,
                (None, _) => true,
            })
            .collect();
        let limit = usize::try_from(limit).unwrap_or(100);
        let has_more = eligible.len() > limit;
        if has_more {
            eligible.drain(..eligible.len() - limit);
        }
        let items: Vec<HistorySummaryItem> = eligible
            .into_iter()
            .map(|((turn, role, kind, text, timestamp), rewound, origin)| {
                history_item_value(HistoryItemView {
                    turn,
                    role,
                    kind,
                    text,
                    timestamp,
                    rewound,
                    origin,
                    max_content_bytes: None,
                })
            })
            .collect();
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::HistoryPage,
                payload(HistoryPagePayload { items, has_more }),
            )
            .await?,
        ])
    }

    async fn rewind(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: RewindRequest = serde_json::from_value(frame.payload.clone())?;
        let restore_workspace = request.workspace == Some(WorkspaceRewindPolicy::Restore);
        let rewound_turn_ids = self
            .service
            .rewind_session(
                &SessionKey::new(frame.session_id.clone()),
                &request.to_turn_id,
                request.inclusive,
                request.cancel_pending,
                restore_workspace,
            )
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::HistoryRewound,
                payload(HistoryRewoundPayload {
                    to_turn_id: request.to_turn_id,
                    inclusive: request.inclusive,
                    rewound_turn_ids,
                    side_effects: Vec::new(),
                    workspace_restored: false,
                }),
            )
            .await?,
        ])
    }

    /// Cancels one job owned by this session.
    async fn cancel_job(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        if frame
            .payload
            .get("grace_ms")
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(HostError::InvalidFrame(
                "CANCEL_JOB.grace_ms must be a non-negative integer".to_owned(),
            ));
        }
        let job_id = frame
            .job_id
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| {
                frame
                    .payload
                    .get("job_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .ok_or_else(|| HostError::InvalidFrame("CANCEL_JOB.job_id is required".to_owned()))?;
        let grace = frame
            .payload
            .get("grace_ms")
            .and_then(Value::as_u64)
            .map_or_else(
                || std::time::Duration::from_secs(5),
                std::time::Duration::from_millis,
            );
        let session = SessionKey::new(frame.session_id.clone());
        let service = self.service_for_session(&frame.session_id).await?;
        let job = service
            .stop_session_job_with_grace(&session, &job_id, Some(grace))
            .await
            .ok_or_else(|| HostError::InvalidFrame("JOB_NOT_FOUND".to_owned()))?;
        let triggered_new_turn = service
            .session_job_triggered_new_turn(&session, &job_id)
            .unwrap_or(false);
        let new_turn_id = if triggered_new_turn {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                service.session_job_new_turn_id(&session, &job_id),
            )
            .await
            .ok()
            .flatten()
            .map(|turn| turn.to_string())
        } else {
            None
        };
        let active_turn_id = service
            .session_active_turn_id(&session)
            .map(|turn| turn.to_string());
        let injected_into_turn_id = (!triggered_new_turn)
            .then_some(active_turn_id.as_deref())
            .flatten();
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::JobFinished,
                job_finished_payload(
                    &job,
                    triggered_new_turn,
                    new_turn_id.as_deref(),
                    injected_into_turn_id,
                ),
            )
            .await?,
        ])
    }

    /// Job listing (RFC 14.3): `JOB_LIST` returns `JOB_LIST_RESULT`.
    async fn job_list(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: JobListRequest =
            serde_json::from_value(frame.payload.clone()).map_err(|error| {
                HostError::InvalidFrame(format!("JOB_LIST payload is invalid: {error}"))
            })?;
        let limit = usize::try_from(request.limit.unwrap_or(100))
            .map_err(|_| HostError::InvalidFrame("JOB_LIST.limit is out of range".to_owned()))?;
        if !(1..=100).contains(&limit) {
            return Err(HostError::InvalidFrame(
                "JOB_LIST.limit must be between 1 and 100".to_owned(),
            ));
        }
        let mut frames = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::ListJobs {
                    session: SessionKey::new(frame.session_id.clone()),
                },
            )
            .await?;
        if let Some(result) = frames
            .iter_mut()
            .find(|result| result.kind == standard(StandardFrameType::JobListResult))
        {
            let mut jobs: Vec<JobListEntry> =
                serde_json::from_value(result.payload["jobs"].clone()).map_err(|error| {
                    HostError::InvalidFrame(format!("JOB_LIST_RESULT payload is invalid: {error}"))
                })?;
            if let Some(statuses) = request.status {
                jobs.retain(|job| statuses.contains(&job.status));
            }
            jobs.truncate(limit);
            result.payload = payload(JobListResultPayload { jobs });
        }
        Ok(frames)
    }

    /// Job state query (RFC 14.3): `JOB_GET` returns `JOB_STATE`.
    ///
    /// The retained output lives in the session's `JobList` summaries (the
    /// registry's `JobInfo` carries the most recent output) and in the
    /// replay window (`ToolOutput`/`ToolFinished` frames). The host
    /// answers from those: `from_index` skips that many leading frames;
    /// running jobs return their current output, finished jobs their
    /// latest output, and unknown jobs `JOB_NOT_FOUND`.
    async fn job_get(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let job_id = frame
            .job_id
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| {
                frame
                    .payload
                    .get("job_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .ok_or_else(|| HostError::InvalidFrame("JOB_NOT_FOUND".to_owned()))?;
        if frame
            .payload
            .get("from_index")
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(HostError::InvalidFrame(
                "JOB_GET.from_index must be a non-negative integer".to_owned(),
            ));
        }
        let from_index = frame
            .payload
            .get("from_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let service = self.service_for_session(&frame.session_id).await?;
        let job = service
            .session_job_with_output_retention(&SessionKey::new(frame.session_id.clone()), &job_id)
            .ok_or_else(|| HostError::InvalidFrame("JOB_NOT_FOUND".to_owned()))?;
        let output = job_output_from_index(&job, from_index)?;
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::JobState,
                payload(JobStatePayload {
                    job_id: &job_id,
                    status: job_status(&job),
                    output,
                    next_index: job.output_bytes_total,
                }),
            )
            .await?,
        ])
    }

    /// Session fork (RFC 18.1): copy a session's history into a new root
    /// session. Maps onto `ForkSession`; the new session id comes back on
    /// the `SESSION_FORKED` event.
    async fn fork(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: ForkRequest = serde_json::from_value(frame.payload.clone())?;
        self.submit_service(
            &frame.session_id,
            ClientRequestKind::ForkSession {
                session: SessionKey::new(frame.session_id.clone()),
                title: request.title,
                from_turn_id: request.from_turn_id,
                inclusive: request.inclusive,
                carry_usage: request.carry_usage,
            },
        )
        .await
    }

    /// Session compaction (RFC 18.3): trigger context compaction now.
    /// Maps onto `Compact`; the `HISTORY_COMPACTED` notification follows.
    async fn compact(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        self.submit_service(
            &frame.session_id,
            ClientRequestKind::Compact {
                session: SessionKey::new(frame.session_id.clone()),
            },
        )
        .await
    }

    /// Context pins (RFC 18.5): pin or unpin turns so they stay verbatim
    /// through compaction. The pins are held in the host's in-memory
    /// state (up to `max_pinned_turns` = 20) and persisted through the
    /// [`PinStore`] when one is configured; without a store they do not
    /// survive a restart. The CLI does not pin; this is so another
    /// client (desktop) can.
    async fn context_pin(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let pinned = frame
            .payload
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let turn_ids: Vec<String> = frame
            .payload
            .get("turn_ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if turn_ids.is_empty() {
            return Err(HostError::InvalidFrame(
                "CONTEXT_PIN requires turn_ids".to_owned(),
            ));
        }
        let mut state = self.state.lock().await;
        let pins = state
            .pinned_turns
            .entry(frame.session_id.clone())
            .or_default();
        for id in &turn_ids {
            if pinned {
                if pins.contains(id) {
                    continue;
                }
                if pins.len() >= 20 {
                    return Err(HostError::InvalidFrame(
                        "PAYLOAD_TOO_LARGE: too many pinned turns".to_owned(),
                    ));
                }
                pins.push(id.clone());
            } else {
                pins.retain(|p| p != id);
            }
        }
        let pinned_turn_ids = pins.clone();
        drop(state);
        if let Some(store) = self.pin_store.clone() {
            store.save(&frame.session_id, &pinned_turn_ids).await?;
        }
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::ContextUpdated,
                payload(ContextUpdatedPayload { pinned_turn_ids }),
            )
            .await?,
        ])
    }

    async fn export(
        &self,
        frame: Envelope<Value>,
        principal: Option<&str>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: ExportRequest = serde_json::from_value(frame.payload)?;
        validate_export_request(&request)?;
        let service = self.service_for_session(&frame.session_id).await?;
        let bytes = self.build_text_bundle(&service, &frame.session_id).await?;
        let (export_id, expires_at, byte_count, digest) =
            self.store_export(bytes, principal).await?;
        let ready = self
            .emit(
                &frame.session_id,
                StandardFrameType::SessionExportReady,
                payload(ExportReadyPayload {
                    export_id: &export_id,
                    download_url: format!("/raep/v1/exports/{export_id}"),
                    bytes: byte_count,
                    sha256: &digest,
                    expires_at,
                    sessions: 1,
                    encrypted: false,
                    signed: false,
                }),
            )
            .await?;
        Ok(vec![ready])
    }

    async fn build_text_bundle(
        &self,
        service: &Service,
        session_id: &str,
    ) -> Result<Vec<u8>, HostError> {
        let key = SessionKey::new(session_id.to_owned());
        let session = service
            .sessions()
            .into_iter()
            .find(|session| session.session.as_str() == session_id)
            .ok_or_else(|| HostError::InvalidFrame("session not found".to_owned()))?;
        if session.phase != norte_code_protocol::Phase::Idle || service.has_running_jobs(&key) {
            return Err(HostError::InvalidFrame(
                "SESSION_BUSY: export requires an idle session with no running jobs".to_owned(),
            ));
        }
        if !service
            .session_rewound_history_with_turns(&key)
            .map_err(|error| HostError::Service(error.to_string()))?
            .is_empty()
        {
            return Err(HostError::InvalidFrame(
                "SESSION_EXPORT cannot preserve rewound history on this host".to_owned(),
            ));
        }
        if !service
            .session_history_is_plain_text(&key)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?
        {
            return Err(HostError::InvalidFrame(
                "SESSION_EXPORT supports only plain-text message parts".to_owned(),
            ));
        }
        let session_agent_id = self
            .state
            .lock()
            .await
            .session_agents
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| self.config.agent_id.clone());
        let history = service
            .session_history_with_provenance(&key)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        let history_bytes = serialize_plain_text_history(history, self.config.max_export_bytes)?;
        let session_path = format!("sessions/{session_id}/session.json");
        let history_path = format!("sessions/{session_id}/history.jsonl");
        let agent = payload(BundleAgent {
            id: &session_agent_id,
        });
        let session_json = serde_json::to_vec(&PortableSessionFile {
            agent: BundleAgent {
                id: &session_agent_id,
            },
            title: &session.title,
            options: BTreeMap::new(),
        })?;
        let files = std::collections::BTreeMap::from([
            (session_path, session_json),
            (history_path.clone(), history_bytes),
        ]);
        let manifest = crate::bundle::BundleManifest {
            bundle_version: "1".to_owned(),
            raep_document_version: None,
            wire_version: Some(WIRE_VERSION.to_owned()),
            exported_at: u64::try_from(now_millis()).unwrap_or_default(),
            exporter: Some(payload(BundleExporter {
                server: &self.config.server_version,
            })),
            root_session_id: session_id.to_owned(),
            sessions: vec![crate::bundle::BundleSession {
                session_id: session_id.to_owned(),
                parent_session_id: None,
                agent,
                state: Some("IDLE".to_owned()),
                history_file: history_path,
                last_seq: Some(self.current_seq(session_id).await),
                usage_total: service.session_usage_total(&key),
            }],
            files: Vec::new(),
            excluded: vec![
                "children".to_owned(),
                "uploads".to_owned(),
                "job_output".to_owned(),
                "outputs".to_owned(),
                "workspace".to_owned(),
                "system_prompt".to_owned(),
                "credentials".to_owned(),
            ],
        };
        let bytes = crate::bundle::create_bundle(manifest, files)
            .map_err(|error| HostError::InvalidFrame(error.to_string()))?;
        if bytes.len() > self.config.max_export_bytes {
            return Err(HostError::InvalidFrame(
                "PAYLOAD_TOO_LARGE: export exceeds max_export_bytes".to_owned(),
            ));
        }
        Ok(bytes)
    }

    async fn store_export(
        &self,
        bytes: Vec<u8>,
        principal: Option<&str>,
    ) -> Result<(String, i64, usize, String), HostError> {
        let byte_count = bytes.len();
        let digest = crate::bundle::sha256(&bytes);
        let export_id = format!("exp_{}", uuid::Uuid::new_v4().simple());
        let expires_at = now_millis().saturating_add(
            i64::try_from(self.config.export_ttl_s.saturating_mul(1000)).unwrap_or(i64::MAX),
        );
        {
            let mut state = self.state.lock().await;
            state
                .exports
                .retain(|_, export| export.expires_at > now_millis());
            if state.exports.len() >= 4 {
                return Err(HostError::InvalidFrame(
                    "EXPORT_STORE_FULL: active export limit reached".to_owned(),
                ));
            }
            state.exports.insert(
                export_id.clone(),
                StoredExport {
                    bytes,
                    owner: principal.map(str::to_owned),
                    expires_at,
                },
            );
        }
        Ok((export_id, expires_at, byte_count, digest))
    }

    /// Rejects a standard operation that this host has not implemented.
    fn not_supported(
        _frame: &Envelope<Value>,
        what: &str,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        Err(HostError::InvalidFrame(format!(
            "{what} is not supported by this host"
        )))
    }

    fn approval_response(frame: &Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "TOOL_APPROVAL_RESPONSE")
    }

    fn input_stream(
        frame: &Envelope<Value>,
        what: &str,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, &format!("INPUT_STREAM_{what}"))
    }

    fn client_tool_result(frame: &Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "CLIENT_TOOL_RESULT")
    }

    fn presence_update(frame: &Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "PRESENCE_UPDATE")
    }

    /// Delegator control (RFC 8.2.11): `suspend`/`release` of the delegator.
    async fn delegator_control(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let action = frame
            .payload
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let child_session_id = frame
            .payload
            .get("child_session_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let child = SessionKey::new(child_session_id.clone());
        let (suspended, queued) = match action.as_str() {
            "suspend" => self
                .service
                .set_delegator_suspended(&child, true)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
            "release" => self
                .service
                .set_delegator_suspended(&child, false)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "DELEGATOR_CONTROL: unknown action `{other}`; use suspend or release"
                )));
            }
        };
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::DelegatorStateChanged,
                payload(DelegatorStateChangedPayload {
                    child_session_id: &child_session_id,
                    suspended,
                    by: DelegatorActor::Client,
                    queued_messages: queued,
                }),
            )
            .await?,
        ])
    }

    fn handoff(frame: &Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "HANDOFF")
    }

    fn auth_refresh(frame: &Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        Self::not_supported(frame, "AUTH_REFRESH")
    }

    /// Bundle import is implemented over HTTP, not as a wire-frame request.
    fn import() -> Result<Vec<Envelope<Value>>, HostError> {
        Err(HostError::InvalidFrame(
            "SESSION_IMPORT is an HTTP workflow; use POST /raep/v1/imports".to_owned(),
        ))
    }

    async fn cancel(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let requested_turn = frame.turn_id.as_ref().ok_or_else(|| {
            HostError::InvalidFrame("CANCEL_TURN requires envelope.turn_id".to_owned())
        })?;
        let (active_turn, cancelled_queued_turn) = {
            let mut state = self.state.lock().await;
            let active_turn = state.active_turns.get(&frame.session_id).cloned();
            if active_turn.as_ref() == Some(requested_turn) {
                (active_turn, false)
            } else {
                let Some(queue) = state.queued_turns.get_mut(&frame.session_id) else {
                    return Err(HostError::TurnNotActive);
                };
                let Some(position) = queue
                    .iter()
                    .position(|queued| &queued.turn_id == requested_turn)
                else {
                    return Err(HostError::TurnNotActive);
                };
                queue.remove(position);
                if queue.is_empty() {
                    state.queued_turns.remove(&frame.session_id);
                }
                if let Some(pending) = state.pending_turn_ids.get_mut(&frame.session_id) {
                    pending.retain(|turn_id| turn_id != requested_turn);
                }
                (active_turn, true)
            }
        };
        if cancelled_queued_turn {
            return self
                .emit_with_turn_id(
                    &frame.session_id,
                    StandardFrameType::TurnStateChanged,
                    payload(raep_types::TurnStateChanged {
                        state: raep_types::TurnState::Cancelled,
                        stop_reason: Some(raep_types::StopReason::Cancelled),
                        usage: None,
                        error: None,
                    }),
                    requested_turn.clone(),
                )
                .await
                .map(|frame| vec![frame]);
        }
        let active_turn = active_turn.ok_or(HostError::TurnNotActive)?;
        if &active_turn != requested_turn {
            return Err(HostError::TurnNotActive);
        }
        let request = ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::CancelTurn {
                session: SessionKey::new(frame.session_id.clone()),
            },
        };
        let events = self
            .service
            .submit(request)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.collect_service_events(&frame.session_id, events).await
    }

    async fn detach(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        let mut state = self.state.lock().await;
        state.attached.remove(&frame.session_id);
        state.attachment_modes.remove(&frame.session_id);
        Ok(Vec::new())
    }

    async fn close(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let purge = frame
            .payload
            .get("purge")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let events = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::DeleteSession {
                    session: SessionKey::new(frame.session_id.clone()),
                },
            )
            .await?;
        let mut output = events;
        output.push(
            self.emit(
                &frame.session_id,
                StandardFrameType::SessionClosed,
                payload(SessionClosedPayload {
                    reason: if purge {
                        SessionClosedReason::Purged
                    } else {
                        SessionClosedReason::Client
                    },
                }),
            )
            .await?,
        );
        {
            let mut state = self.state.lock().await;
            state.attached.remove(&frame.session_id);
            state.attachment_modes.remove(&frame.session_id);
        }
        Ok(output)
    }

    async fn ack(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let last_seq = frame
            .payload
            .get("last_seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| HostError::InvalidFrame("ACK.last_seq is required".to_owned()))?;
        let mut state = self.state.lock().await;
        if last_seq
            > state
                .sequences
                .get(&frame.session_id)
                .copied()
                .unwrap_or_default()
        {
            return Err(HostError::InvalidFrame(
                "ACK.last_seq exceeds the last emitted sequence".to_owned(),
            ));
        }
        if let Some(frames) = state.replay.get_mut(&frame.session_id) {
            frames.retain(|stored| stored.seq.is_none_or(|seq| seq > last_seq));
        }
        Ok(Vec::new())
    }

    async fn extension_command(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let request: ClientRequest = serde_json::from_value(frame.payload)?;
        if is_plugin_request(&request.kind) {
            return self.plugin_command(&frame.session_id, request).await;
        }
        // Connection-scoped commands deliberately ignore the route's
        // placeholder session id. The HTTP adapter has a session-shaped URL
        // for every command, but listing sessions, creating one, and querying
        // host-wide catalogues must work before any session is attached.
        let session_id = session_for_request(&request.kind)
            .map(std::string::ToString::to_string)
            .unwrap_or_default();
        if !session_id.is_empty() {
            self.require_attached(&session_id).await?;
        }
        let service = if session_id.is_empty() {
            Arc::clone(&self.service)
        } else {
            self.service_for_session(&session_id).await?
        };
        self.collect_service_events(
            &session_id,
            service
                .submit(request)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
        )
        .await
    }

    async fn plugin_command(
        &self,
        session_id: &str,
        request: ClientRequest,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let event = match request.kind {
            ClientRequestKind::ListPlugins => ServerEventKind::PluginList {
                plugins: crate::plugin_manager::plugin_info(),
            },
            ClientRequestKind::InstallPlugin { url, package, all } => {
                let result = tokio::task::spawn_blocking(move || {
                    if all {
                        crate::plugin_manager::install_all_plugins(&url, &|_, _, _| {})
                            .map(|names| names.join(", "))
                    } else {
                        crate::plugin_manager::install_plugin(
                            &url,
                            package.as_deref(),
                            &|_, _, _| {},
                        )
                    }
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: "installed".to_owned(),
                    name: result,
                }
            }
            ClientRequestKind::UpdatePlugins { name } => {
                let result = tokio::task::spawn_blocking(move || {
                    crate::plugin_manager::update_plugins(name.as_deref())
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                let name = result
                    .into_iter()
                    .filter_map(|(_, result)| result.ok())
                    .collect::<Vec<_>>()
                    .join(", ");
                ServerEventKind::PluginChanged {
                    operation: "updated".to_owned(),
                    name,
                }
            }
            ClientRequestKind::RemovePlugin { name } => {
                tokio::task::spawn_blocking({
                    let name = name.clone();
                    move || crate::plugin_manager::remove_plugin(&name)
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: "removed".to_owned(),
                    name,
                }
            }
            ClientRequestKind::SetPluginEnabled { name, enabled } => {
                tokio::task::spawn_blocking({
                    let name = name.clone();
                    move || crate::plugin_manager::set_plugin_enabled(&name, enabled)
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: if enabled { "enabled" } else { "disabled" }.to_owned(),
                    name,
                }
            }
            _ => unreachable!("plugin request was checked before dispatch"),
        };
        self.emit_event(
            session_id,
            ServerEvent {
                request_id: request.request_id,
                session: None,
                kind: event,
            },
        )
        .await
        .map(|frame| vec![frame])
    }

    async fn submit_service(
        &self,
        session_id: &str,
        kind: ClientRequestKind,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind,
            })
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.collect_service_events(session_id, stream).await
    }

    #[allow(clippy::too_many_lines)]
    async fn collect_service_events(
        &self,
        session_id: &str,
        mut stream: norte_code_agent::EventStream,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        use futures_util::StreamExt;
        let mut output = Vec::new();
        while let Some(event) = stream.next().await {
            let event_session = event
                .session
                .as_ref()
                .map_or_else(|| session_id.to_owned(), |key| key.as_str().to_owned());
            if let ServerEventKind::TurnStarted { turn } = &event.kind {
                let mut state = self.state.lock().await;
                let client_turn_id = state
                    .pending_turn_ids
                    .get_mut(&event_session)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or_else(|| {
                        TurnId::new(turn.to_string()).expect("numeric turn id is valid")
                    });
                state
                    .turn_aliases
                    .insert((event_session.clone(), *turn), client_turn_id.clone());
                state
                    .turn_alias_order
                    .push_back((event_session.clone(), *turn));
                while state.turn_alias_order.len() > self.config.idempotency_capacity {
                    if let Some(oldest) = state.turn_alias_order.pop_front() {
                        state.turn_aliases.remove(&oldest);
                    }
                }
                state
                    .active_turns
                    .insert(event_session.clone(), client_turn_id);
                state.turn_reservations.remove(&event_session);
                drop(state);
                let requested = self
                    .state
                    .lock()
                    .await
                    .pending_output
                    .remove(&event_session);
                if let Some(requested) = requested {
                    self.state
                        .lock()
                        .await
                        .turn_output
                        .insert((event_session.clone(), *turn), requested);
                }
            }
            if let ServerEventKind::Delta {
                delta: Delta::Media { .. },
                ..
            } = &event.kind
            {
                continue;
            }
            if let ServerEventKind::MediaPartCompleted { turn, index, media } = &event.kind {
                let requested = self
                    .state
                    .lock()
                    .await
                    .turn_output
                    .get(&(event_session.clone(), *turn))
                    .cloned()
                    .unwrap_or_default();
                if raep_modality_for_media(media.kind)
                    .is_some_and(|modality| requested.contains(&modality))
                {
                    output.extend(
                        self.emit_media_output(&event_session, *turn, *index, media)
                            .await?,
                    );
                }
                continue;
            }
            let tool_state_change = match &event.kind {
                ServerEventKind::ToolStarted { turn, .. } => {
                    Some((*turn, raep_types::TurnState::ExecutingTool))
                }
                _ => None,
            };
            let finished_tool_turn = match &event.kind {
                ServerEventKind::ToolFinished { turn, .. } => Some(*turn),
                _ => None,
            };
            if let Some((turn, state)) = tool_state_change {
                let turn_id = self
                    .state
                    .lock()
                    .await
                    .turn_aliases
                    .get(&(event_session.clone(), turn))
                    .cloned();
                if let Some(turn_id) = turn_id {
                    output.push(
                        self.emit_with_turn_id(
                            &event_session,
                            StandardFrameType::TurnStateChanged,
                            payload(raep_types::TurnStateChanged {
                                state,
                                stop_reason: None,
                                usage: None,
                                error: None,
                            }),
                            turn_id,
                        )
                        .await?,
                    );
                }
            }
            let (finished_turn, stopped_turn_id) = match &event.kind {
                ServerEventKind::TurnFinished { turn, .. } => (Some(*turn), None),
                ServerEventKind::TurnStopped { .. } => {
                    let state = self.state.lock().await;
                    let client_turn_id = state.active_turns.get(&event_session).cloned();
                    let service_turn_id = client_turn_id.as_ref().and_then(|client_turn_id| {
                        state
                            .turn_aliases
                            .iter()
                            .find(|((session, _), alias)| {
                                session == &event_session && *alias == client_turn_id
                            })
                            .map(|((_, turn), _)| *turn)
                    });
                    (service_turn_id, client_turn_id)
                }
                _ => (None, None),
            };
            output.push(
                self.emit_event_with_turn_id(session_id, event, stopped_turn_id.clone())
                    .await?,
            );
            if let Some(turn) = finished_tool_turn {
                let turn_id = self
                    .state
                    .lock()
                    .await
                    .turn_aliases
                    .get(&(event_session.clone(), turn))
                    .cloned();
                if let Some(turn_id) = turn_id {
                    output.push(
                        self.emit_with_turn_id(
                            &event_session,
                            StandardFrameType::TurnStateChanged,
                            payload(raep_types::TurnStateChanged {
                                state: raep_types::TurnState::CallingProvider,
                                stop_reason: None,
                                usage: None,
                                error: None,
                            }),
                            turn_id,
                        )
                        .await?,
                    );
                }
            }
            if finished_turn.is_some() || stopped_turn_id.is_some() {
                {
                    let mut state = self.state.lock().await;
                    if let Some(turn) = finished_turn {
                        state.turn_output.remove(&(event_session.clone(), turn));
                    }
                    state.active_turns.remove(&event_session);
                    state.turn_reservations.remove(&event_session);
                }
                if let Some(next) = self.reserve_next_queued_turn(&event_session).await {
                    output.extend(self.start_queued_turn(&event_session, next).await?);
                }
            }
        }
        self.state.lock().await.pending_output.remove(session_id);
        Ok(output)
    }

    async fn reserve_next_queued_turn(&self, session_id: &str) -> Option<QueuedTurn> {
        let mut state = self.state.lock().await;
        let next = state
            .queued_turns
            .get_mut(session_id)
            .and_then(VecDeque::pop_front);
        if state
            .queued_turns
            .get(session_id)
            .is_some_and(VecDeque::is_empty)
        {
            state.queued_turns.remove(session_id);
        }
        let next = next?;
        state
            .turn_reservations
            .insert(session_id.to_owned(), next.turn_id.clone());
        state
            .pending_turn_ids
            .entry(session_id.to_owned())
            .or_default()
            .push_back(next.turn_id.clone());
        if next
            .output
            .iter()
            .any(|modality| !matches!(modality, raep_types::Modality::Text))
        {
            state
                .pending_output
                .insert(session_id.to_owned(), next.output.clone());
        }
        Some(next)
    }

    async fn start_queued_turn(
        &self,
        session_id: &str,
        turn: QueuedTurn,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::SendMessage {
                    session: Some(SessionKey::new(session_id)),
                    text: turn.text,
                    attachments: turn.attachments,
                    budget: turn.budget,
                    reasoning: turn.reasoning,
                    metadata: turn.metadata,
                    replay: false,
                },
            })
            .await;
        match stream {
            Ok(stream) => Box::pin(self.collect_service_events(session_id, stream)).await,
            Err(error) => {
                let message = error.to_string();
                {
                    let mut state = self.state.lock().await;
                    state.turn_reservations.remove(session_id);
                    state.pending_turn_ids.remove(session_id);
                    state.pending_output.remove(session_id);
                }
                let terminal = self
                    .emit_with_turn_id(
                        session_id,
                        StandardFrameType::TurnStateChanged,
                        payload(raep_types::TurnStateChanged {
                            state: raep_types::TurnState::Failed,
                            stop_reason: Some(raep_types::StopReason::Error),
                            usage: None,
                            error: Some(raep_types::ErrorPayload {
                                code: "UNAVAILABLE".to_owned(),
                                message,
                                retryable: Some(true),
                                details: None,
                            }),
                        }),
                        turn.turn_id,
                    )
                    .await?;
                let mut output = vec![terminal];
                if let Some(next) = self.reserve_next_queued_turn(session_id).await {
                    output.extend(Box::pin(self.start_queued_turn(session_id, next)).await?);
                }
                Ok(output)
            }
        }
    }

    async fn load_generated_media(
        &self,
        media: &norte::MediaPart,
    ) -> Result<(OutputPartKind, String, Vec<u8>), HostError> {
        let (kind, input_kind) = match media.kind {
            norte::MediaKind::Image => (OutputPartKind::Image, AcceptedPartKind::Image),
            norte::MediaKind::Document => (OutputPartKind::Document, AcceptedPartKind::Document),
            norte::MediaKind::Audio => (OutputPartKind::Audio, AcceptedPartKind::Audio),
            norte::MediaKind::Video => (OutputPartKind::Video, AcceptedPartKind::Video),
            _ => {
                return Err(HostError::Artifact(
                    "the generated media kind is not supported by RAEP".to_owned(),
                ));
            }
        };
        let mime_type = media.mime.as_str();
        validate_media_mime(input_kind, mime_type).map_err(HostError::Artifact)?;
        let artifact_store = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("output storage is disabled".to_owned()))?;
        let bytes = match &media.source {
            norte::MediaSource::Inline(bytes) => bytes.clone(),
            norte::MediaSource::Blob(id) => artifact_store.get(&id.to_string()).await?,
            norte::MediaSource::ProviderFile(reference) => {
                let blob = reference.blob.as_ref().ok_or_else(|| {
                    HostError::Artifact(
                        "generated provider file has no durable blob reference".to_owned(),
                    )
                })?;
                artifact_store.get(&blob.to_string()).await?
            }
            norte::MediaSource::Url(_) => {
                return Err(HostError::Artifact(
                    "generated media URL was not materialized into the blob store".to_owned(),
                ));
            }
            _ => {
                return Err(HostError::Artifact(
                    "generated media source is not supported".to_owned(),
                ));
            }
        };
        let output_limit = self.output_part_bytes_limit();
        if bytes.is_empty() || bytes.len() > output_limit {
            return Err(HostError::Artifact(format!(
                "generated media must be between 1 and {output_limit} bytes"
            )));
        }
        Ok((kind, mime_type.to_owned(), bytes))
    }

    async fn emit_media_output(
        &self,
        session_id: &str,
        turn: u64,
        index: u32,
        media: &norte::MediaPart,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let (kind, mime_type, bytes) = self.load_generated_media(media).await?;
        let artifact_store = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| HostError::Artifact("output storage is disabled".to_owned()))?;
        let upload_index = self
            .upload_index
            .as_ref()
            .ok_or_else(|| HostError::Artifact("output metadata storage is disabled".to_owned()))?;
        let blob = artifact_store.put(&mime_type, &bytes).await?;
        let output_id = format!("out_{}", uuid::Uuid::new_v4());
        let sha256 = crate::bundle::sha256(&bytes);
        let owner = self
            .state
            .lock()
            .await
            .session_owners
            .get(session_id)
            .cloned();
        upload_index
            .insert(UploadRecord {
                id: output_id.clone(),
                session_id: session_id.to_owned(),
                owner,
                blob_id: blob.id,
                mime_type: mime_type.clone(),
                bytes: bytes.len() as u64,
                sha256: sha256.clone(),
                expires_at: i64::MAX,
                referenced: true,
            })
            .await?;
        let name = media.metadata.extension.get("name").and_then(Value::as_str);
        let alt = media.metadata.extension.get("alt").and_then(Value::as_str);
        let started = self
            .emit_with_extension(
                session_id,
                standard(StandardFrameType::OutputPartStarted),
                payload(OutputPartStartedPayload {
                    part_id: &output_id,
                    index,
                    kind,
                    mime_type: &mime_type,
                    streamed: false,
                    name,
                    alt,
                    duration_ms: media.metadata.duration_ms,
                    width: media.metadata.width,
                    height: media.metadata.height,
                    bytes_expected: bytes.len(),
                }),
                None,
                Some(turn),
                None,
                None,
            )
            .await?;
        let finished = self
            .emit_with_extension(
                session_id,
                standard(StandardFrameType::OutputPartFinished),
                payload(OutputPartFinishedPayload {
                    part_id: &output_id,
                    index,
                    status: OutputPartStatus::Ok,
                    source: OutputPartSource::Output {
                        output_id: &output_id,
                    },
                    bytes: bytes.len(),
                    sha256: &sha256,
                    transcript: media.metadata.transcript.as_deref(),
                }),
                None,
                Some(turn),
                None,
                None,
            )
            .await?;
        Ok(vec![started, finished])
    }

    async fn emit_event(
        &self,
        session_id: &str,
        event: ServerEvent,
    ) -> Result<Envelope<Value>, HostError> {
        self.emit_event_with_turn_id(session_id, event, None).await
    }

    async fn emit_event_with_turn_id(
        &self,
        session_id: &str,
        event: ServerEvent,
        turn_id_override: Option<TurnId>,
    ) -> Result<Envelope<Value>, HostError> {
        let kind = frame_type_for_event(&event.kind);
        let session_id = event
            .session
            .as_ref()
            .map_or_else(|| session_id.to_owned(), |key| key.as_str().to_owned());
        let job_id = job_id_for_event(&event.kind);
        let event_payload = standard_event_payload(&event.kind)
            .unwrap_or_else(|| canonical_event_payload(&event.kind));
        self.emit_with_extension(
            &session_id,
            kind,
            event_payload,
            Some(payload(NorteCodeEventExtension { norte_code: &event })),
            turn_id_for_event(&event.kind),
            job_id.as_deref(),
            turn_id_override,
        )
        .await
    }

    async fn emit(
        &self,
        session_id: &str,
        kind: StandardFrameType,
        payload: Value,
    ) -> Result<Envelope<Value>, HostError> {
        self.emit_with_extension(session_id, standard(kind), payload, None, None, None, None)
            .await
    }

    async fn emit_with_turn_id(
        &self,
        session_id: &str,
        kind: StandardFrameType,
        payload: Value,
        turn_id: TurnId,
    ) -> Result<Envelope<Value>, HostError> {
        let mut frame = Envelope::new(standard(kind), payload, now_millis());
        frame.session_id = session_id.to_owned();
        frame.turn_id = Some(turn_id);
        self.emit_frame(session_id, frame).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn emit_with_extension(
        &self,
        session_id: &str,
        kind: FrameType,
        payload: Value,
        ext: Option<Value>,
        turn_id: Option<u64>,
        job_id: Option<&str>,
        turn_id_override: Option<TurnId>,
    ) -> Result<Envelope<Value>, HostError> {
        let mut frame = Envelope::new(kind, payload, now_millis());
        frame.session_id = session_id.to_owned();
        frame.ext = ext;
        if let Some(job) = job_id {
            frame.job_id = Some(
                raep_types::JobId::new(job.to_owned())
                    .map_err(|error| HostError::InvalidFrame(error.to_string()))?,
            );
        }
        if let Some(turn) = turn_id {
            let numeric_turn_id = TurnId::new(turn.to_string())
                .map_err(|error| HostError::InvalidFrame(error.to_string()))?;
            let state = self.state.lock().await;
            frame.turn_id = state
                .turn_aliases
                .get(&(session_id.to_owned(), turn))
                .cloned()
                .or(Some(numeric_turn_id));
        }
        if let Some(turn_id) = turn_id_override {
            frame.turn_id = Some(turn_id);
        }
        self.emit_frame(session_id, frame).await
    }

    async fn emit_frame(
        &self,
        session_id: &str,
        mut frame: Envelope<Value>,
    ) -> Result<Envelope<Value>, HostError> {
        let mut state = self.state.lock().await;
        let sequence = state
            .sequences
            .get(session_id)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        frame.seq = Some(sequence);
        if let Some(store) = &self.replay_store {
            store.append(&frame).await?;
        }
        state.sequences.insert(session_id.to_owned(), sequence);
        if let Some(turn_id) = &frame.turn_id {
            let key = (session_id.to_owned(), turn_id.clone());
            if state.turn_request_fingerprints.contains_key(&key) {
                state.turn_request_frames.insert(key, frame.clone());
            }
        }
        let replay = state.replay.entry(session_id.to_owned()).or_default();
        replay.push_back(frame.clone());
        while replay.len() > self.config.replay_capacity {
            replay.pop_front();
        }
        drop(state);
        let _ = self.events.send(frame.clone());
        Ok(frame)
    }

    async fn require_attached(&self, session_id: &str) -> Result<(), HostError> {
        if self.state.lock().await.attached.contains(session_id) {
            Ok(())
        } else {
            Err(HostError::InvalidFrame(
                "session is not attached".to_owned(),
            ))
        }
    }

    async fn remove_turn_request(&self, session_id: &str, turn_id: &TurnId) {
        let mut state = self.state.lock().await;
        let key = (session_id.to_owned(), turn_id.clone());
        state.turn_request_fingerprints.remove(&key);
        state.turn_request_frames.remove(&key);
        state
            .turn_request_order
            .retain(|candidate| candidate != &key);
        if state.turn_reservations.get(session_id) == Some(turn_id) {
            state.turn_reservations.remove(session_id);
            state.pending_output.remove(session_id);
        }
        if let Some(pending) = state.pending_turn_ids.get_mut(session_id) {
            pending.retain(|pending_turn| pending_turn != turn_id);
            if pending.is_empty() {
                state.pending_turn_ids.remove(session_id);
            }
        }
        if let Some(queue) = state.queued_turns.get_mut(session_id) {
            queue.retain(|queued| &queued.turn_id != turn_id);
            if queue.is_empty() {
                state.queued_turns.remove(session_id);
            }
        }
    }
}

fn turn_id_for_event(kind: &ServerEventKind) -> Option<u64> {
    match kind {
        ServerEventKind::TurnStarted { turn }
        | ServerEventKind::Delta { turn, .. }
        | ServerEventKind::MediaPartCompleted { turn, .. }
        | ServerEventKind::ToolStarted { turn, .. }
        | ServerEventKind::ToolOutput { turn, .. }
        | ServerEventKind::ToolFinished { turn, .. }
        | ServerEventKind::RequestReconstructed { turn, .. }
        | ServerEventKind::TurnFinished { turn, .. }
        | ServerEventKind::QuestionAsked { turn, .. }
        | ServerEventKind::RetryScheduled { turn, .. }
        | ServerEventKind::RetryStarted { turn, .. } => Some(*turn),
        _ => None,
    }
}

fn job_id_for_event(kind: &ServerEventKind) -> Option<String> {
    match kind {
        ServerEventKind::JobStarted { job_id, .. }
        | ServerEventKind::JobProgressDelta { job_id, .. } => Some(job_id.clone()),
        ServerEventKind::JobFinished { job, .. } => Some(job.id.clone()),
        _ => None,
    }
}

fn frame_type_for_event(kind: &ServerEventKind) -> FrameType {
    match kind {
        ServerEventKind::Delta { .. } => standard(StandardFrameType::TokenDelta),
        ServerEventKind::MediaPartCompleted { .. } => {
            standard(StandardFrameType::OutputPartFinished)
        }
        ServerEventKind::ToolStarted { .. } => standard(StandardFrameType::ToolCallStarted),
        ServerEventKind::ToolOutput { .. } => standard(StandardFrameType::ToolOutputDelta),
        ServerEventKind::ToolFinished { .. } => standard(StandardFrameType::ToolCallFinished),
        ServerEventKind::JobStarted { .. } => standard(StandardFrameType::JobStarted),
        ServerEventKind::JobProgressDelta { .. } => standard(StandardFrameType::JobProgressDelta),
        ServerEventKind::JobFinished { .. } => standard(StandardFrameType::JobFinished),
        ServerEventKind::TurnStarted { .. }
        | ServerEventKind::TurnFinished { .. }
        | ServerEventKind::TurnStopped { .. } => standard(StandardFrameType::TurnStateChanged),
        ServerEventKind::JobList { .. } => standard(StandardFrameType::JobListResult),
        ServerEventKind::MessageReplayed { .. } => standard(StandardFrameType::HistoryPage),
        ServerEventKind::Error { .. } => standard(StandardFrameType::Error),
        ServerEventKind::SteerApplied { .. } => standard(StandardFrameType::UserInputAccepted),
        ServerEventKind::ChildSessionStarted { .. } => {
            standard(StandardFrameType::ChildSessionStarted)
        }
        ServerEventKind::ChildSessionUpdated { .. }
        | ServerEventKind::ChildAwaitingInput { .. } => {
            standard(StandardFrameType::ChildSessionUpdated)
        }
        ServerEventKind::DelegatorStateChanged { .. } => {
            standard(StandardFrameType::DelegatorStateChanged)
        }
        other => FrameType::Extension(extension_name_for_event(other)),
    }
}

/// Names a `NorteCode` event that has no faithful standard frame.
///
/// Extensions carry the adapter's own payload, so a standard frame is never
/// sent with a shape the standard does not define.
fn extension_name_for_event(kind: &ServerEventKind) -> String {
    let tag = serde_json::to_value(kind)
        .ok()
        .and_then(|value| {
            value
                .get("event")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "event".to_owned());
    format!("norte_code.{tag}")
}

fn session_for_request(kind: &ClientRequestKind) -> Option<&SessionKey> {
    match kind {
        ClientRequestKind::RenameSession { session, .. }
        | ClientRequestKind::DeleteSession { session }
        | ClientRequestKind::Steer { session, .. }
        | ClientRequestKind::CancelTurn { session }
        | ClientRequestKind::StopJobs { session, .. }
        | ClientRequestKind::DetachTurn { session }
        | ClientRequestKind::ListJobs { session }
        | ClientRequestKind::Resume { session, .. }
        | ClientRequestKind::AnswerQuestion { session, .. }
        | ClientRequestKind::SetModel { session, .. }
        | ClientRequestKind::SetReasoning { session, .. }
        | ClientRequestKind::SetBudget { session, .. }
        | ClientRequestKind::Compact { session }
        | ClientRequestKind::ForkSession { session, .. }
        | ClientRequestKind::ListTools {
            session: Some(session),
        }
        | ClientRequestKind::SetApprovalRule {
            session: Some(session),
            ..
        }
        | ClientRequestKind::ForgetApprovalRule {
            session: Some(session),
            ..
        } => Some(session),
        ClientRequestKind::SendMessage { session, .. }
        | ClientRequestKind::CostReport { session }
        | ClientRequestKind::ListTelemetry { session } => session.as_ref(),
        ClientRequestKind::CreateSession { .. }
        | ClientRequestKind::ListSessions
        | ClientRequestKind::ListModels
        | ClientRequestKind::ListTools { session: None }
        | ClientRequestKind::SetApprovalRule { session: None, .. }
        | ClientRequestKind::ForgetApprovalRule { session: None, .. }
        | ClientRequestKind::RefreshCatalog
        | ClientRequestKind::ListSkills
        | ClientRequestKind::RefreshSkills
        | ClientRequestKind::ListPlugins
        | ClientRequestKind::InstallPlugin { .. }
        | ClientRequestKind::UpdatePlugins { .. }
        | ClientRequestKind::RemovePlugin { .. }
        | ClientRequestKind::SetPluginEnabled { .. }
        | ClientRequestKind::ListMcp
        | ClientRequestKind::RefreshMcp
        | ClientRequestKind::SetMcpActive { .. } => None,
    }
}

fn is_plugin_request(kind: &ClientRequestKind) -> bool {
    matches!(
        kind,
        ClientRequestKind::ListPlugins
            | ClientRequestKind::InstallPlugin { .. }
            | ClientRequestKind::UpdatePlugins { .. }
            | ClientRequestKind::RemovePlugin { .. }
            | ClientRequestKind::SetPluginEnabled { .. }
    )
}

fn standard(kind: StandardFrameType) -> FrameType {
    FrameType::Standard(kind)
}

fn observer_read_only_frame(kind: &FrameType) -> bool {
    matches!(
        kind,
        FrameType::Standard(
            StandardFrameType::Ack
                | StandardFrameType::HistoryGet
                | StandardFrameType::JobList
                | StandardFrameType::JobGet
        )
    )
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

pub(super) fn job_status(job: &norte_code_protocol::JobInfo) -> JobStatus {
    match job.state {
        norte_code_protocol::JobState::Running => JobStatus::Running,
        norte_code_protocol::JobState::Cancelled => JobStatus::Killed,
        norte_code_protocol::JobState::Finished if job.timed_out => JobStatus::TimedOut,
        norte_code_protocol::JobState::Finished
            if job.exit_code.is_some_and(|exit_code| exit_code != 0) =>
        {
            JobStatus::Failed
        }
        norte_code_protocol::JobState::Finished => JobStatus::Completed,
    }
}

fn job_output_from_index(
    job: &norte_code_protocol::JobInfo,
    from_index: u64,
) -> Result<&str, HostError> {
    let retained_bytes = u64::try_from(job.output.len()).unwrap_or(u64::MAX);
    let retained_from = job.output_bytes_total.saturating_sub(retained_bytes);
    if from_index < retained_from {
        return Err(HostError::InvalidFrame("JOB_OUTPUT_EXPIRED".to_owned()));
    }
    if from_index > job.output_bytes_total {
        return Err(HostError::InvalidFrame(
            "JOB_GET.from_index exceeds available output".to_owned(),
        ));
    }
    let relative = usize::try_from(from_index - retained_from)
        .map_err(|_| HostError::InvalidFrame("JOB_GET.from_index is out of range".to_owned()))?;
    job.output.get(relative..).ok_or_else(|| {
        HostError::InvalidFrame("JOB_GET.from_index is not a UTF-8 boundary".to_owned())
    })
}

pub(super) fn job_finished_payload(
    job: &norte_code_protocol::JobInfo,
    triggered_new_turn: bool,
    new_turn_id: Option<&str>,
    injected_into_turn_id: Option<&str>,
) -> Value {
    let start = job
        .output
        .char_indices()
        .find_map(|(index, _)| {
            (job.output.len().saturating_sub(16 * 1024) <= index).then_some(index)
        })
        .unwrap_or(0);
    let output_snapshot = &job.output[start..];
    payload(JobFinishedPayload {
        status: job_status(job),
        exit_code: job.exit_code,
        signal: None,
        summary: &job.label,
        output_snapshot,
        output_truncated: job.dropped || start > 0,
        output_bytes_total: job.output_bytes_total,
        duration_ms: job.elapsed_ms,
        triggered_new_turn,
        new_turn_id,
        injected_into_turn_id,
    })
}

fn parse_portable_history(
    bytes: &[u8],
) -> Result<Vec<norte_code_agent::service::ImportedHistoryItem>, HostError> {
    let mut items = Vec::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let item: PortableHistoryItem = serde_json::from_slice(line)?;
        if item.rewound {
            return Err(HostError::InvalidFrame(
                "BUNDLE_UNSUPPORTED: rewound history cannot be restored by this host".to_owned(),
            ));
        }
        let content = match item.content {
            Value::String(content) => content,
            Value::Array(parts) => {
                let mut content = String::new();
                for part in parts {
                    if part.get("type").and_then(Value::as_str) != Some("text") {
                        return Err(HostError::InvalidFrame(
                            "BUNDLE_UNSUPPORTED: non-text history parts are not supported"
                                .to_owned(),
                        ));
                    }
                    let text = part.get("text").and_then(Value::as_str).ok_or_else(|| {
                        HostError::InvalidFrame(
                            "BUNDLE_INVALID: text content part has no text".to_owned(),
                        )
                    })?;
                    content.push_str(text);
                }
                content
            }
            _ => {
                return Err(HostError::InvalidFrame(
                    "BUNDLE_INVALID: history content must be text".to_owned(),
                ));
            }
        };
        items.push(norte_code_agent::service::ImportedHistoryItem {
            role: item.role.as_str().to_owned(),
            content,
            turn_id: item.turn_id,
            timestamp: item.timestamp,
            origin: item.origin.or(item.provenance),
        });
        if items.len() > 100_000 {
            return Err(HostError::InvalidFrame(
                "BUNDLE_INVALID: too many history items".to_owned(),
            ));
        }
    }
    Ok(items)
}

fn validate_bundle_usage(usage: &Value) -> Result<(), HostError> {
    let Some(object) = usage.as_object() else {
        return Err(HostError::InvalidFrame(
            "BUNDLE_INVALID: usage_total must be an object".to_owned(),
        ));
    };
    for key in [
        "input_tokens",
        "output_tokens",
        "cached_tokens",
        "reasoning_tokens",
        "tool_calls",
        "duration_ms",
    ] {
        if object
            .get(key)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(HostError::InvalidFrame(format!(
                "BUNDLE_INVALID: usage_total.{key} must be a non-negative integer"
            )));
        }
    }
    if let Some(cost) = object.get("cost") {
        let valid = cost
            .get("amount")
            .and_then(Value::as_f64)
            .is_some_and(|amount| amount >= 0.0)
            && cost
                .get("currency")
                .and_then(Value::as_str)
                .is_some_and(|currency| currency == "USD");
        if !valid {
            return Err(HostError::InvalidFrame(
                "BUNDLE_UNSUPPORTED: usage_total.cost must use USD".to_owned(),
            ));
        }
    }
    let extra = object
        .get("extra")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let Some(extra) = extra.as_object() else {
        return Err(HostError::InvalidFrame(
            "BUNDLE_INVALID: usage_total.extra must be an object".to_owned(),
        ));
    };
    if extra
        .keys()
        .any(|key| key != "requests" && key != "unpriced_requests")
        || ["reasoning_tokens", "tool_calls", "duration_ms"]
            .into_iter()
            .any(|key| object.get(key).and_then(Value::as_u64).unwrap_or(0) > 0)
    {
        return Err(HostError::InvalidFrame(
            "BUNDLE_UNSUPPORTED: usage_total has counters this host cannot preserve".to_owned(),
        ));
    }
    if ["requests", "unpriced_requests"]
        .into_iter()
        .any(|key| extra.get(key).is_some_and(|value| value.as_u64().is_none()))
    {
        return Err(HostError::InvalidFrame(
            "BUNDLE_INVALID: usage_total.extra counters must be non-negative integers".to_owned(),
        ));
    }
    Ok(())
}

fn validate_export_request(request: &ExportRequest) -> Result<(), HostError> {
    if request
        .quiesce
        .is_some_and(|policy| policy != QuiescePolicy::Reject)
    {
        return Err(HostError::InvalidFrame(
            "SESSION_EXPORT currently supports only quiesce=reject".to_owned(),
        ));
    }
    if request.wait_timeout_s.is_some() {
        return Err(HostError::InvalidFrame(
            "wait_timeout_s is only valid with quiesce=wait".to_owned(),
        ));
    }
    if request.encrypt.is_some() {
        return Err(HostError::InvalidFrame(
            "encrypted exports are not supported".to_owned(),
        ));
    }
    if request.include.children
        || request.include.uploads
        || request.include.job_output
        || request.include.outputs
        || request.include.workspace
    {
        return Err(HostError::InvalidFrame(
            "SESSION_EXPORT requires children, uploads, job_output, outputs, and workspace all false"
                .to_owned(),
        ));
    }
    Ok(())
}

type PortableHistoryEntry = (
    Option<u64>,
    norte_code_protocol::MessageRole,
    norte_code_protocol::ReplayedKind,
    String,
    i64,
    Option<Value>,
);

fn serialize_plain_text_history(
    history: Vec<PortableHistoryEntry>,
    max_bytes: usize,
) -> Result<Vec<u8>, HostError> {
    let mut bytes = Vec::new();
    for (turn, role, kind, content, timestamp, origin) in history {
        let role = match (role, kind) {
            (norte_code_protocol::MessageRole::User, norte_code_protocol::ReplayedKind::Text) => {
                ExportHistoryRole::User
            }
            (
                norte_code_protocol::MessageRole::Assistant,
                norte_code_protocol::ReplayedKind::Text,
            ) => ExportHistoryRole::Assistant,
            (
                norte_code_protocol::MessageRole::System
                | norte_code_protocol::MessageRole::Developer
                | norte_code_protocol::MessageRole::Unknown,
                _,
            ) => continue,
            _ => {
                return Err(HostError::InvalidFrame(
                    "SESSION_EXPORT supports only user and assistant text history".to_owned(),
                ));
            }
        };
        let item = HistoryExportItem {
            role,
            content: &content,
            turn_id: turn.map(|turn| format!("turn_{turn}")),
            timestamp,
            origin,
        };
        serde_json::to_writer(&mut bytes, &item)?;
        bytes.push(b'\n');
        if bytes.len() > max_bytes {
            return Err(HostError::InvalidFrame(
                "PAYLOAD_TOO_LARGE: export exceeds max_export_bytes".to_owned(),
            ));
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[test]
    fn user_input_requires_exactly_one_nonempty_form() {
        let missing: AcceptedUserInput = serde_json::from_value(json!({})).expect("decode");
        assert!(validate_user_input(&missing).is_err());

        let both: AcceptedUserInput = serde_json::from_value(json!({
            "prompt": "hello",
            "content": [{"type": "text", "text": "also hello"}]
        }))
        .expect("decode");
        assert!(validate_user_input(&both).is_err());

        let empty: AcceptedUserInput =
            serde_json::from_value(json!({"prompt": "  "})).expect("decode");
        assert!(validate_user_input(&empty).is_err());

        let prompt: AcceptedUserInput =
            serde_json::from_value(json!({"prompt": "hello"})).expect("decode");
        assert!(validate_user_input(&prompt).is_ok());
    }

    #[test]
    fn multimodal_input_accepts_only_supported_upload_parts() {
        let image: AcceptedUserInput = serde_json::from_value(json!({
            "content": [{
                "type": "image",
                "source": {"kind": "upload", "upload_id": "upl_test"},
                "mime_type": "image/png",
                "alt": "diagram"
            }]
        }))
        .expect("decode");
        assert!(validate_user_input(&image).is_ok());

        let foreign_source = serde_json::from_value::<AcceptedUserInput>(json!({
            "content": [{
                "type": "image",
                "source": {"kind": "url", "url": "https://example.test/image.png"},
                "mime_type": "image/png",
                "alt": "diagram"
            }]
        }));
        assert!(foreign_source.is_err());

        let inline = serde_json::from_value::<AcceptedUserInput>(json!({
            "content": [{
                "type": "image",
                "source": {"kind": "inline", "data": "AA=="},
                "mime_type": "image/png"
            }]
        }));
        assert!(inline.is_err());

        let video: AcceptedUserInput = serde_json::from_value(json!({
            "content": [{
                "type": "video",
                "source": {"kind": "upload", "upload_id": "upl_test"},
                "mime_type": "video/mp4",
                "duration_ms": 1200
            }]
        }))
        .expect("decode");
        assert!(validate_user_input(&video).is_ok());

        let audio: AcceptedUserInput = serde_json::from_value(json!({
            "content": [{
                "type": "audio",
                "source": {"kind": "upload", "upload_id": "upl_test"},
                "mime_type": "audio/wav",
                "duration_ms": 1200,
                "transcript": "hello"
            }]
        }))
        .expect("decode");
        assert!(validate_user_input(&audio).is_ok());

        let wrong_mime: AcceptedUserInput = serde_json::from_value(json!({
            "content": [{
                "type": "document",
                "source": {"kind": "upload", "upload_id": "upl_test"},
                "mime_type": "image/png"
            }]
        }))
        .expect("decode");
        assert!(validate_user_input(&wrong_mime).is_err());
    }

    #[tokio::test]
    async fn upload_index_keeps_session_and_owner_boundaries() {
        let index = MemoryUploadIndex::default();
        let record = UploadRecord {
            id: "upl_test".to_owned(),
            session_id: "session-a".to_owned(),
            owner: Some("alice".to_owned()),
            blob_id: "blob".to_owned(),
            mime_type: "image/png".to_owned(),
            bytes: 3,
            sha256: "abc".to_owned(),
            expires_at: now_millis() + 60_000,
            referenced: false,
        };
        index.insert(record.clone()).await.expect("insert");
        assert_eq!(index.get("upl_test").await.expect("get"), Some(record));
        index
            .mark_referenced("session-a", Some("alice"), &["upl_test".to_owned()])
            .await
            .expect("mark referenced");
        assert!(
            index
                .get("upl_test")
                .await
                .expect("get")
                .is_some_and(|upload| upload.referenced)
        );
        assert!(
            index
                .mark_referenced("session-b", Some("alice"), &["upl_test".to_owned()],)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn generated_audio_output_is_durable_and_owner_scoped() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let blobs = Arc::new(norte::InMemoryBlobStore::new());
        let host = NorteCodeRaepHost::new(service, HostConfig::default())
            .expect("host")
            .with_artifact_store(Arc::new(BlobArtifactStore::new(
                Arc::clone(&blobs) as Arc<dyn BlobStore>
            )))
            .with_upload_index(Arc::new(MemoryUploadIndex::default()));
        host.state
            .lock()
            .await
            .session_owners
            .insert("session-a".to_owned(), "alice".to_owned());

        let media = norte::MediaPart::new(
            norte::MediaKind::Audio,
            norte::MediaSource::Inline(b"generated audio".to_vec()),
            norte::MimeType::new("audio/wav").expect("mime"),
        );
        let events = host
            .emit_media_output("session-a", 7, 0, &media)
            .await
            .expect("emit media output");

        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].kind,
            standard(StandardFrameType::OutputPartStarted)
        );
        assert_eq!(
            events[1].kind,
            standard(StandardFrameType::OutputPartFinished)
        );
        let output_id = events[1].payload["source"]["output_id"]
            .as_str()
            .expect("output id");
        let (bytes, mime_type) = host
            .get_session_output_as("session-a", output_id, Some("alice"))
            .await
            .expect("fetch output");
        assert_eq!(bytes, b"generated audio");
        assert_eq!(mime_type, "audio/wav");
        assert!(
            host.get_session_output_as("session-a", output_id, Some("bob"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn generated_output_http_download_supports_ranges_and_auth() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let blobs = Arc::new(norte::InMemoryBlobStore::new());
        let host = Arc::new(
            NorteCodeRaepHost::new(
                service,
                HostConfig {
                    auth_principals: HashMap::from([
                        ("token-a".to_owned(), "alice".to_owned()),
                        ("token-b".to_owned(), "bob".to_owned()),
                    ]),
                    ..HostConfig::default()
                },
            )
            .expect("host")
            .with_artifact_store(Arc::new(BlobArtifactStore::new(
                Arc::clone(&blobs) as Arc<dyn BlobStore>
            )))
            .with_upload_index(Arc::new(MemoryUploadIndex::default())),
        );
        host.state
            .lock()
            .await
            .session_owners
            .insert("session-a".to_owned(), "alice".to_owned());
        let media = norte::MediaPart::new(
            norte::MediaKind::Audio,
            norte::MediaSource::Inline(b"generated audio".to_vec()),
            norte::MimeType::new("audio/wav").expect("mime"),
        );
        let events = host
            .emit_media_output("session-a", 7, 0, &media)
            .await
            .expect("emit media output");
        let output_id = events[1].payload["source"]["output_id"]
            .as_str()
            .expect("output id");
        let app = crate::http::router(host);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/sessions/session-a/outputs/{output_id}"))
                    .header("authorization", "Bearer token-a")
                    .header("range", "bytes=0-7")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_RANGE)
                .expect("content range"),
            "bytes 0-7/15"
        );
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        assert_eq!(&body[..], b"generate");

        let denied = app
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/sessions/session-a/outputs/{output_id}"))
                    .header("authorization", "Bearer token-b")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(denied.status(), axum::http::StatusCode::NOT_FOUND);
    }

    fn test_job_info(
        state: norte_code_protocol::JobState,
        exit_code: Option<i32>,
        timed_out: bool,
        output: &str,
        output_bytes_total: u64,
    ) -> norte_code_protocol::JobInfo {
        norte_code_protocol::JobInfo {
            id: "job-test".to_owned(),
            label: "test".to_owned(),
            script: "test".to_owned(),
            state,
            exit_code,
            timed_out,
            elapsed_ms: 10,
            output: output.to_owned(),
            dropped: output_bytes_total > output.len() as u64,
            output_bytes_total,
        }
    }

    #[test]
    fn job_state_uses_registry_status_and_retained_output_cursor() {
        let failed = test_job_info(
            norte_code_protocol::JobState::Finished,
            Some(2),
            false,
            "output",
            6,
        );
        assert_eq!(job_status(&failed), JobStatus::Failed);
        assert_eq!(
            job_output_from_index(&failed, 2).expect("retained output"),
            "tput"
        );
        assert!(matches!(
            job_output_from_index(&failed, 7),
            Err(HostError::InvalidFrame(message)) if message.contains("exceeds")
        ));

        let timed_out = test_job_info(
            norte_code_protocol::JobState::Finished,
            None,
            true,
            "output",
            10,
        );
        assert_eq!(job_status(&timed_out), JobStatus::TimedOut);
        assert!(matches!(
            job_output_from_index(&timed_out, 3),
            Err(HostError::InvalidFrame(message)) if message == "JOB_OUTPUT_EXPIRED"
        ));
    }

    #[test]
    fn history_items_keep_rewound_turn_identity() {
        let item = history_item_value(HistoryItemView {
            turn: Some(3),
            role: norte_code_protocol::MessageRole::User,
            kind: norte_code_protocol::ReplayedKind::Text,
            text: "old input".to_owned(),
            timestamp: 1,
            rewound: true,
            origin: None,
            max_content_bytes: None,
        });
        assert_eq!(item.turn_id.as_deref(), Some("turn_3"));
        assert!(item.rewound);
    }
    use norte::BlobStore;

    #[tokio::test]
    async fn file_replay_store_round_trips_frames_by_session() {
        let path = std::env::temp_dir().join(format!(
            "norte-code-raep-replay-{}.jsonl",
            FrameId::generate()
        ));
        let store = FileReplayStore::open(&path).expect("replay store");
        let mut frame = Envelope::new(
            standard(StandardFrameType::TokenDelta),
            json!({"delta": "hello"}),
            1,
        );
        frame.session_id = "session-a".to_owned();
        frame.seq = Some(7);
        store.append(&frame).await.expect("append");

        let frames = store.load("session-a").await.expect("load");
        assert_eq!(frames, vec![frame]);
        assert_eq!(store.load("session-b").await.expect("load"), Vec::new());
        std::fs::remove_file(path).expect("remove replay journal");
    }

    #[tokio::test]
    async fn file_replay_store_rotates_each_session_window() {
        let path = std::env::temp_dir().join(format!(
            "norte-code-raep-rotate-{}.jsonl",
            FrameId::generate()
        ));
        let store = FileReplayStore::open_with_capacity(&path, 2).expect("replay store");
        for seq in 1..=3 {
            let mut frame = Envelope::new(
                standard(StandardFrameType::TokenDelta),
                json!({"seq": seq}),
                i64::try_from(seq).expect("test sequence fits in i64"),
            );
            frame.session_id = "session-a".to_owned();
            frame.seq = Some(seq);
            store.append(&frame).await.expect("append");
        }
        let frames = store.load("session-a").await.expect("load");
        assert_eq!(
            frames
                .iter()
                .filter_map(|frame| frame.seq)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        std::fs::remove_file(path).expect("remove replay journal");
    }

    #[tokio::test]
    async fn file_replay_store_persists_bounded_idempotency_by_session() {
        let path = std::env::temp_dir().join(format!(
            "norte-code-raep-idempotency-{}.jsonl",
            FrameId::generate()
        ));
        let store = FileReplayStore::open_with_capacity(&path, 2).expect("replay store");
        for id in ["one", "two", "three"] {
            store
                .save_idempotency(&IdempotencyRecord {
                    session_id: "session-a".to_owned(),
                    frame_id: FrameId::new(id).expect("frame id"),
                    fingerprint: id.to_owned(),
                    turn_id: None,
                    turn_fingerprint: None,
                    service_turn_id: None,
                    response: Vec::new(),
                })
                .await
                .expect("persist idempotency");
        }
        assert_eq!(
            store
                .load_idempotency("session-a")
                .await
                .expect("load idempotency")
                .iter()
                .map(|record| record.frame_id.as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
        assert_eq!(
            store
                .load_idempotency("session-b")
                .await
                .expect("load other session"),
            Vec::new()
        );
        std::fs::remove_file(path.with_extension("idempotency.jsonl"))
            .expect("remove idempotency journal");
    }

    #[tokio::test]
    async fn sqlite_replay_store_round_trips_and_rotates() {
        let path =
            std::env::temp_dir().join(format!("norte-code-raep-replay-{}.db", FrameId::generate()));
        let store = SqliteReplayStore::open(&path, 2).expect("replay store");
        for seq in 1..=3 {
            let mut frame = Envelope::new(
                standard(StandardFrameType::TokenDelta),
                json!({"seq": seq}),
                i64::try_from(seq).expect("test sequence fits in i64"),
            );
            frame.session_id = "session-a".to_owned();
            frame.seq = Some(seq);
            store.append(&frame).await.expect("append");
        }
        let frames = store.load("session-a").await.expect("load");
        assert_eq!(
            frames
                .iter()
                .filter_map(|frame| frame.seq)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        let response = frames[0].clone();
        let record = IdempotencyRecord {
            session_id: "session-a".to_owned(),
            frame_id: FrameId::new("request-1").expect("frame id"),
            fingerprint: "request-fingerprint".to_owned(),
            turn_id: Some(TurnId::new("client-turn-1").expect("turn id")),
            turn_fingerprint: Some("turn-fingerprint".to_owned()),
            service_turn_id: Some(12),
            response: vec![response],
        };
        store
            .save_idempotency(&record)
            .await
            .expect("save idempotency record");
        assert_eq!(
            store
                .load_idempotency("session-a")
                .await
                .expect("load idempotency record"),
            vec![record]
        );
        std::fs::remove_file(path).expect("remove replay database");
    }

    #[tokio::test]
    async fn blob_artifacts_return_content_addressed_ids() {
        let blobs = Arc::new(norte::InMemoryBlobStore::new());
        let store = BlobArtifactStore::new(Arc::clone(&blobs) as Arc<dyn BlobStore>);
        let reference = store.put("image/png", b"png").await.expect("put");
        let id = reference.id.parse().expect("blob id");
        assert_eq!(blobs.get(id).await.expect("get"), b"png");
        assert_eq!(reference.mime_type, "image/png");
        assert_eq!(store.get(&reference.id).await.expect("get"), b"png");
    }

    #[tokio::test]
    async fn artifact_uploads_obey_host_limit() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let config = HostConfig {
            max_artifact_bytes: 2,
            ..HostConfig::default()
        };
        let host = NorteCodeRaepHost::new(service, config).expect("host");
        let error = host.put_artifact("text/plain", b"123").await;
        assert!(matches!(error, Err(HostError::Artifact(message)) if message.contains("limit")));
    }

    #[tokio::test]
    async fn idempotency_cache_is_bounded() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(
            service,
            HostConfig {
                idempotency_capacity: 1,
                ..HostConfig::default()
            },
        )
        .expect("host");
        for _ in 0..3 {
            let frame = Envelope::new(
                FrameType::Extension(NorteCodeExtension::Command.as_str().to_owned()),
                serde_json::to_value(ClientRequest {
                    request_id: 1,
                    kind: ClientRequestKind::ListSessions,
                })
                .expect("request"),
                1,
            );
            host.handle(frame).await.expect("request");
        }
        assert!(
            host.state.lock().await.idempotency.len() <= 1,
            "the idempotency cache must stay bounded"
        );
    }

    #[tokio::test]
    async fn command_frames_are_idempotent_and_reject_unknown_wire_versions() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
        let mut frame = Envelope::new(
            FrameType::Extension(NorteCodeExtension::Command.as_str().to_owned()),
            serde_json::to_value(ClientRequest {
                request_id: 1,
                kind: ClientRequestKind::ListSessions,
            })
            .expect("request"),
            1,
        );
        let first = host.handle(frame.clone()).await.expect("first request");
        let second = host.handle(frame.clone()).await.expect("duplicate request");
        assert_eq!(first, second);
        frame.payload = json!({"different": true});
        assert!(matches!(
            host.handle(frame.clone()).await,
            Err(HostError::IdempotencyConflict(_))
        ));
        frame.id = FrameId::generate();
        frame.kind = FrameType::Extension("future_type".to_owned());
        assert!(matches!(
            host.handle(frame.clone()).await,
            Err(HostError::UnknownType(_))
        ));
        frame.id = FrameId::generate();
        frame.kind = FrameType::Extension(NorteCodeExtension::Command.as_str().to_owned());
        frame.seq = Some(1);
        assert!(matches!(
            host.handle(frame.clone()).await,
            Err(HostError::InvalidFrame(message)) if message.contains("must not include seq")
        ));
        frame.seq = None;
        frame.version = "unsupported".to_owned();
        assert!(matches!(
            host.handle(frame).await,
            Err(HostError::InvalidFrame(message)) if message.contains("wire version")
        ));
    }

    #[tokio::test]
    async fn sqlite_idempotency_survives_host_restart() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let path = std::env::temp_dir().join(format!(
            "norte-code-raep-idempotency-{}.db",
            FrameId::generate()
        ));
        let store = Arc::new(SqliteReplayStore::open(&path, 16).expect("replay store"));
        let host = NorteCodeRaepHost::new(Arc::clone(&service), HostConfig::default())
            .expect("host")
            .with_replay_store_instance(Arc::clone(&store) as Arc<dyn ReplayStore>);
        let frame = Envelope::new(
            FrameType::Extension(NorteCodeExtension::Command.as_str().to_owned()),
            serde_json::to_value(ClientRequest {
                request_id: 1,
                kind: ClientRequestKind::ListSessions,
            })
            .expect("request"),
            1,
        );
        let first = host.handle(frame.clone()).await.expect("first request");
        drop(host);

        let restarted = NorteCodeRaepHost::new(service, HostConfig::default())
            .expect("restarted host")
            .with_replay_store_instance(store);
        assert_eq!(
            restarted.handle(frame).await.expect("retry after restart"),
            first
        );
        std::fs::remove_file(path).expect("remove replay database");
    }

    #[tokio::test]
    async fn service_turn_events_keep_the_client_turn_id() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
        let client_turn_id = TurnId::new("turn_from_client").expect("turn id");
        let queued_turn_id = TurnId::new("queued_turn").expect("queued turn id");
        {
            let mut state = host.state.lock().await;
            state
                .pending_turn_ids
                .entry("session".to_owned())
                .or_default()
                .push_back(client_turn_id.clone());
            state.queued_turns.insert(
                "session".to_owned(),
                VecDeque::from([QueuedTurn {
                    turn_id: queued_turn_id.clone(),
                    text: "queued prompt".to_owned(),
                    attachments: Vec::new(),
                    budget: None,
                    reasoning: None,
                    metadata: None,
                    output: vec![raep_types::Modality::Text],
                }]),
            );
        }
        let session = SessionKey::new("session");
        let events = vec![
            ServerEvent {
                request_id: 0,
                session: Some(session.clone()),
                kind: ServerEventKind::TurnStarted { turn: 17 },
            },
            ServerEvent {
                request_id: 0,
                session: Some(session.clone()),
                kind: ServerEventKind::ToolStarted {
                    turn: 17,
                    call_id: "call-1".to_owned(),
                    tool: "shell".to_owned(),
                    arguments: None,
                    source: None,
                },
            },
            ServerEvent {
                request_id: 0,
                session: Some(session.clone()),
                kind: ServerEventKind::ToolFinished {
                    turn: 17,
                    call_id: "call-1".to_owned(),
                    tool: "shell".to_owned(),
                    is_error: false,
                    error: None,
                    output: Some("done".to_owned()),
                    truncated: false,
                    detached: false,
                    duration_ms: 10,
                },
            },
            ServerEvent {
                request_id: 0,
                session: Some(session),
                kind: ServerEventKind::TurnStopped {
                    reason: "cancelled".to_owned(),
                },
            },
        ];
        let frames = host
            .collect_service_events("session", Box::pin(futures_util::stream::iter(events)))
            .await
            .expect("turn events");
        assert_eq!(frames.len(), 7);
        assert_eq!(frames[0].payload["state"], "CALLING_PROVIDER");
        assert_eq!(frames[1].payload["state"], "EXECUTING_TOOL");
        assert_eq!(frames[2].kind, standard(StandardFrameType::ToolCallStarted));
        assert_eq!(
            frames[3].kind,
            standard(StandardFrameType::ToolCallFinished)
        );
        assert_eq!(frames[4].payload["state"], "CALLING_PROVIDER");
        assert_eq!(frames[5].payload["state"], "FAILED");
        assert_eq!(frames[5].payload["stop_reason"], "error");
        assert!(
            frames[..6]
                .iter()
                .all(|frame| frame.turn_id.as_ref() == Some(&client_turn_id))
        );
        assert_eq!(frames[6].turn_id.as_ref(), Some(&queued_turn_id));
        assert_eq!(frames[6].payload["state"], "FAILED");
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn user_inputs_require_turn_ids_and_queue_with_a_bounded_position() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(
            service,
            HostConfig {
                max_queued_inputs: 1,
                ..HostConfig::default()
            },
        )
        .expect("host");
        let attach = Envelope::new(
            standard(StandardFrameType::SessionAttach),
            payload(SessionAttach {
                agent_id: "norte-code".to_owned(),
                agent_override: None,
                create_if_missing: Some(true),
                mode: None,
                takeover: None,
                resume_from_seq: None,
                capabilities: None,
                client_info: None,
                options: None,
            }),
            1,
        );
        let attached = host.handle(attach).await.expect("attach");
        let session_id = attached[0].session_id.clone();
        let mut missing_turn_id = Envelope::new(
            standard(StandardFrameType::UserInput),
            json!({"prompt": "must include a turn id"}),
            2,
        );
        missing_turn_id.session_id = session_id.clone();
        assert!(matches!(
            host.handle(missing_turn_id).await,
            Err(HostError::InvalidFrame(message)) if message.contains("turn_id")
        ));

        host.state.lock().await.active_turns.insert(
            session_id.clone(),
            TurnId::new("active_turn").expect("turn id"),
        );
        let mut queued = Envelope::new(
            standard(StandardFrameType::UserInput),
            json!({"prompt": "queued input", "queue_policy": "queue"}),
            3,
        );
        queued.session_id = session_id.clone();
        queued.turn_id = Some(TurnId::new("queued_turn").expect("turn id"));
        let response = host.handle(queued).await.expect("queued input");
        assert_eq!(response.len(), 2);
        assert_eq!(
            response[0].kind,
            standard(StandardFrameType::UserInputAccepted)
        );
        assert_eq!(response[0].payload["position"], 1);
        assert_eq!(
            response[0].turn_id.as_ref().map(TurnId::as_str),
            Some("queued_turn")
        );
        assert_eq!(response[1].payload["state"], "QUEUED");

        let mut conflicting_retry = Envelope::new(
            standard(StandardFrameType::UserInput),
            json!({"prompt": "different content", "queue_policy": "queue"}),
            4,
        );
        conflicting_retry.session_id = session_id.clone();
        conflicting_retry.turn_id = Some(TurnId::new("queued_turn").expect("turn id"));
        assert!(matches!(
            host.handle(conflicting_retry).await,
            Err(HostError::IdempotencyConflict(_))
        ));

        let mut overflow = Envelope::new(
            standard(StandardFrameType::UserInput),
            json!({"prompt": "one too many", "queue_policy": "queue"}),
            5,
        );
        overflow.session_id = session_id.clone();
        overflow.turn_id = Some(TurnId::new("overflow_turn").expect("turn id"));
        assert!(matches!(
            host.handle(overflow).await,
            Err(HostError::SessionBusy)
        ));
        let mut cancel_queued =
            Envelope::new(standard(StandardFrameType::CancelTurn), json!({}), 6);
        cancel_queued.session_id = session_id;
        cancel_queued.turn_id = Some(TurnId::new("queued_turn").expect("turn id"));
        let cancelled = host
            .handle(cancel_queued)
            .await
            .expect("cancel queued turn");
        assert_eq!(cancelled[0].payload["state"], "CANCELLED");
        assert_eq!(cancelled[0].payload["stop_reason"], "cancelled");
    }

    #[tokio::test]
    async fn attach_delivers_replay_between_attached_and_state() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
        let first = host
            .handle(Envelope::new(
                standard(StandardFrameType::SessionAttach),
                payload(SessionAttach {
                    agent_id: "norte-code".to_owned(),
                    agent_override: None,
                    create_if_missing: Some(true),
                    mode: None,
                    takeover: None,
                    resume_from_seq: None,
                    capabilities: None,
                    client_info: None,
                    options: None,
                }),
                1,
            ))
            .await
            .expect("initial attach");
        let session_id = first[0].session_id.clone();
        let mut detach = Envelope::new(standard(StandardFrameType::SessionDetach), json!({}), 2);
        detach.session_id = session_id.clone();
        host.handle(detach).await.expect("detach");
        host.emit(
            &session_id,
            StandardFrameType::TokenDelta,
            json!({"delta": "replayed"}),
        )
        .await
        .expect("emit replay frame");

        let mut resume = Envelope::new(
            standard(StandardFrameType::SessionAttach),
            payload(SessionAttach {
                agent_id: "norte-code".to_owned(),
                agent_override: None,
                create_if_missing: Some(false),
                mode: None,
                takeover: None,
                resume_from_seq: Some(2),
                capabilities: None,
                client_info: None,
                options: None,
            }),
            3,
        );
        resume.session_id = session_id.clone();
        let resumed = host.handle(resume).await.expect("resume");
        assert_eq!(resumed.len(), 3);
        assert_eq!(
            resumed
                .iter()
                .map(|frame| frame.kind.clone())
                .collect::<Vec<_>>(),
            [
                standard(StandardFrameType::SessionAttached),
                standard(StandardFrameType::TokenDelta),
                standard(StandardFrameType::SessionState),
            ]
        );
        let current_seq = resumed
            .iter()
            .filter_map(|frame| frame.seq)
            .max()
            .expect("resumed sequence");
        let mut future_ack = Envelope::new(
            standard(StandardFrameType::Ack),
            json!({"last_seq": current_seq + 1}),
            4,
        );
        future_ack.session_id = session_id.clone();
        assert!(matches!(
            host.handle(future_ack).await,
            Err(HostError::InvalidFrame(message)) if message.contains("exceeds")
        ));
        let mut ack = Envelope::new(
            standard(StandardFrameType::Ack),
            json!({"last_seq": current_seq}),
            5,
        );
        ack.session_id = session_id;
        host.handle(ack).await.expect("ack emitted frames");
        assert_eq!(
            resumed
                .iter()
                .filter_map(|frame| frame.seq)
                .collect::<Vec<_>>(),
            [4, 3, 5]
        );
    }

    #[tokio::test]
    async fn connection_limit_is_enforced_without_waiting() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(
            service,
            HostConfig {
                max_connections: 1,
                ..HostConfig::default()
            },
        )
        .expect("host");
        let permit = host.try_connection().expect("first connection");
        assert!(host.try_connection().is_none());
        drop(permit);
        assert!(host.try_connection().is_some());
    }

    #[tokio::test]
    async fn attaching_emits_schema_shaped_state_without_replaying_history() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
        let hello = host.hello();
        assert_eq!(hello.payload["limits"]["max_concurrent_jobs"], 8);
        assert_eq!(hello.payload["limits"]["job_output_retention_s"], 86_400);
        let info = host.info();
        assert_eq!(info["limits"]["max_concurrent_jobs"], 8);
        assert_eq!(info["limits"]["job_output_retention_s"], 86_400);
        let frame = Envelope::new(
            standard(StandardFrameType::SessionAttach),
            json!({"agent_id": "norte-code", "create_if_missing": true}),
            1,
        );
        let frames = host.handle(frame).await.expect("attach");
        assert_eq!(frames.len(), 2, "attach must not replay history");
        assert_eq!(frames[0].kind, standard(StandardFrameType::SessionAttached));
        assert_eq!(frames[0].payload["created"], true);
        assert_eq!(frames[0].payload["mode"], "controller");
        assert_eq!(frames[0].payload["last_seq"], 0);
        assert_eq!(frames[1].kind, standard(StandardFrameType::SessionState));
        assert_eq!(frames[1].payload["status"], "ACTIVE");
        assert!(
            frames[1].payload["history_summary"].is_array(),
            "SESSION_STATE must carry history_summary"
        );
    }

    #[tokio::test]
    async fn attaching_existing_session_infers_its_agent_from_service() {
        let service = || {
            Arc::new(
                Service::new(
                    norte_code_protocol::config::Config::default(),
                    Arc::new(norte_code_protocol::catalog::Catalog::default()),
                    None,
                    Arc::new(norte_code_agent::VendorModels),
                    Arc::new(norte_code_protocol::InMemoryStore::new()),
                    Arc::new(norte::InMemoryBlobStore::new()),
                    "system",
                )
                .expect("service"),
            )
        };
        let default_service = service();
        let subagent_service = service();
        let session_id = subagent_service
            .start_session(None)
            .await
            .expect("start session")
            .to_string();
        let host = NorteCodeRaepHost::with_agents(
            HashMap::from([
                ("norte-code".to_owned(), default_service),
                ("norte-code-subagent".to_owned(), subagent_service),
            ]),
            HostConfig {
                agent_id: "norte-code".to_owned(),
                ..HostConfig::default()
            },
        )
        .expect("host");
        let mut frame = Envelope::new(
            standard(StandardFrameType::SessionAttach),
            json!({"agent_id": "norte-code-subagent", "create_if_missing": false}),
            1,
        );
        frame.session_id = session_id;

        assert!(
            host.handle(frame)
                .await
                .expect("attach to existing subagent session")
                .iter()
                .any(|frame| frame.kind == standard(StandardFrameType::SessionAttached))
        );
    }

    #[tokio::test]
    async fn history_get_uses_turn_cursor_and_rejects_bad_requests() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
        let attached = host
            .handle(Envelope::new(
                standard(StandardFrameType::SessionAttach),
                json!({"agent_id": "norte-code", "create_if_missing": true}),
                1,
            ))
            .await
            .expect("attach");
        let session_id = attached[0].session_id.clone();
        let mut request = Envelope::new(
            standard(StandardFrameType::HistoryGet),
            json!({"limit": 10}),
            2,
        );
        request.session_id = session_id.clone();
        let page = host.handle(request).await.expect("history page");
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].kind, standard(StandardFrameType::HistoryPage));
        assert_eq!(page[0].payload["items"], json!([]));
        assert_eq!(page[0].payload["has_more"], false);

        let mut invalid_cursor = Envelope::new(
            standard(StandardFrameType::HistoryGet),
            json!({"before_turn_id": "not-a-turn"}),
            3,
        );
        invalid_cursor.session_id = session_id;
        assert!(matches!(
            host.handle(invalid_cursor).await,
            Err(HostError::InvalidFrame(message)) if message.contains("before_turn_id")
        ));

        let mut obsolete_cursor = Envelope::new(
            standard(StandardFrameType::HistoryGet),
            json!({"after": "1"}),
            4,
        );
        obsolete_cursor.session_id = attached[0].session_id.clone();
        assert!(host.handle(obsolete_cursor).await.is_err());

        let mut invalid_limit = Envelope::new(
            standard(StandardFrameType::HistoryGet),
            json!({"limit": 101}),
            5,
        );
        invalid_limit.session_id = attached[0].session_id.clone();
        assert!(matches!(
            host.handle(invalid_limit).await,
            Err(HostError::InvalidFrame(message)) if message.contains("limit")
        ));

        let mut unknown_turn = Envelope::new(
            standard(StandardFrameType::HistoryGet),
            json!({"before_turn_id": "turn_999"}),
            6,
        );
        unknown_turn.session_id = attached[0].session_id.clone();
        assert!(matches!(
            host.handle(unknown_turn).await,
            Err(HostError::InvalidFrame(message)) if message.contains("TURN_NOT_FOUND")
        ));
    }

    #[tokio::test]
    async fn text_bundle_export_is_bounded_and_not_advertised_as_full_portability() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
        let info = host.info();
        let capabilities = info["capabilities"].as_array().expect("capability list");
        let capability_names = capabilities
            .iter()
            .filter_map(Value::as_str)
            .collect::<HashSet<_>>();
        assert!(capability_names.contains("transport.ws"));
        assert!(capability_names.contains("transport.sse"));
        assert!(capability_names.contains("history"));
        assert!(capability_names.contains("X_NORTECODE_JOBS_BASIC"));
        assert!(capability_names.contains("X_NORTECODE_DELEGATION_BASIC"));
        for unsupported in [
            "jobs",
            "delegation",
            "handoff",
            "quota",
            "manage",
            "reconfigure",
            "fork",
            "compaction",
            "export",
            "import",
        ] {
            assert!(
                !capability_names.contains(unsupported),
                "unsupported capability {unsupported} must not be advertised"
            );
        }
        assert!(matches!(
            NorteCodeRaepHost::import(),
            Err(HostError::InvalidFrame(message)) if message.contains("HTTP workflow")
        ));

        let attached = host
            .handle(Envelope::new(
                standard(StandardFrameType::SessionAttach),
                json!({"agent_id": "norte-code", "create_if_missing": true}),
                1,
            ))
            .await
            .expect("attach");
        let session_id = attached[0].session_id.clone();
        let mut default_export =
            Envelope::new(standard(StandardFrameType::SessionExport), json!({}), 2);
        default_export.session_id = session_id.clone();
        assert!(matches!(
            host.handle(default_export).await,
            Err(HostError::InvalidFrame(message)) if message.contains("all false")
        ));
        let mut export = Envelope::new(
            standard(StandardFrameType::SessionExport),
            json!({"include": {
                "children": false,
                "uploads": false,
                "job_output": false,
                "outputs": false,
                "workspace": false
            }}),
            3,
        );
        export.session_id = session_id;
        let ready = host.handle_as(export, Some("local")).await.expect("export");
        assert_eq!(
            ready[0].kind,
            standard(StandardFrameType::SessionExportReady)
        );
        let export_id = ready[0].payload["export_id"].as_str().expect("export id");
        let (bytes, _) = host
            .get_export_as(export_id, "local")
            .await
            .expect("bundle");
        assert!(host.get_export_as(export_id, "other").await.is_err());
        let (manifest, files) =
            crate::bundle::read_bundle(&bytes, 1024 * 1024).expect("valid exported bundle");
        assert_eq!(manifest.sessions.len(), 1);
        assert_eq!(files.len(), 2);
    }

    #[tokio::test]
    async fn observer_attach_can_read_history_but_cannot_control_session() {
        let service = Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
        let attached = host
            .handle(Envelope::new(
                standard(StandardFrameType::SessionAttach),
                json!({
                    "agent_id": "norte-code",
                    "create_if_missing": true,
                    "mode": "observer"
                }),
                1,
            ))
            .await
            .expect("observer attach");
        let session_id = attached[0].session_id.clone();
        let mut history = Envelope::new(
            standard(StandardFrameType::HistoryGet),
            json!({"limit": 10}),
            2,
        );
        history.session_id = session_id.clone();
        assert!(host.handle(history).await.is_ok());

        let mut input = Envelope::new(
            standard(StandardFrameType::UserInput),
            json!({"prompt": "must be rejected"}),
            3,
        );
        input.session_id = session_id;
        assert!(matches!(
            host.handle(input).await,
            Err(HostError::InvalidFrame(message)) if message.contains("observer")
        ));
    }

    #[test]
    fn ordinary_tool_output_is_not_mislabeled_as_a_job() {
        let kind = ServerEventKind::ToolOutput {
            turn: 1,
            call_id: "job-1".to_owned(),
            tool: "shell".to_owned(),
            stream: norte_code_protocol::types::OutputStream::Stdout,
            text: "hi".to_owned(),
            last: false,
        };
        assert_eq!(job_id_for_event(&kind), None);
        assert_eq!(turn_id_for_event(&kind), Some(1));
    }

    #[test]
    fn job_list_status_filters_use_the_closed_status_enum() {
        let request: JobListRequest = serde_json::from_value(json!({
            "status": ["RUNNING", "TIMED_OUT"],
            "limit": 10
        }))
        .expect("valid job list request");
        assert_eq!(
            request.status,
            Some(vec![JobStatus::Running, JobStatus::TimedOut])
        );
        assert!(
            serde_json::from_value::<JobListRequest>(json!({
                "status": ["RUNNIG"]
            }))
            .is_err()
        );
    }

    #[test]
    fn detached_job_events_use_standard_frames_and_job_envelope_ids() {
        let started = ServerEventKind::JobStarted {
            job_id: "job-1".to_owned(),
            tool_call_id: "call-1".to_owned(),
            command: "make".to_owned(),
            started_at: 1_700_000_000_000,
            limits: norte_code_protocol::JobLimits {
                timeout_s: Some(120),
                max_output_bytes: Some(1_048_576),
            },
        };
        assert_eq!(
            frame_type_for_event(&started),
            standard(StandardFrameType::JobStarted)
        );
        assert_eq!(job_id_for_event(&started).as_deref(), Some("job-1"));

        let progress = ServerEventKind::JobProgressDelta {
            job_id: "job-1".to_owned(),
            stream: norte_code_protocol::OutputStream::Stdout,
            delta: "ok\n".to_owned(),
            index: 0,
        };
        assert_eq!(
            frame_type_for_event(&progress),
            standard(StandardFrameType::JobProgressDelta)
        );
        assert_eq!(job_id_for_event(&progress).as_deref(), Some("job-1"));
        assert_eq!(turn_id_for_event(&progress), None);

        let finished = ServerEventKind::JobFinished {
            job: norte_code_protocol::JobInfo {
                id: "job-1".to_owned(),
                label: "make".to_owned(),
                script: "make".to_owned(),
                state: norte_code_protocol::JobState::Finished,
                exit_code: Some(0),
                timed_out: false,
                elapsed_ms: 10,
                output: "ok\n".to_owned(),
                dropped: false,
                output_bytes_total: 3,
            },
            triggered_new_turn: false,
            new_turn_id: None,
            injected_into_turn_id: None,
        };
        assert_eq!(
            frame_type_for_event(&finished),
            standard(StandardFrameType::JobFinished)
        );
        assert_eq!(job_id_for_event(&finished).as_deref(), Some("job-1"));
    }

    #[test]
    fn session_job_turn_policy_is_parsed_and_unknown_options_are_rejected() {
        assert_eq!(
            parse_job_turn_policy(Some(&json!({"job_turn_policy": "always"})))
                .expect("always policy"),
            Some(norte_code_protocol::JobTurnPolicy::Always)
        );
        assert_eq!(
            parse_job_turn_policy(Some(&json!({"job_turn_policy": "on_failure"})))
                .expect("on_failure policy"),
            Some(norte_code_protocol::JobTurnPolicy::OnFailure)
        );
        assert_eq!(
            parse_job_turn_policy(Some(&json!({"job_turn_policy": "never"})))
                .expect("never policy"),
            Some(norte_code_protocol::JobTurnPolicy::Never)
        );
        assert!(
            parse_job_turn_policy(Some(&json!({"notify": true}))).is_err(),
            "unsupported session options fail rather than being ignored"
        );
    }

    #[test]
    fn questions_use_a_namespaced_server_frame() {
        let frame = frame_type_for_event(&ServerEventKind::QuestionAsked {
            turn: 1,
            question: "approval".to_owned(),
            prompt: "Continue?".to_owned(),
            options: Vec::new(),
            multiple: false,
        });
        assert_eq!(
            frame,
            FrameType::Extension("norte_code.question_asked".to_owned())
        );
    }
}
