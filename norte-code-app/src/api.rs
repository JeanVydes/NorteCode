use crate::domain::{Capability, ConversationItem, HistoryKind, HistoryRole, MessageDelivery};
use norte_code_wire::CanonicalEvent;
use norte_code_wire::{CatalogModelInfo, CatalogProviderInfo};
use raep_types::TurnId;
use raep_types::{ClientInfo, Envelope, FrameType, SessionAttach, StandardFrameType};
use reqwest::Url;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

/// Which model a session starts on, as `SessionCreate` accepts it.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentOverride<'a> {
    pub model: AgentModelRef<'a>,
    pub provider: &'a str,
}

/// The model half of an override.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentModelRef<'a> {
    pub id: &'a str,
}

/// The `SESSION_EXPORT` body: every optional section off, so the bundle holds
/// the conversation and its metadata only.
#[derive(Clone, Copy, Debug, Default, Serialize)]
struct ExportRequest {
    include: ExportInclude,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct ExportInclude {
    children: bool,
    uploads: bool,
    job_output: bool,
    outputs: bool,
    workspace: bool,
}

// The settings `SET_REASONING` and `SET_BUDGET` carry are the protocol's own
// values, shared with the terminal client and the host's decoder.
use norte_code_wire::{Budget, Reasoning, ReasoningScope};

/// The editable catalogue as the host returns it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CatalogDetail {
    #[serde(default)]
    pub providers: Vec<CatalogProviderInfo>,
    #[serde(default)]
    pub models: Vec<CatalogModelInfo>,
}

