//! One of the service's domains, as `impl Service` blocks.
//!
//! Child modules see the service's private fields, so a domain owns its
//! behaviour here while the state stays in one place.

use mcp::McpServers;

#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    /// Connects the configured MCP servers, once, at startup.
    pub(super) async fn connect_mcp(&self) {
        let root = self.workspace_root.clone();
        // The servers are read before the first await: a `std` lock must not be
        // held across one.
        let configured = self.lock_config().mcp_servers.clone();
        let connected = McpServers::connect(&configured, &root).await;
        for (name, reason) in &connected.failures {
            tracing::warn!(server = %name, %reason, "an MCP server could not be started");
        }
        if let Ok(mut slot) = self.mcp_servers.write() {
            *slot = connected;
        }
    }
    /// The MCP servers that connected, for the startup line.
    ///
    /// Each one carries the names of the tools it published, because a count
    /// says the server is up and the names are what a person needs to ask the
    /// model for one.
    pub fn mcp_status(&self) -> Vec<McpServerStatus> {
        self.mcp_servers
            .read()
            .map(|servers| {
                servers
                    .servers
                    .iter()
                    .map(|server| McpServerStatus {
                        name: server.name.clone(),
                        command: server.label(),
                        tools: server.tool_names(),
                        active: server.is_active(),
                        filtered: server.filtered.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Turns one MCP server off or on for the rest of the process.
    ///
    /// The process stays up, so turning it back on is instant; what changes is
    /// whether its tools are put in front of the model.
    ///
    /// Returns the state the server is in afterwards, or `None` when no server
    /// by that name connected.
    pub fn set_mcp_active(&self, name: &str, active: bool) -> Option<bool> {
        let mut servers = self.mcp_servers.write().ok()?;
        servers.set_active(name, active)
    }
    /// The MCP servers that could not be started, for the startup line.
    pub fn mcp_failures(&self) -> Vec<(String, String)> {
        self.mcp_servers
            .read()
            .map(|servers| servers.failures.clone())
            .unwrap_or_default()
    }
    /// Retries the MCP servers that failed, returning how many answered.
    ///
    /// The ones that are connected are left alone: they keep their process and
    /// their session.
    pub async fn retry_mcp(&self) -> usize {
        let root = self.workspace_root.clone();
        // A `std` lock must not be held across an await, so the config is read
        // and the retry runs without one; the lock is taken only to install the
        // result.
        let configured = self.lock_config().mcp_servers.clone();
        let pending = match self.mcp_servers.read() {
            Ok(servers) => servers.failed_to_retry(&configured),
            Err(_) => return 0,
        };
        let mut recovered = Vec::new();
        let mut still_failed = Vec::new();
        for (name, config) in pending {
            match McpServers::connect_configured(&name, &config, &root).await {
                Ok(server) => recovered.push(server),
                Err(reason) => still_failed.push((name, reason)),
            }
        }
        let count = recovered.len();
        if let Ok(mut servers) = self.mcp_servers.write() {
            servers.install(recovered, still_failed);
        }
        count
    }
    /// Rebuilds the MCP connections from the current configuration.
    ///
    /// A server added, removed or disabled in the config takes effect here: the
    /// connections are made from the configuration in force, not the one the
    /// process started with. The on/off state a person set in the panel is
    /// carried across by name, so a server they turned off is not turned back on
    /// by a refresh, and a process that is no longer configured is dropped with
    /// its children.
    ///
    /// Returns how many servers are up afterwards.
    pub async fn reload_mcp(&self) -> usize {
        let root = self.workspace_root.clone();
        let configured = self.lock_config().mcp_servers.clone();
        let was_active = self
            .mcp_servers
            .read()
            .map(|servers| servers.active_by_name())
            .unwrap_or_default();
        let mut connected = McpServers::connect(&configured, &root).await;
        for (name, reason) in &connected.failures {
            tracing::warn!(server = %name, %reason, "an MCP server could not be started");
        }
        // The state a person set wins over the default the connection has: a
        // server carried across the reload keeps whether it was on.
        for server in &mut connected.servers {
            if let Some(active) = was_active.get(&server.name) {
                server.active = *active;
            }
        }
        let up = connected.servers.len();
        if let Ok(mut slot) = self.mcp_servers.write() {
            *slot = connected;
        }
        up
    }
}
