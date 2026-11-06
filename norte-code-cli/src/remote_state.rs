//! Client-side state backed by the RAEP server.

#![allow(
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::unused_self,
    clippy::unnecessary_wraps,
    dead_code
)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use futures_util::StreamExt;
use norte_code_client::RemoteClient;
use norte_code_client::{AgentClient, ClientError, EventStream};
use norte_code_protocol::ClientRequest;
use norte_code_protocol::ClientRequestKind;
use norte_code_protocol::config::{CliConfig, Config, project_cli_path, user_cli_path};
use norte_code_protocol::plugins::ContextUsage;
use norte_code_protocol::sqlite::RequestRecord;
use norte_code_protocol::{
    McpFailure, McpInfo, ModelInfo, Reasoning, ServerEventKind, SessionKey, ToolInfo,
};

/// The state the terminal keeps while it talks to the remote host.
///
/// It contains presentation configuration and a catalogue snapshot only. Agent
/// execution, sessions, tools and persistence remain exclusively on the RAEP
/// server.
pub(crate) struct RemoteState {
    client: Arc<RemoteClient>,
    models: Arc<Vec<ModelInfo>>,
    config: RwLock<Config>,
    config_path: Option<PathBuf>,
}

impl RemoteState {
    pub(crate) fn new(
        client: Arc<RemoteClient>,
        models: Vec<ModelInfo>,
        config: Config,
        config_path: Option<PathBuf>,
    ) -> Self {
        Self {
            client,
            models: Arc::new(models),
            config: RwLock::new(config),
            config_path,
        }
    }

    pub(crate) fn client(&self) -> &Arc<RemoteClient> {
        &self.client
    }

    pub(crate) async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError> {
        self.client.submit(request).await
    }

