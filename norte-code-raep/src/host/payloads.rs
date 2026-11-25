//! The payloads this host serializes, one struct per frame.

use norte_code_wire::SessionKey;
use raep_types::TurnId;
use serde_json::Value;
use std::collections::BTreeMap;

#[allow(clippy::wildcard_imports)]
use super::*;

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DelegatorControlRequest {
    pub(crate) action: DelegatorControlAction,
    pub(crate) child_session_id: SessionKey,
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DelegatorControlAction {
    Suspend,
    Release,
}

/// The session-state payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SessionStatePayload<'a> {
    pub(crate) status: SessionStateStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<&'a str>,
    pub(crate) history_summary: Vec<HistorySummaryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) imported_from: Option<&'a Value>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum SessionStateStatus {
    Active,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct HistorySummaryItem {
    pub(crate) role: norte_code_wire::HistorySpeaker,
    pub(crate) kind: norte_code_wire::HistoryItemKind,
    pub(crate) content: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) timestamp: i64,
    pub(crate) rewound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) origin: Option<Value>,
}

pub(crate) struct HistoryItemView {
    pub(crate) turn: Option<u64>,
    pub(crate) role: norte_code_wire::MessageRole,
    pub(crate) kind: norte_code_wire::ReplayedKind,
    pub(crate) text: String,
    pub(crate) timestamp: i64,
    pub(crate) rewound: bool,
    pub(crate) origin: Option<Value>,
    pub(crate) max_content_bytes: Option<usize>,
}

