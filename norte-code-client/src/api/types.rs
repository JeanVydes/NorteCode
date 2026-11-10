//! The shapes the host answers with.

use serde::Deserialize;

use norte_code_wire::{
    AgentSummary, CatalogModelInfo, CatalogProviderInfo, McpFailure, McpInfo, SkillInfo,
};

/// The editable catalogue as the host returns it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CatalogDetail {
    /// One entry per provider, file and plugin alike.
    #[serde(default)]
    pub providers: Vec<CatalogProviderInfo>,
    /// One entry per model.
    #[serde(default)]
    pub models: Vec<CatalogModelInfo>,
}

/// What the server says about itself.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerInfo {
    /// The server's own version.
    #[serde(default)]
    pub server_version: String,
    /// The protocol it speaks.
    #[serde(default)]
    pub protocol: Option<ProtocolInfo>,
    /// What it can do.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// The extension frames it accepts.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// The bounds it advertises.
    #[serde(default)]
    pub limits: ServerLimits,
    /// The agent a client gets when it names none.
    #[serde(default)]
    pub default_agent: String,
}

/// The protocol identity.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProtocolInfo {
    /// Its name.
    #[serde(default)]
    pub name: String,
    /// Its version.
    #[serde(default)]
    pub version: String,
}

/// The bounds the server advertises.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerLimits {
    /// Largest frame accepted.
    #[serde(rename = "max_frame_bytes")]
    pub frame_bytes: Option<u64>,
    /// Largest number of concurrent jobs.
    #[serde(rename = "max_concurrent_jobs")]
    pub concurrent_jobs: Option<u64>,
    /// Largest artifact accepted.
    #[serde(rename = "max_artifact_bytes")]
    pub artifact_bytes: Option<u64>,
    /// Largest prompt accepted.
    #[serde(rename = "max_prompt_bytes")]
    pub prompt_bytes: Option<u64>,
}

/// A health probe status.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// The process is up.
    Live,
    /// It can serve.
    Ready,
}

/// The liveness answer.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Liveness {
    /// The status.
    pub status: HealthStatus,
}

/// The readiness answer.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Readiness {
    /// The status.
    pub status: HealthStatus,
    /// The default agent.
    pub agent_id: String,
    /// How many agents are configured.
    pub agents: usize,
    /// Whether replay is on.
    pub replay: bool,
    /// Whether artifacts are on.
    pub artifacts: bool,
    /// The connection bounds.
    pub limits: ReadinessLimits,
}

/// The readiness connection bounds.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ReadinessLimits {
    /// Concurrent connections.
    pub max_connections: usize,
    /// Per-command timeout.
    pub command_timeout_ms: u64,
}

/// One installed plugin.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct PluginSummary {
    /// Its name.
    #[serde(default)]
    pub name: String,
    /// Whether it is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Whether it loaded.
    #[serde(default)]
    pub loaded: bool,
}

/// The result of a plugin operation.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct PluginChange {
    /// What happened.
    #[serde(default)]
    pub operation: String,
    /// Which plugin.
    #[serde(default)]
    pub name: String,
}

/// The installed skills and what was skipped.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct SkillCatalog {
    /// One entry per valid skill.
    #[serde(default)]
    pub skills: Vec<SkillInfo>,
    /// Skills that were skipped, with the reason.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// One tool rule.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ToolRule {
    /// The tool's name.
    #[serde(default)]
    pub name: String,
    /// Whether it may run.
    #[serde(default)]
    pub allowed: bool,
    /// Where the rule applies.
    #[serde(default)]
    pub scope: String,
}

/// The MCP servers and the ones that failed.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct McpCatalog {
    /// Connected servers.
    #[serde(default)]
    pub servers: Vec<McpInfo>,
    /// Servers that failed to start.
    #[serde(default)]
    pub failures: Vec<McpFailure>,
}

/// The provider-reported cost totals.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct CostReport {
    /// The total, when the provider reported one.
    #[serde(default)]
    pub total_usd: Option<f64>,
    /// How many requests are in the total.
    #[serde(default)]
    pub requests: u64,
}

#[derive(Deserialize)]
pub(crate) struct AgentListResponse {
    #[serde(default)]
    pub(crate) agents: Vec<AgentSummary>,
}

#[derive(Deserialize)]
pub(crate) struct SessionListResponse {
    #[serde(default)]
    pub(crate) sessions: Vec<norte_code_wire::SessionSummary>,
}

#[derive(Deserialize)]
pub(crate) struct PlanListResponse {
    #[serde(default)]
    pub(crate) entries: Vec<norte_code_wire::PlanSummary>,
}