    pub(crate) async fn telemetry(
        &self,
        session: Option<SessionKey>,
    ) -> Result<Vec<RequestRecord>, String> {
        let mut stream = self
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::ListTelemetry { session },
            })
            .await
            .map_err(|error| error.to_string())?;
        while let Some(event) = stream.next().await {
            match event.kind {
                ServerEventKind::Telemetry { records } => return Ok(records),
                ServerEventKind::Error { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("the service did not return telemetry".to_owned())
    }

    pub(crate) async fn tool_rules_remote(
        &self,
        session: Option<SessionKey>,
    ) -> Result<Vec<ToolInfo>, String> {
        let mut stream = self
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::ListTools { session },
            })
            .await
            .map_err(|error| error.to_string())?;
        while let Some(event) = stream.next().await {
            match event.kind {
                ServerEventKind::ToolList { tools } => return Ok(tools),
                ServerEventKind::Error { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("the service did not return tool rules".to_owned())
    }

    pub(crate) async fn set_tool_rule(
        &self,
        session: Option<SessionKey>,
        tool: String,
        allow: bool,
        global: bool,
    ) -> Result<Vec<ToolInfo>, String> {
        let mut stream = self
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::SetApprovalRule {
                    session,
                    tool,
                    allow,
                    global,
                },
            })
            .await
            .map_err(|error| error.to_string())?;
        while let Some(event) = stream.next().await {
            match event.kind {
                ServerEventKind::ToolList { tools } => return Ok(tools),
                ServerEventKind::Error { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("the service did not return updated tool rules".to_owned())
    }

    pub(crate) async fn forget_tool_rule(
        &self,
        session: Option<SessionKey>,
        tool: String,
        global: bool,
    ) -> Result<Vec<ToolInfo>, String> {
        let mut stream = self
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::ForgetApprovalRule {
                    session,
                    tool,
                    global,
                },
            })
            .await
            .map_err(|error| error.to_string())?;
        while let Some(event) = stream.next().await {
            match event.kind {
                ServerEventKind::ToolList { tools } => return Ok(tools),
                ServerEventKind::Error { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("the service did not return updated tool rules".to_owned())
    }

    pub(crate) async fn mcp_status_remote(
        &self,
    ) -> Result<(Vec<McpInfo>, Vec<McpFailure>), String> {
        let mut stream = self
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::ListMcp,
            })
            .await
            .map_err(|error| error.to_string())?;
        while let Some(event) = stream.next().await {
            match event.kind {
                ServerEventKind::McpList { servers, failures } => {
                    return Ok((servers, failures));
                }
                ServerEventKind::Error { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("the service did not return MCP status".to_owned())
    }

    pub(crate) async fn refresh_mcp_remote(
        &self,
    ) -> Result<(Vec<McpInfo>, Vec<McpFailure>), String> {
        let mut stream = self
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::RefreshMcp,
            })
            .await
            .map_err(|error| error.to_string())?;
        while let Some(event) = stream.next().await {
            match event.kind {
                ServerEventKind::McpList { servers, failures } => {
                    return Ok((servers, failures));
                }
                ServerEventKind::Error { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("the service did not return refreshed MCP status".to_owned())
    }

    pub(crate) async fn set_mcp_active_remote(
        &self,
        name: String,
        active: bool,
    ) -> Result<(Vec<McpInfo>, Vec<McpFailure>), String> {
        let mut stream = self
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::SetMcpActive { name, active },
            })
            .await
            .map_err(|error| error.to_string())?;
        while let Some(event) = stream.next().await {
            match event.kind {
                ServerEventKind::McpList { servers, failures } => {
                    return Ok((servers, failures));
                }
                ServerEventKind::Error { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("the service did not return updated MCP status".to_owned())
    }

    pub(crate) fn models(&self) -> Arc<Vec<ModelInfo>> {
        Arc::clone(&self.models)
    }

    pub(crate) fn model_count(&self) -> usize {
        self.models.len()
    }

    pub(crate) fn providers(&self) -> Vec<String> {
        let mut providers: Vec<_> = self
            .models
            .iter()
            .map(|model| model.provider.clone())
            .collect();
        providers.sort();
        providers.dedup();
        providers
    }

    pub(crate) fn models_of(&self, provider: &str) -> Vec<ModelInfo> {
        let mut models: Vec<_> = self
            .models
            .iter()
            .filter(|model| model.provider == provider)
            .cloned()
            .collect();
        models.sort_by(|a, b| a.model.cmp(&b.model));
        models
    }

    pub(crate) fn config(&self) -> Config {
        self.config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    pub(crate) fn default_spec(&self) -> Option<ModelInfo> {
        let config = self.config();
        self.model(
            config.default_provider.as_deref()?,
            config.default_model.as_deref()?,
        )
    }

    pub(crate) fn session_spec(&self, session_key: &SessionKey) -> Option<ModelInfo> {
        let session = self.client.session_info(session_key.as_str());
        match session.and_then(|info| info.provider.zip(info.model)) {
            Some((provider, model)) => self.model(&provider, &model),
            None => self.default_spec(),
        }
    }

    pub(crate) fn session_reasoning(&self, session: &SessionKey) -> Option<Reasoning> {
        self.client.session_reasoning(session.as_str())
    }

    pub(crate) fn session_title(&self, session_key: &SessionKey) -> Option<String> {
        self.client
            .session_info(session_key.as_str())
            .and_then(|info| info.title)
    }

    pub(crate) fn session_model(&self, session: &SessionKey) -> Option<String> {
        self.session_spec(session).map(|model| model.model)
    }

    pub(crate) fn session_context(&self, session: &SessionKey) -> ContextUsage {
        self.client
            .session_stats(session.as_str())
            .map(|stats| ContextUsage {
                used: stats.context_used,
                window: stats.context_window,
            })
            .unwrap_or_default()
    }

    pub(crate) fn has_running_jobs(&self, session: &SessionKey) -> bool {
        self.client
            .session_jobs(session.as_str())
            .iter()
            .any(|job| !job.state.is_done())
    }

    pub(crate) fn model(&self, provider: &str, model: &str) -> Option<ModelInfo> {
        self.models
            .iter()
            .find(|entry| entry.provider == provider && entry.model == model)
            .cloned()
    }

    pub(crate) fn reload_config(&self) -> Result<ReloadedConfig, String> {
        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        let user = self.config_path.clone().or_else(user_cli_path);
        let loaded = CliConfig::load(user.as_deref(), Some(&project_cli_path(&cwd)))
            .map_err(|error| error.to_string())?;
        let mut config = self.config();
        config.ui = loaded.config;
        *self
            .config
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = config.clone();
        Ok(ReloadedConfig {
            config,
            warnings: loaded.warnings,
        })
    }

    pub(crate) async fn retitle_session(&self, _session: &SessionKey) -> Result<String, String> {
        Err("session title changes are handled by the RAEP host".to_owned())
    }

    pub(crate) async fn set_approval_rule(
        &self,
        session: Option<&SessionKey>,
        tool: &str,
        allow: bool,
        global: bool,
    ) -> Result<(), String> {
        self.set_tool_rule(session.cloned(), tool.to_owned(), allow, global)
            .await
            .map(|_| ())
    }

    pub(crate) async fn forget_approval_rule(
        &self,
        session: Option<&SessionKey>,
        tool: &str,
        global: bool,
    ) -> Result<(), String> {
        self.forget_tool_rule(session.cloned(), tool.to_owned(), global)
            .await
            .map(|_| ())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ReloadedConfig {
    pub(crate) config: Config,
    pub(crate) warnings: Vec<String>,
}

pub(crate) trait ModelInfoExt {
    fn label(&self) -> String;
}

impl ModelInfoExt for ModelInfo {
    fn label(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

pub(crate) fn effective_reasoning(
    requested: Option<&Reasoning>,
    configured: &serde_json::Value,
    _capabilities: &norte_code_protocol::ReasoningInfo,
) -> Result<Reasoning, String> {
    Ok(requested.cloned().unwrap_or_else(|| {
        let _ = configured;
        Reasoning::Automatic
    }))
}