const CLIENT_ID: &str = "norte-code-dioxus";
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base_url: Url,
    bearer_token: Option<String>,
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Invalid server URL: {0}")]
    InvalidUrl(String),
    #[error("Request failed: {0}")]
    Request(String),
    #[error("Server returned HTTP {0}")]
    HttpStatus(u16),
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub server_version: String,
    #[serde(default)]
    pub protocol: Option<ProtocolInfo>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub limits: ServerLimits,
    #[serde(default)]
    pub default_agent: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Live,
    Ready,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Liveness {
    pub status: HealthStatus,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Readiness {
    pub status: HealthStatus,
    pub agent_id: String,
    pub agents: usize,
    pub replay: bool,
    pub artifacts: bool,
    pub limits: ReadinessLimits,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ReadinessLimits {
    pub max_connections: usize,
    pub command_timeout_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProtocolInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerLimits {
    #[serde(rename = "max_frame_bytes")]
    pub frame_bytes: Option<u64>,
    #[serde(rename = "max_concurrent_jobs")]
    pub concurrent_jobs: Option<u64>,
    #[serde(rename = "max_artifact_bytes")]
    pub artifact_bytes: Option<u64>,
    #[serde(rename = "max_prompt_bytes")]
    pub prompt_bytes: Option<u64>,
}

use norte_code_wire::AgentSummary;

// The model list is the protocol's model description, not a second one.
use norte_code_wire::{ModelInfo as ModelSummary, QuotaReport};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct PluginSummary {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub loaded: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct PluginChange {
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct SkillCatalog {
    #[serde(default)]
    pub skills: Vec<Value>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ToolRule {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub allowed: bool,
    #[serde(default)]
    pub scope: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct McpCatalog {
    #[serde(default)]
    pub servers: Vec<Value>,
    #[serde(default)]
    pub failures: Vec<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct CostReport {
    #[serde(default)]
    pub total_usd: Option<f64>,
    #[serde(default)]
    pub requests: u64,
}

// The session list is the shared shape, decoded from the same definition the
// host serializes.
use norte_code_wire::SessionSummary;

/// Where a child session stands.
///
/// The same type the event stream carries, not a second enum that has to be
/// translated: two names for one state is how `RUNNING` and `Streaming` drift
/// apart and a subagent shows as waiting when it is working.
use norte_code_wire::ChildSessionSummary;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct UploadReceipt {
    pub upload_id: String,
    pub mime_type: String,
    pub bytes: u64,
}

// The telemetry shapes are the wire's: the host serializes them from the same
// definitions this client decodes.
use norte_code_wire::TelemetryReport;

// The job and plan shapes are the wire's, not this client's: the host serializes
// them from the same definitions, so a status cannot disagree with the server.
use norte_code_wire::{JobSummary, PlanSummary};

#[derive(Deserialize)]
struct AgentListResponse {
    #[serde(default)]
    agents: Vec<AgentSummary>,
}

#[derive(Deserialize)]
struct SessionListResponse {
    #[serde(default)]
    sessions: Vec<SessionSummary>,
}

#[derive(Deserialize)]
struct PlanListResponse {
    #[serde(default)]
    entries: Vec<PlanSummary>,
}

#[derive(Deserialize)]
struct ChildSessionListResponse {
    #[serde(default)]
    children: Vec<ChildSessionSummary>,
}

#[derive(Deserialize)]
pub(crate) struct CommandResponse {
    #[serde(default)]
    pub(crate) accepted: bool,
    #[serde(default)]
    pub(crate) events: Vec<Envelope<Value>>,
    #[serde(default)]
    pub(crate) error: Option<RemoteError>,
}

#[derive(Deserialize)]
pub(crate) struct RemoteError {
    #[serde(default)]
    pub(crate) message: String,
}

#[derive(Deserialize)]
struct RemoteErrorResponse {
    #[serde(default)]
    error: Option<RemoteError>,
}

#[derive(Serialize)]
struct CreateSessionRequest<'a> {
    agent_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    /// The model the session starts on, applied before its first turn. The
    /// host translates it through the same path as `SESSION_CONFIGURE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_override: Option<AgentOverride<'a>>,
}

#[derive(Serialize)]
struct RenameSessionRequest<'a> {
    title: &'a str,
}

#[derive(Serialize)]
struct HistoryGetRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    before_turn_id: Option<String>,
    limit: u64,
    include_rewound: bool,
}

#[derive(Deserialize)]
struct HistoryPage {
    #[serde(default)]
    items: Vec<HistoryItem>,
}

#[derive(Deserialize)]
struct HistoryItem {
    role: HistoryRole,
    kind: HistoryKind,
    content: String,
    #[serde(default)]
    turn_id: Option<String>,
    timestamp: i64,
}

#[derive(Serialize)]
struct EmptyRequest {}

#[derive(Serialize)]
struct NorteCodeRequest {
    request_id: u64,
    kind: NorteCodeCommand,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NorteCodeCommand {
    ListModels,
    ListCatalog,
    CatalogUpsertProvider {
        provider_id: String,
        provider: Value,
    },
    CatalogRemoveProvider {
        provider_id: String,
    },
    CatalogUpsertModel {
        model: Value,
    },
    CatalogRemoveModel {
        provider_id: String,
        model_id: String,
    },
    ListPlans {
        session: String,
    },
    SetReasoning {
        session: String,
        scope: ReasoningScope,
        reasoning: Reasoning,
    },
    SetBudget {
        session: String,
        budget: Budget,
    },
    ListPlugins,
    InstallPlugin {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        package: Option<String>,
        all: bool,
    },
    UpdatePlugins {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    RemovePlugin {
        name: String,
    },
    SetPluginEnabled {
        name: String,
        enabled: bool,
    },
    ListSkills,
    RefreshSkills,
    ListTools {
        session: Option<String>,
    },
    SetApprovalRule {
        session: Option<String>,
        tool: String,
        allow: bool,
        global: bool,
    },
    ForgetApprovalRule {
        session: Option<String>,
        tool: String,
        global: bool,
    },
    ListMcp,
    RefreshMcp,
    SetMcpActive {
        name: String,
        active: bool,
    },
    CostReport {
        session: Option<String>,
    },
    SetModel {
        session: String,
        provider: String,
        model: String,
    },
    AnswerQuestion {
        session: String,
        question: String,
        answer: String,
    },
}

#[derive(Deserialize)]
struct AgentCatalogResponse {
    #[serde(default)]
    entries: Vec<ModelSummary>,
}

#[derive(Deserialize)]
struct PluginListResponse {
    #[serde(default)]
    plugins: Vec<PluginSummary>,
}

#[derive(Deserialize)]
struct PluginChangedResponse {
    #[serde(default)]
    operation: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct AgentCatalogSkillsResponse {
    #[serde(default)]
    skills: Vec<Value>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct McpListResponse {
    #[serde(default)]
    servers: Vec<Value>,
    #[serde(default)]
    failures: Vec<Value>,
}

#[derive(Deserialize)]
struct CostUpdatedResponse {
    total_usd: f64,
    requests: u64,
}

#[derive(Deserialize)]
struct CostNotReportedResponse {
    requests: u64,
}

impl ApiClient {
    pub fn new(base_url: &str, bearer_token: &str) -> Result<Self, ApiError> {
        let mut base_url =
            Url::parse(base_url.trim()).map_err(|error| ApiError::InvalidUrl(error.to_string()))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(ApiError::InvalidUrl(
                "Only HTTP and HTTPS server URLs are supported.".to_owned(),
            ));
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            return Err(ApiError::InvalidUrl(
                "The server URL cannot contain a query or fragment.".to_owned(),
            ));
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        Ok(Self {
            http: reqwest::Client::new(),
            base_url,
            bearer_token: (!bearer_token.trim().is_empty()).then(|| bearer_token.trim().to_owned()),
        })
    }

    pub async fn info(&self) -> Result<ServerInfo, ApiError> {
        self.get_json("raep/v1/info").await
    }

    pub async fn health_live(&self) -> Result<Liveness, ApiError> {
        self.get_json("raep/v1/health/live").await
    }

    pub async fn health_ready(&self) -> Result<Readiness, ApiError> {
        self.get_json("raep/v1/health/ready").await
    }

    pub async fn quota(&self) -> Result<QuotaReport, ApiError> {
        self.get_json("raep/v1/quota").await
    }

    pub async fn agents(&self) -> Result<Vec<AgentSummary>, ApiError> {
        let response: AgentListResponse = self.get_json("raep/v1/agents").await?;
        Ok(response.agents)
    }

    pub async fn sessions(&self) -> Result<Vec<SessionSummary>, ApiError> {
        let response: SessionListResponse = self.get_json("raep/v1/sessions").await?;
        Ok(response.sessions)
    }

    pub async fn child_sessions(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<ChildSessionSummary>, ApiError> {
        let url = self.session_url(parent_session_id, "children")?;
        let response: ChildSessionListResponse =
            self.send_json(self.auth(self.http.get(url))).await?;
        Ok(response.children)
    }

    pub async fn create_session(
        &self,
        agent_id: &str,
        title: Option<&str>,
    ) -> Result<SessionSummary, ApiError> {
        self.create_session_with_model(agent_id, title, None).await
    }

    /// Creates a session pinned to one provider/model from its first turn.
    pub async fn create_session_with_model(
        &self,
        agent_id: &str,
        title: Option<&str>,
        model: Option<(&str, &str)>,
    ) -> Result<SessionSummary, ApiError> {
        let agent_override = model.map(|(provider, model)| AgentOverride {
            model: AgentModelRef { id: model },
            provider,
        });
        let url = self.resource_url("raep/v1/sessions")?;
        self.send_json(self.auth(self.http.post(url).json(&CreateSessionRequest {
            agent_id,
            title,
            agent_override,
        })))
        .await
    }

    pub async fn rename_session(&self, session_id: &str, title: &str) -> Result<(), ApiError> {
        let url = self.session_resource_url(session_id)?;
        self.send_status(self.auth(self.http.patch(url).json(&RenameSessionRequest { title })))
            .await
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<(), ApiError> {
        let url = self.session_resource_url(session_id)?;
        self.send_status(self.auth(self.http.delete(url))).await
    }

    pub async fn telemetry(&self, session_id: Option<&str>) -> Result<TelemetryReport, ApiError> {
        let mut url = self.resource_url("raep/v1/telemetry")?;
        if let Some(session_id) = session_id {
            url.query_pairs_mut().append_pair("session", session_id);
        }
        self.send_json(self.http.get(url)).await
    }

    pub async fn models(&self) -> Result<Vec<ModelSummary>, ApiError> {
        let response = self
            .norte_code_command("unused", NorteCodeCommand::ListModels)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::agent_models().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("Model catalogue was not returned.".to_owned())
            })?;
        Ok(decode_payload::<AgentCatalogResponse>(&payload.payload)?.entries)
    }

    pub async fn plugins(&self) -> Result<Vec<PluginSummary>, ApiError> {
        let response = self
            .norte_code_command("unused", NorteCodeCommand::ListPlugins)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::PluginList.matches(&frame.payload))
            .ok_or_else(|| ApiError::InvalidResponse("Plugin list was not returned.".to_owned()))?;
        Ok(decode_payload::<PluginListResponse>(&payload.payload)?.plugins)
    }

    pub async fn install_plugin(
        &self,
        url: &str,
        package: Option<&str>,
        all: bool,
    ) -> Result<PluginChange, ApiError> {
        self.plugin_change(NorteCodeCommand::InstallPlugin {
            url: url.to_owned(),
            package: package.map(str::to_owned),
            all,
        })
        .await
    }

    pub async fn update_plugin(&self, name: Option<&str>) -> Result<PluginChange, ApiError> {
        self.plugin_change(NorteCodeCommand::UpdatePlugins {
            name: name.map(str::to_owned),
        })
        .await
    }

    pub async fn remove_plugin(&self, name: &str) -> Result<PluginChange, ApiError> {
        self.plugin_change(NorteCodeCommand::RemovePlugin {
            name: name.to_owned(),
        })
        .await
    }

    pub async fn set_plugin_enabled(
        &self,
        name: &str,
        enabled: bool,
    ) -> Result<PluginChange, ApiError> {
        self.plugin_change(NorteCodeCommand::SetPluginEnabled {
            name: name.to_owned(),
            enabled,
        })
        .await
    }

    pub async fn set_session_model(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
    ) -> Result<(), ApiError> {
        self.norte_code_command(
            session_id,
            NorteCodeCommand::SetModel {
                session: session_id.to_owned(),
                provider: provider.to_owned(),
                model: model.to_owned(),
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn skills(&self) -> Result<SkillCatalog, ApiError> {
        self.read_skills().await
    }

    pub async fn refresh_skills(&self) -> Result<SkillCatalog, ApiError> {
        self.norte_code_command("unused", NorteCodeCommand::RefreshSkills)
            .await?;
        self.read_skills().await
    }

    async fn read_skills(&self) -> Result<SkillCatalog, ApiError> {
        let response = self
            .norte_code_command("unused", NorteCodeCommand::ListSkills)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::agent_skills().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("Skill catalogue was not returned.".to_owned())
            })?;
        let entries: AgentCatalogSkillsResponse = decode_payload(&payload.payload["entries"])?;
        Ok(SkillCatalog {
            skills: entries.skills,
            warnings: entries.warnings,
        })
    }

    pub async fn tool_rules(&self) -> Result<Vec<ToolRule>, ApiError> {
        let response = self
            .norte_code_command("unused", NorteCodeCommand::ListTools { session: None })
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::tool_list().matches(&frame.payload))
            .ok_or_else(|| ApiError::InvalidResponse("Tool rules were not returned.".to_owned()))?;
        decode_payload(&payload.payload["data"])
    }

    pub async fn set_tool_allowed(&self, tool: &str, allow: bool) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            NorteCodeCommand::SetApprovalRule {
                session: None,
                tool: tool.to_owned(),
                allow,
                global: true,
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn forget_tool_rule(&self, tool: &str) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            NorteCodeCommand::ForgetApprovalRule {
                session: None,
                tool: tool.to_owned(),
                global: true,
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn mcp_catalog(&self) -> Result<McpCatalog, ApiError> {
        self.read_mcp_catalog().await
    }

    pub async fn refresh_mcp(&self) -> Result<McpCatalog, ApiError> {
        self.norte_code_command("unused", NorteCodeCommand::RefreshMcp)
            .await?;
        self.read_mcp_catalog().await
    }

    async fn read_mcp_catalog(&self) -> Result<McpCatalog, ApiError> {
        let response = self
            .norte_code_command("unused", NorteCodeCommand::ListMcp)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::mcp_list().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("MCP server list was not returned.".to_owned())
            })?;
        let data: McpListResponse = decode_payload(&payload.payload["data"])?;
        Ok(McpCatalog {
            servers: data.servers,
            failures: data.failures,
        })
    }

    pub async fn set_mcp_active(&self, name: &str, active: bool) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            NorteCodeCommand::SetMcpActive {
                name: name.to_owned(),
                active,
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn cost_report(&self, session_id: Option<&str>) -> Result<CostReport, ApiError> {
        let response = self
            .norte_code_command(
                session_id.unwrap_or("unused"),
                NorteCodeCommand::CostReport {
                    session: session_id.map(str::to_owned),
                },
            )
            .await?;
        if let Some(payload) = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::CostUpdated.matches(&frame.payload))
        {
            let report: CostUpdatedResponse = decode_payload(&payload.payload)?;
            return Ok(CostReport {
                total_usd: Some(report.total_usd),
                requests: report.requests,
            });
        }
        if let Some(payload) = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::CostNotReported.matches(&frame.payload))
        {
            let report: CostNotReportedResponse = decode_payload(&payload.payload)?;
            return Ok(CostReport {
                total_usd: None,
                requests: report.requests,
            });
        }
        Err(ApiError::InvalidResponse(
            "Cost report was not returned.".to_owned(),
        ))
    }

    pub async fn answer_question(
        &self,
        session_id: &str,
        question: &str,
        answer: &str,
    ) -> Result<(), ApiError> {
        self.norte_code_command(
            session_id,
            NorteCodeCommand::AnswerQuestion {
                session: session_id.to_owned(),
                question: question.to_owned(),
                answer: answer.to_owned(),
            },
        )
        .await
        .map(|_| ())
    }

    async fn plugin_change(&self, command: NorteCodeCommand) -> Result<PluginChange, ApiError> {
        let response = self.norte_code_command("unused", command).await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::PluginChanged.matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("Plugin operation result was not returned.".to_owned())
            })?;
        let change = decode_payload::<PluginChangedResponse>(&payload.payload)?;
        Ok(PluginChange {
            operation: change.operation,
            name: change.name,
        })
    }

    async fn norte_code_command(
        &self,
        session_id: &str,
        kind: NorteCodeCommand,
    ) -> Result<CommandResponse, ApiError> {
        let request = NorteCodeRequest {
            request_id: u64::try_from(now_millis()?)
                .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            kind,
        };
        let response = self
            .post_frame(
                session_id,
                FrameType::Extension("norte_code.command".to_owned()),
                &request,
            )
            .await?;
        if response.accepted {
            Ok(response)
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Norte Code command was rejected.".to_owned(),
                |error| error.message,
            )))
        }
    }

    pub async fn attach(&self, session_id: &str, agent_id: &str) -> Result<(), ApiError> {
        let payload = SessionAttach {
            agent_id: agent_id.to_owned(),
            agent_override: None,
            create_if_missing: Some(false),
            mode: None,
            takeover: None,
            resume_from_seq: None,
            capabilities: None,
            client_info: Some(ClientInfo {
                name: Some(CLIENT_ID.to_owned()),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            options: None,
        };
        let response = self
            .post_frame(
                session_id,
                FrameType::Standard(StandardFrameType::SessionAttach),
                &payload,
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Session attachment was rejected.".to_owned(),
                |e| e.message,
            )))
        }
    }

    pub async fn open_session_events(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<crate::sse::SessionEventStream, ApiError> {
        let mut url = self.session_url(session_id, "events")?;
        url.query_pairs_mut()
            .append_pair("create_if_missing", "false");
        if !agent_id.trim().is_empty() {
            url.query_pairs_mut().append_pair("agent_id", agent_id);
        }
        #[cfg(target_arch = "wasm32")]
        {
            crate::sse::SessionEventStream::connect(url.as_str(), self.bearer_token.as_deref())
                .await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::sse::SessionEventStream::connect(
                &self.http,
                url.as_str(),
                self.bearer_token.as_deref(),
            )
            .await
        }
    }

    pub async fn detach(&self, session_id: &str) -> Result<(), ApiError> {
        let response = self
            .post_frame(
                session_id,
                FrameType::Standard(StandardFrameType::SessionDetach),
                &EmptyRequest {},
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Session detachment was rejected.".to_owned(),
                |e| e.message,
            )))
        }
    }

    pub async fn upload(
        &self,
        session_id: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<UploadReceipt, ApiError> {
        let url = self.session_url(session_id, "uploads")?;
        self.send_json(
            self.auth(
                self.http
                    .post(url)
                    .header(reqwest::header::CONTENT_TYPE, mime_type)
                    .body(bytes),
            ),
        )
        .await
    }

    pub async fn history(&self, session_id: &str) -> Result<Vec<ConversationItem>, ApiError> {
        let response = self
            .post_frame(
                session_id,
                FrameType::Standard(StandardFrameType::HistoryGet),
                &HistoryGetRequest {
                    before_turn_id: None,
                    limit: 100,
                    include_rewound: false,
                },
            )
            .await?;
        let page = response
            .events
            .iter()
            .find(|frame| frame.kind == FrameType::Standard(StandardFrameType::HistoryPage))
            .ok_or_else(|| {
                ApiError::InvalidResponse("History page was not returned.".to_owned())
            })?;
        let page: HistoryPage = decode_payload(&page.payload)?;
        Ok(page
            .items
            .into_iter()
            .enumerate()
            .map(|(index, item)| ConversationItem {
                id: format!(
                    "{}:{}:{index}",
                    item.turn_id.as_deref().unwrap_or("unassigned"),
                    item.timestamp
                ),
                turn_id: item.turn_id,
                role: item.role,
                kind: item.kind,
                content: item.content,
                created_at: item.timestamp,
                delivery: MessageDelivery::Sent,
                tool: None,
                usage: None,
            })
            .collect())
    }

    pub async fn cancel_turn(&self, session_id: &str, turn_id: &TurnId) -> Result<(), ApiError> {
        let response = self
            .post_turn_frame(
                session_id,
                FrameType::Standard(StandardFrameType::CancelTurn),
                turn_id,
                &EmptyRequest {},
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Turn cancellation was rejected.".to_owned(),
                |e| e.message,
            )))
        }
    }

    /// Sets how much the session reasons, now or for the next message.
    pub async fn set_reasoning(
        &self,
        session_id: &str,
        scope: ReasoningScope,
        reasoning: Reasoning,
    ) -> Result<(), ApiError> {
        self.norte_code_command(
            session_id,
            NorteCodeCommand::SetReasoning {
                session: session_id.to_owned(),
                scope,
                reasoning,
            },
        )
        .await
        .map(|_| ())
    }

    /// Sets the session's spending limit.
    pub async fn set_budget(&self, session_id: &str, budget: Budget) -> Result<(), ApiError> {
        self.norte_code_command(
            session_id,
            NorteCodeCommand::SetBudget {
                session: session_id.to_owned(),
                budget,
            },
        )
        .await
        .map(|_| ())
    }

    /// Asks the host to package the session and returns the export id.
    pub async fn export_session(&self, session_id: &str) -> Result<String, ApiError> {
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionExport),
            serde_json::to_value(ExportRequest::default())
                .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            now_millis()?,
        );
        frame.session_id = session_id.to_owned();
        let response = self.post_command(session_id, &frame).await?;
        if !response.accepted {
            return Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "The export was refused.".to_owned(),
                |error| error.message,
            )));
        }
        response
            .events
            .iter()
            .find_map(|event| event.payload["export_id"].as_str().map(str::to_owned))
            .ok_or_else(|| ApiError::InvalidResponse("The export id was not returned.".to_owned()))
    }

    /// The URL the export's bundle downloads from.
    pub fn export_url(&self, export_id: &str) -> Result<String, ApiError> {
        Ok(self
            .resource_url(&format!("raep/v1/exports/{export_id}"))?
            .to_string())
    }

    /// Lists the session's plans.
    pub async fn plans(&self, session_id: &str) -> Result<Vec<PlanSummary>, ApiError> {
        let response = self
            .norte_code_command(
                session_id,
                NorteCodeCommand::ListPlans {
                    session: session_id.to_owned(),
                },
            )
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::plan_list().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("The plan list was not returned.".to_owned())
            })?;
        Ok(decode_payload::<PlanListResponse>(&payload.payload)?.entries)
    }

    /// The catalogue with its origin, for the editor.
    pub async fn catalog_detail(&self) -> Result<CatalogDetail, ApiError> {
        let response = self
            .norte_code_command("unused", NorteCodeCommand::ListCatalog)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::catalog_providers().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("The catalogue detail was not returned.".to_owned())
            })?;
        // The providers catalogue is an `agent_catalog` envelope: the entries
        // carry both lists, under `entries`, not at the top level.
        decode_payload(&payload.payload["entries"])
    }

    /// Writes one provider into `providers.json` and reloads the catalogue.
    pub async fn upsert_provider(
        &self,
        provider_id: &str,
        provider: Value,
    ) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            NorteCodeCommand::CatalogUpsertProvider {
                provider_id: provider_id.to_owned(),
                provider,
            },
        )
        .await
        .map(|_| ())
    }

    /// Removes a provider and its models from the catalogue files.
    pub async fn remove_provider(&self, provider_id: &str) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            NorteCodeCommand::CatalogRemoveProvider {
                provider_id: provider_id.to_owned(),
            },
        )
        .await
        .map(|_| ())
    }

    /// Writes one model into `models.json`.
    pub async fn upsert_model(&self, model: Value) -> Result<(), ApiError> {
        self.norte_code_command("unused", NorteCodeCommand::CatalogUpsertModel { model })
            .await
            .map(|_| ())
    }

    /// Removes one model from `models.json`.
    pub async fn remove_model(&self, provider_id: &str, model_id: &str) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            NorteCodeCommand::CatalogRemoveModel {
                provider_id: provider_id.to_owned(),
                model_id: model_id.to_owned(),
            },
        )
        .await
        .map(|_| ())
    }

    /// The session's background jobs, as the host retains them.
    pub async fn list_jobs(&self, session_id: &str) -> Result<Vec<JobSummary>, ApiError> {
        #[derive(Serialize)]
        struct JobListRequest {
            status: Option<Vec<&'static str>>,
            limit: u64,
        }
        #[derive(Deserialize)]
        struct JobListResponse {
            #[serde(default)]
            jobs: Vec<JobSummary>,
        }
        let response = self
            .post_frame(
                session_id,
                FrameType::Standard(StandardFrameType::JobList),
                &JobListRequest {
                    status: None,
                    limit: 100,
                },
            )
            .await?;
        let result = response
            .events
            .iter()
            .find(|frame| frame.kind == FrameType::Standard(StandardFrameType::JobListResult))
            .ok_or_else(|| {
                ApiError::InvalidResponse("The job list was not returned.".to_owned())
            })?;
        let result: JobListResponse = decode_payload(&result.payload)?;
        Ok(result.jobs)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let url = self.resource_url(path)?;
        self.send_json(self.auth(self.http.get(url))).await
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let response = request
            .send()
            .await
            .map_err(|error| ApiError::Request(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.json::<RemoteErrorResponse>().await.ok();
            return Err(body.and_then(|body| body.error).map_or_else(
                || ApiError::HttpStatus(status.as_u16()),
                |error| ApiError::InvalidResponse(error.message),
            ));
        }
        response
            .json()
            .await
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))
    }

    async fn send_status(&self, request: reqwest::RequestBuilder) -> Result<(), ApiError> {
        let response = request
            .send()
            .await
            .map_err(|error| ApiError::Request(error.to_string()))?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let body = response.json::<RemoteErrorResponse>().await.ok();
        Err(body.and_then(|body| body.error).map_or_else(
            || ApiError::HttpStatus(status.as_u16()),
            |error| ApiError::InvalidResponse(error.message),
        ))
    }

    async fn post_frame<P: Serialize>(
        &self,
        session_id: &str,
        kind: FrameType,
        payload: &P,
    ) -> Result<CommandResponse, ApiError> {
        let url = self.session_url(session_id, "commands")?;
        let payload = serde_json::to_value(payload)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let mut frame = Envelope::new(kind, payload, now_millis()?);
        frame.session_id = session_id.to_owned();
        self.send_json(self.auth(self.http.post(url).json(&frame)))
            .await
    }

    /// Posts one frame to the session's command endpoint.
    pub(crate) async fn post_command(
        &self,
        session_id: &str,
        frame: &Envelope<Value>,
    ) -> Result<CommandResponse, ApiError> {
        let url = self.session_url(session_id, "commands")?;
        self.send_json(self.auth(self.http.post(url).json(frame)))
            .await
    }

    async fn post_turn_frame<P: Serialize>(
        &self,
        session_id: &str,
        kind: FrameType,
        turn_id: &TurnId,
        payload: &P,
    ) -> Result<CommandResponse, ApiError> {
        let url = self.session_url(session_id, "commands")?;
        let payload = serde_json::to_value(payload)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let mut frame = Envelope::new(kind, payload, now_millis()?);
        frame.session_id = session_id.to_owned();
        frame.turn_id = Some(turn_id.clone());
        self.send_json(self.auth(self.http.post(url).json(&frame)))
            .await
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.bearer_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    fn resource_url(&self, path: &str) -> Result<Url, ApiError> {
        self.base_url
            .join(path)
            .map_err(|error| ApiError::InvalidUrl(error.to_string()))
    }

    fn session_url(&self, session_id: &str, suffix: &str) -> Result<Url, ApiError> {
        let mut url = self.session_resource_url(session_id)?;
        url.path_segments_mut()
            .map_err(|()| {
                ApiError::InvalidUrl("Server URL cannot contain path segments.".to_owned())
            })?
            .push(suffix);
        Ok(url)
    }

    fn session_resource_url(&self, session_id: &str) -> Result<Url, ApiError> {
        let mut url = self.resource_url("raep/v1/sessions")?;
        url.path_segments_mut()
            .map_err(|()| {
                ApiError::InvalidUrl("Server URL cannot contain path segments.".to_owned())
            })?
            .push(session_id);
        Ok(url)
    }
}

fn decode_payload<T: DeserializeOwned>(payload: &Value) -> Result<T, ApiError> {
    serde_json::from_value(payload.clone())
        .map_err(|error| ApiError::InvalidResponse(error.to_string()))
}

pub(crate) fn now_millis() -> Result<i64, ApiError> {
    #[cfg(target_arch = "wasm32")]
    {
        let timestamp = js_sys::Date::now();
        if !timestamp.is_finite() {
            return Err(ApiError::InvalidResponse(
                "The browser clock returned an invalid timestamp.".to_owned(),
            ));
        }
        timestamp.round().to_string().parse().map_err(|error| {
            ApiError::InvalidResponse(format!("Could not read the browser clock: {error}"))
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))
            .and_then(|duration| {
                i64::try_from(duration.as_millis())
                    .map_err(|error| ApiError::InvalidResponse(error.to_string()))
            })
    }
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
