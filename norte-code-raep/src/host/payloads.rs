//! The payloads this host serializes, one struct per frame.

use norte_code_wire::ServerEvent;
use raep_types::TurnId;
use serde_json::Value;
use std::collections::BTreeMap;

#[allow(clippy::wildcard_imports)]
use super::*;

/// The session-state payload.
#[derive(Debug, serde::Serialize)]
pub(super) struct SessionStatePayload<'a> {
    pub(super) status: SessionStateStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<&'a str>,
    pub(super) history_summary: Vec<HistorySummaryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) imported_from: Option<&'a Value>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum SessionStateStatus {
    Active,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct HistorySummaryItem {
    pub(super) role: HistoryRole,
    pub(super) kind: HistoryItemKind,
    pub(super) content: String,
    pub(super) turn_id: Option<String>,
    pub(super) timestamp: i64,
    pub(super) rewound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) origin: Option<Value>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum HistoryRole {
    Tool,
    SystemEvent,
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum HistoryItemKind {
    Tool,
    SystemEvent,
    User,
    Assistant,
    Reasoning,
    ToolCall,
    ToolResult,
    Refusal,
}

pub(super) struct HistoryItemView {
    pub(super) turn: Option<u64>,
    pub(super) role: norte_code_wire::MessageRole,
    pub(super) kind: norte_code_wire::ReplayedKind,
    pub(super) text: String,
    pub(super) timestamp: i64,
    pub(super) rewound: bool,
    pub(super) origin: Option<Value>,
    pub(super) max_content_bytes: Option<usize>,
}

pub(super) fn history_item_value(item: HistoryItemView) -> HistorySummaryItem {
    let role = match item.role {
        norte_code_wire::MessageRole::Tool => HistoryRole::Tool,
        norte_code_wire::MessageRole::System
        | norte_code_wire::MessageRole::Developer
        | norte_code_wire::MessageRole::Unknown => HistoryRole::SystemEvent,
        norte_code_wire::MessageRole::User => HistoryRole::User,
        norte_code_wire::MessageRole::Assistant => HistoryRole::Assistant,
    };
    let kind = match item.kind {
        norte_code_wire::ReplayedKind::Text => match role {
            HistoryRole::Tool => HistoryItemKind::Tool,
            HistoryRole::SystemEvent => HistoryItemKind::SystemEvent,
            HistoryRole::User => HistoryItemKind::User,
            HistoryRole::Assistant => HistoryItemKind::Assistant,
        },
        norte_code_wire::ReplayedKind::Reasoning => HistoryItemKind::Reasoning,
        norte_code_wire::ReplayedKind::ToolCall => HistoryItemKind::ToolCall,
        norte_code_wire::ReplayedKind::ToolResult => HistoryItemKind::ToolResult,
        norte_code_wire::ReplayedKind::Refusal => HistoryItemKind::Refusal,
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
pub(super) struct HistoryPagePayload {
    pub(super) items: Vec<HistorySummaryItem>,
    pub(super) has_more: bool,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct HistoryRewoundPayload {
    pub(super) to_turn_id: String,
    pub(super) inclusive: bool,
    pub(super) rewound_turn_ids: Vec<String>,
    pub(super) side_effects: Vec<HistorySideEffect>,
    pub(super) workspace_restored: bool,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct HistorySideEffect {
    pub(super) tool_call_id: String,
    pub(super) tool_name: String,
    pub(super) side_effects: raep_types::ToolSideEffects,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct JobStatePayload<'a> {
    pub(super) job_id: &'a str,
    pub(super) status: JobStatus,
    pub(super) output: &'a str,
    pub(super) next_index: u64,
}

// The status the host writes is the one the clients read: no second enum.
pub(super) use norte_code_wire::JobStatus;

#[derive(Debug, serde::Deserialize)]
pub(super) struct JobListRequest {
    pub(super) status: Option<Vec<JobStatus>>,
    pub(super) limit: Option<u64>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(super) struct JobListEntry {
    pub(super) status: JobStatus,
    #[serde(flatten)]
    pub(super) fields: BTreeMap<String, Value>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct JobListResultPayload {
    pub(super) jobs: Vec<JobListEntry>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ContextUpdatedPayload {
    pub(super) pinned_turn_ids: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ExportReadyPayload<'a> {
    pub(super) export_id: &'a str,
    pub(super) download_url: String,
    pub(super) bytes: usize,
    pub(super) sha256: &'a str,
    pub(super) expires_at: i64,
    pub(super) sessions: u32,
    pub(super) encrypted: bool,
    pub(super) signed: bool,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ImportAgentAvailability<'a> {
    pub(super) agent_id: &'a str,
    pub(super) version: &'a str,
    pub(super) available: bool,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ImportSignatureStatus {
    Absent,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ImportPreviewPayload<'a> {
    pub(super) import_id: &'a str,
    pub(super) bundle_version: &'a str,
    pub(super) sessions: usize,
    pub(super) signature: ImportSignatureStatus,
    pub(super) agents: Vec<ImportAgentAvailability<'a>>,
    pub(super) warnings: [&'static str; 2],
    pub(super) expires_at: i64,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ImportedFromPayload<'a> {
    pub(super) exporter: &'a Option<Value>,
    pub(super) session_id: &'a str,
    pub(super) exported_at: u64,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ImportCommittedPayload<'a> {
    pub(super) session_id: &'a str,
    pub(super) id_map: BTreeMap<String, String>,
    pub(super) warnings: [&'static str; 2],
}

#[derive(Debug, serde::Serialize)]
pub(super) struct HistoryExportItem<'a> {
    pub(super) role: ExportHistoryRole,
    pub(super) content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) turn_id: Option<String>,
    pub(super) timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) origin: Option<Value>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ExportHistoryRole {
    User,
    Assistant,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct BundleAgent<'a> {
    pub(super) id: &'a str,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct PortableSessionFile<'a> {
    pub(super) agent: BundleAgent<'a>,
    pub(super) title: &'a Option<String>,
    pub(super) options: BTreeMap<String, Value>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct BundleExporter<'a> {
    pub(super) server: &'a str,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum SessionClosedReason {
    Client,
    Purged,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct SessionClosedPayload {
    pub(super) reason: SessionClosedReason,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct DelegatorStateChangedPayload<'a> {
    pub(super) child_session_id: &'a str,
    pub(super) suspended: bool,
    pub(super) by: DelegatorActor,
    pub(super) queued_messages: usize,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DelegatorActor {
    Client,
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OutputPartKind {
    Image,
    Document,
    Audio,
    Video,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct OutputPartStartedPayload<'a> {
    pub(super) part_id: &'a str,
    pub(super) index: u32,
    #[serde(rename = "type")]
    pub(super) kind: OutputPartKind,
    pub(super) mime_type: &'a str,
    pub(super) streamed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) alt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) height: Option<u32>,
    pub(super) bytes_expected: usize,
}

#[derive(Debug, serde::Serialize)]
pub(super) enum OutputPartStatus {
    #[serde(rename = "OK")]
    Ok,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(super) enum OutputPartSource<'a> {
    Output { output_id: &'a str },
}

#[derive(Debug, serde::Serialize)]
pub(super) struct OutputPartFinishedPayload<'a> {
    pub(super) part_id: &'a str,
    pub(super) index: u32,
    pub(super) status: OutputPartStatus,
    pub(super) source: OutputPartSource<'a>,
    pub(super) bytes: usize,
    pub(super) sha256: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) transcript: Option<&'a str>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct NorteCodeEventExtension<'a> {
    pub(super) norte_code: &'a ServerEvent,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct UserInputAcceptedPayload {
    pub(super) turn_id: TurnId,
    pub(super) origin: UserInputOrigin,
    pub(super) content: Vec<AcceptedContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) position: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) replaces_turn_id: Option<String>,
}