pub(crate) fn history_item_value(item: HistoryItemView) -> HistorySummaryItem {
    let role = match item.role {
        norte_code_wire::MessageRole::Tool => norte_code_wire::HistorySpeaker::Tool,
        norte_code_wire::MessageRole::System
        | norte_code_wire::MessageRole::Developer
        | norte_code_wire::MessageRole::Unknown => norte_code_wire::HistorySpeaker::SystemEvent,
        norte_code_wire::MessageRole::User => norte_code_wire::HistorySpeaker::User,
        norte_code_wire::MessageRole::Assistant => norte_code_wire::HistorySpeaker::Assistant,
    };
    let kind = match item.kind {
        norte_code_wire::ReplayedKind::Text => match role {
            norte_code_wire::HistorySpeaker::Tool => norte_code_wire::HistoryItemKind::Tool,
            norte_code_wire::HistorySpeaker::SystemEvent => {
                norte_code_wire::HistoryItemKind::SystemEvent
            }
            norte_code_wire::HistorySpeaker::User => norte_code_wire::HistoryItemKind::User,
            norte_code_wire::HistorySpeaker::Assistant => {
                norte_code_wire::HistoryItemKind::Assistant
            }
        },
        norte_code_wire::ReplayedKind::Reasoning => norte_code_wire::HistoryItemKind::Reasoning,
        norte_code_wire::ReplayedKind::ToolCall => norte_code_wire::HistoryItemKind::ToolCall,
        norte_code_wire::ReplayedKind::ToolResult => norte_code_wire::HistoryItemKind::ToolResult,
        norte_code_wire::ReplayedKind::Refusal => norte_code_wire::HistoryItemKind::Refusal,
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
pub(crate) struct HistoryPagePayload {
    pub(crate) items: Vec<HistorySummaryItem>,
    pub(crate) has_more: bool,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct HistoryRewoundPayload {
    pub(crate) to_turn_id: String,
    pub(crate) inclusive: bool,
    pub(crate) rewound_turn_ids: Vec<String>,
    pub(crate) side_effects: Vec<HistorySideEffect>,
    pub(crate) workspace_restored: bool,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct HistorySideEffect {
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) side_effects: raep_types::ToolSideEffects,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct JobStatePayload<'a> {
    pub(crate) job_id: &'a str,
    pub(crate) status: JobStatus,
    pub(crate) output: &'a str,
    pub(crate) next_index: u64,
}

// The status the host writes is the one the clients read: no second enum.
pub(crate) use norte_code_wire::JobStatus;

#[derive(Debug, serde::Deserialize)]
pub(crate) struct JobListRequest {
    pub(crate) status: Option<Vec<JobStatus>>,
    pub(crate) limit: Option<u64>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct JobListEntry {
    pub(crate) status: JobStatus,
    #[serde(flatten)]
    pub(crate) fields: BTreeMap<String, Value>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct JobListResultPayload {
    pub(crate) jobs: Vec<JobListEntry>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ContextUpdatedPayload {
    pub(crate) pinned_turn_ids: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ExportReadyPayload<'a> {
    pub(crate) export_id: &'a str,
    pub(crate) download_url: String,
    pub(crate) bytes: usize,
    pub(crate) sha256: &'a str,
    pub(crate) expires_at: i64,
    pub(crate) sessions: u32,
    pub(crate) encrypted: bool,
    pub(crate) signed: bool,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ImportAgentAvailability<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) version: &'a str,
    pub(crate) available: bool,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ImportSignatureStatus {
    Absent,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ImportPreviewPayload<'a> {
    pub(crate) import_id: &'a str,
    pub(crate) bundle_version: &'a str,
    pub(crate) sessions: usize,
    pub(crate) signature: ImportSignatureStatus,
    pub(crate) agents: Vec<ImportAgentAvailability<'a>>,
    pub(crate) warnings: [&'static str; 2],
    pub(crate) expires_at: i64,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ImportedFromPayload<'a> {
    pub(crate) exporter: &'a Option<Value>,
    pub(crate) session_id: &'a str,
    pub(crate) exported_at: u64,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ImportCommittedPayload<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) id_map: BTreeMap<String, String>,
    pub(crate) warnings: [&'static str; 2],
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct HistoryExportItem<'a> {
    pub(crate) role: ExportHistoryRole,
    pub(crate) content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) turn_id: Option<String>,
    pub(crate) timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) origin: Option<Value>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ExportHistoryRole {
    User,
    Assistant,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct BundleAgent<'a> {
    pub(crate) id: &'a str,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct PortableSessionFile<'a> {
    pub(crate) agent: BundleAgent<'a>,
    pub(crate) title: &'a Option<String>,
    pub(crate) options: BTreeMap<String, Value>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct BundleExporter<'a> {
    pub(crate) server: &'a str,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SessionClosedReason {
    Client,
    Purged,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct SessionClosedPayload {
    pub(crate) reason: SessionClosedReason,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct DelegatorStateChangedPayload<'a> {
    pub(crate) child_session_id: &'a str,
    pub(crate) suspended: bool,
    pub(crate) by: DelegatorActor,
    pub(crate) queued_messages: usize,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DelegatorActor {
    Client,
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputPartKind {
    Image,
    Document,
    Audio,
    Video,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct OutputPartStartedPayload<'a> {
    pub(crate) part_id: &'a str,
    pub(crate) index: u32,
    #[serde(rename = "type")]
    pub(crate) kind: OutputPartKind,
    pub(crate) mime_type: &'a str,
    pub(crate) streamed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) alt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) height: Option<u32>,
    pub(crate) bytes_expected: usize,
}

#[derive(Debug, serde::Serialize)]
pub(crate) enum OutputPartStatus {
    #[serde(rename = "OK")]
    Ok,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum OutputPartSource<'a> {
    Output { output_id: &'a str },
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct OutputPartFinishedPayload<'a> {
    pub(crate) part_id: &'a str,
    pub(crate) index: u32,
    pub(crate) status: OutputPartStatus,
    pub(crate) source: OutputPartSource<'a>,
    pub(crate) bytes: usize,
    pub(crate) sha256: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) transcript: Option<&'a str>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct UserInputAcceptedPayload {
    pub(crate) turn_id: TurnId,
    pub(crate) origin: UserInputOrigin,
    pub(crate) content: Vec<AcceptedContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) position: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) replaces_turn_id: Option<String>,
}