#[derive(Deserialize)]
pub(crate) struct ChildSessionListResponse {
    #[serde(default)]
    pub(crate) children: Vec<norte_code_wire::ChildSessionSummary>,
}

#[derive(Deserialize)]
pub(crate) struct AgentCatalogResponse {
    #[serde(default)]
    pub(crate) entries: Vec<norte_code_wire::ModelInfo>,
}

#[derive(Deserialize)]
pub(crate) struct PluginListResponse {
    #[serde(default)]
    pub(crate) plugins: Vec<PluginSummary>,
}

#[derive(Deserialize)]
pub(crate) struct PluginChangedResponse {
    #[serde(default)]
    pub(crate) operation: String,
    #[serde(default)]
    pub(crate) name: String,
}

#[derive(Deserialize)]
pub(crate) struct AgentCatalogSkillsResponse {
    #[serde(default)]
    pub(crate) skills: Vec<SkillInfo>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct McpListResponse {
    #[serde(default)]
    pub(crate) servers: Vec<McpInfo>,
    #[serde(default)]
    pub(crate) failures: Vec<McpFailure>,
}

#[derive(Deserialize)]
pub(crate) struct CostUpdatedResponse {
    pub(crate) total_usd: f64,
    pub(crate) requests: u64,
}

#[derive(Deserialize)]
pub(crate) struct CostNotReportedResponse {
    pub(crate) requests: u64,
}

#[derive(Deserialize)]
pub(crate) struct JobListResponse {
    #[serde(default)]
    pub(crate) jobs: Vec<norte_code_wire::JobSummary>,
}

#[derive(serde::Serialize)]
pub(crate) struct JobListRequest {
    pub(crate) status: Option<Vec<&'static str>>,
    pub(crate) limit: u64,
}

#[derive(serde::Serialize)]
pub(crate) struct HistoryGetRequest {
    pub(crate) before_turn_id: Option<String>,
    pub(crate) limit: u64,
    pub(crate) include_rewound: bool,
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub(crate) struct ExportRequest {
    pub(crate) include: ExportInclude,
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct ExportInclude {
    pub(crate) children: bool,
    pub(crate) uploads: bool,
    pub(crate) job_output: bool,
    pub(crate) outputs: bool,
    pub(crate) workspace: bool,
}

/// A `SESSION_REWIND` body.
#[derive(serde::Serialize)]
pub(crate) struct RewindRequest<'a> {
    pub(crate) to_turn_id: &'a str,
    pub(crate) inclusive: bool,
    pub(crate) cancel_pending: bool,
    pub(crate) workspace: RewindWorkspace,
}

/// What a rewind does to the workspace.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RewindWorkspace {
    Keep,
}

/// A `SESSION_FORK` body.
#[derive(serde::Serialize)]
pub(crate) struct ForkRequest<'a> {
    pub(crate) from_turn_id: Option<&'a str>,
    pub(crate) title: Option<&'a str>,
    pub(crate) include: ForkInclude,
}

/// What a fork copies besides the messages.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub(crate) struct ForkInclude {
    pub(crate) uploads: bool,
    pub(crate) outputs: bool,
    pub(crate) workspace: bool,
}

/// A `SESSION_COMPACT` body.
#[derive(serde::Serialize)]
pub(crate) struct CompactRequest {
    pub(crate) strategy: CompactStrategy,
}

/// How a compaction replaces the history.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompactStrategy {
    Summarize,
}

/// A `CONTEXT_PIN` body.
#[derive(serde::Serialize)]
pub(crate) struct PinRequest<'a> {
    pub(crate) turn_ids: &'a [String],
    pub(crate) pinned: bool,
}

/// One `USER_INPUT`: a prompt or content parts, never both.
#[derive(serde::Serialize)]
#[serde(untagged)]
pub(crate) enum UserInputRequest {
    Prompt {
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        replaces_turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        queue_policy: Option<String>,
    },
    Content {
        content: Vec<norte_code_wire::UserInputPart>,
        #[serde(skip_serializing_if = "Option::is_none")]
        replaces_turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        queue_policy: Option<String>,
    },
}

impl UserInputRequest {
    pub(crate) fn build(
        prompt: Option<String>,
        content: Option<Vec<norte_code_wire::UserInputPart>>,
        replaces_turn_id: Option<String>,
        queue_policy: Option<String>,
    ) -> Option<Self> {
        match (prompt, content) {
            (Some(prompt), _) if !prompt.is_empty() => Some(Self::Prompt {
                prompt,
                replaces_turn_id,
                queue_policy,
            }),
            (_, Some(content)) if !content.is_empty() => Some(Self::Content {
                content,
                replaces_turn_id,
                queue_policy,
            }),
            _ => None,
        }
    }
}
