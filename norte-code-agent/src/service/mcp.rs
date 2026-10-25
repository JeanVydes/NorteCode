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
use norte_mcp::{
    ClientCapabilities, ClientOptions, Implementation, McpClient, RemoteTool, StdioTransport,
};

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
    /// The tools the server exposed and the filter kept, as host tools.
    pub(super) tools: Vec<Arc<dyn AnyFunction>>,
    /// Names the configuration asked for that the server does not publish.
    pub(super) filtered: Vec<String>,
    /// Whether its tools reach the model right now.
    pub(super) active: bool,
}

impl McpServer {
    /// Whether the server is on.
    pub(super) fn is_active(&self) -> bool {
        self.active
    }
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

    /// The failures worth retrying, with their configuration.
    ///
    /// Only the failures are named: a server that is up keeps its process and
    /// its session, so a retry cannot disturb it.
    pub(super) fn failed_to_retry(
        &self,
        configured: &BTreeMap<String, McpServerConfig>,
    ) -> Vec<(String, McpServerConfig)> {
        self.failures
            .iter()
            .filter_map(|(name, _)| {
                configured
                    .get(name)
                    .filter(|config| config.enabled)
                    .map(|config| (name.clone(), config.clone()))
            })
            .collect()
    }

    /// Installs the result of a retry: what came up, and what did not.
    pub(super) fn install(
        &mut self,
        recovered: Vec<McpServer>,
        still_failed: Vec<(String, String)>,
    ) {
        self.servers.extend(recovered);
        self.failures = still_failed;
    }

    /// Connects one named server, for a retry that must not hold the set.
    ///
    /// # Errors
    ///
    /// Returns the reason it could not be reached, as the startup path does.
    pub(super) async fn connect_configured(
        name: &str,
        config: &McpServerConfig,
        root: &Path,
    ) -> Result<McpServer, String> {
        Self::connect_one(name, config, root).await
    }

    async fn connect_one(
        name: &str,
        config: &McpServerConfig,
        root: &Path,
    ) -> Result<McpServer, String> {
        let transport = transport_for(config, root)?;
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

        let published = client
            .list_tools()
            .await
            .map_err(|error| format!("could not list its tools: {error}"))?;
        let names: Vec<&str> = published.iter().map(|tool| tool.name.as_str()).collect();
        let kept: Vec<&str> = config.select(names.iter()).into_iter().copied().collect();
        // A name in `allow` the server does not publish is a typo, and a silent
        // one: the tool the host asked for is simply absent.
        let mut filtered = Vec::new();
        for wanted in &config.allow {
            if !names.iter().any(|name| name == wanted) {
                filtered.push(format!("`{wanted}` is not a tool of this server"));
            }
        }
        let tools = published
            .iter()
            .filter(|tool| kept.contains(&tool.name.as_str()))
            .map(|definition| {
                RemoteTool::new(Arc::clone(&client), name, definition.clone())
                    .map(|tool| Arc::new(tool) as Arc<dyn AnyFunction>)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not wrap its tools: {error}"))?;

        Ok(McpServer {
            name: name.to_owned(),
            command: config.target(),
            tools,
            filtered,
            active: true,
        })
    }

    /// Every tool of every server that is on, in order.
    pub(super) fn tools(&self) -> Vec<Arc<dyn AnyFunction>> {
        self.servers
            .iter()
            .filter(|server| server.active)
            .flat_map(|server| server.tools.iter().cloned())
            .collect()
    }

    /// Turns one server off or on.
    ///
    /// Returns the state it is in afterwards, or `None` when no such server
    /// connected: a name that was never up is a mistake worth reporting, and
    /// `Some` for a server already in the asked state keeps the command
    /// idempotent.
    pub(super) fn set_active(&mut self, name: &str, active: bool) -> Option<bool> {
        let server = self.servers.iter_mut().find(|server| server.name == name)?;
        server.active = active;
        Some(active)
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

/// The transport an entry names: a remote endpoint, or a program to run.
///
/// A `url` needs the HTTP transport, which lives in a Norte revision that is not
/// on the remote yet; until the dependency can move, a remote entry is reported
/// as unreachable rather than silently ignored.
fn transport_for(config: &McpServerConfig, root: &Path) -> Result<StdioTransport, String> {
    if let Some(url) = config.url.as_ref().filter(|url| !url.trim().is_empty()) {
        return Err(format!(
            "`{url}` is remote, and this build speaks only to a local server"
        ));
    }
    StdioTransport::from_command(command_for(config, root))
        .map_err(|error| format!("could not start `{}`: {error}", config.command))
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
