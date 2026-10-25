//! MCP servers: connect at startup and keep their tools for every turn.
//!
//! A server is a child process speaking the Model Context Protocol over its
//! standard streams. It is started once, when the service is built, because a
//! process is not something to spawn per turn; what it publishes is then
//! registered in the tool registry the engine reads on every request.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use norte::AnyFunction;
use norte_code_config::McpServerConfig;
use norte_mcp::{ClientCapabilities, ClientOptions, Implementation, McpClient, StdioTransport};

/// A deadline for one MCP request.
///
/// A server that hangs must not hang the turn: the same rule the rest of the
/// host follows for anything it does not control.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// One connected server, with the tools it published.
#[derive(Debug)]
pub(super) struct McpServer {
    /// The name the configuration gave it, which every tool name carries.
    pub(super) name: String,
    /// The program that was started, for the startup line.
    pub(super) command: String,
    /// The tools the server exposed, wrapped as host tools.
    pub(super) tools: Vec<Arc<dyn AnyFunction>>,
}

impl McpServer {
    /// The program that was started, as the startup line names it.
    pub(super) fn label(&self) -> String {
        self.command.clone()
    }

    /// The names of the tools this server published, as the model sees them.
    pub(super) fn tool_names(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect()
    }
}

/// Every server that connected, and the ones that did not.
#[derive(Debug)]
pub(super) struct McpServers {
    /// The servers that are up, in configuration order.
    pub(super) servers: Vec<McpServer>,
    /// A server that could not be reached, with the reason.
    pub(super) failures: Vec<(String, String)>,
}

impl McpServers {
    /// An empty set, for a service built without the asynchronous startup.
    pub(super) fn none() -> Self {
        Self {
            servers: Vec::new(),
            failures: Vec::new(),
        }
    }

    /// Connects every enabled server in `configured`.
    ///
    /// A server that fails to start is reported and skipped, never fatal: one
    /// broken entry must not stop the agent, which is what a person expects
    /// when they paste a configuration they have not tried yet.
    pub(super) async fn connect(
        configured: &BTreeMap<String, McpServerConfig>,
        root: &Path,
    ) -> Self {
        let mut servers = Vec::new();
        let mut failures = Vec::new();
        for (name, config) in configured {
            if !config.enabled {
                continue;
            }
            match Self::connect_one(name, config, root).await {
                Ok(server) => servers.push(server),
                Err(reason) => failures.push((name.clone(), reason)),
            }
        }
        Self { servers, failures }
    }

    async fn connect_one(
        name: &str,
        config: &McpServerConfig,
        root: &Path,
    ) -> Result<McpServer, String> {
        let transport = StdioTransport::from_command(command_for(config, root))
            .map_err(|error| format!("could not start `{}`: {error}", config.command))?;
        let client = Arc::new(
            McpClient::connect(
                Arc::new(transport),
                ClientOptions {
                    client_info: Implementation {
                        name: "norte-code".to_owned(),
                        version: env!("CARGO_PKG_VERSION").to_owned(),
                        title: Some("Norte Code".to_owned()),
                        extra: BTreeMap::new(),
                    },
                    capabilities: ClientCapabilities::default(),
                    request_timeout: Some(REQUEST_TIMEOUT),
                },
            )
            .await
            .map_err(|error| format!("handshake failed: {error}"))?,
        );

        let tools = client
            .tools(name)
            .await
            .map_err(|error| format!("could not list its tools: {error}"))?;
        Ok(McpServer {
            name: name.to_owned(),
            command: config.command.clone(),
            tools,
        })
    }

    /// Every tool of every connected server, in order.
    pub(super) fn tools(&self) -> Vec<Arc<dyn AnyFunction>> {
        self.servers
            .iter()
            .flat_map(|server| server.tools.iter().cloned())
            .collect()
    }

    /// The values every configured server names, for redaction.
    ///
    /// A server's `env` is where its token lives, so the values are registered
    /// like any other secret before the child is started.
    pub(super) fn secrets(configured: &BTreeMap<String, McpServerConfig>) -> Vec<String> {
        configured
            .values()
            .flat_map(|server| server.env.values().cloned())
            .filter(|value| !value.is_empty())
            .collect()
    }
}

/// The child command for one server.
///
/// The child gets the configured variables and the `PATH`, and nothing else the
/// host holds: a server is a program from elsewhere, and the ambient
/// environment is where a key would leak from.
fn command_for(config: &McpServerConfig, root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&config.command);
    command.args(&config.args).current_dir(root).env_clear();
    for (key, value) in &config.env {
        command.env(key, value);
    }
    if let Ok(path) = std::env::var("PATH") {
        command.env("PATH", path);
    }
    command
}
