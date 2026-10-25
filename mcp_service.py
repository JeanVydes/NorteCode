"""Expose the on/off switch and the filtered names through the service."""

import pathlib

path = pathlib.Path("norte-code-agent/src/service/mod.rs")
text = path.read_text()


def sub(old, new, why, count=1):
    global text
    found = text.count(old)
    assert found == count, (why, found, count)
    text = text.replace(old, new)


sub(
    """                    .map(|server| McpServerStatus {
                        name: server.name.clone(),
                        command: server.label(),
                        tools: server.tool_names(),
                    })""",
    """                    .map(|server| McpServerStatus {
                        name: server.name.clone(),
                        command: server.label(),
                        tools: server.tool_names(),
                        active: server.is_active(),
                        filtered: server.filtered.clone(),
                    })""",
    "status",
)

sub(
    """    /// The tools it published, by the name the model sees.
    pub tools: Vec<String>,
}""",
    """    /// The tools it published, by the name the model sees.
    pub tools: Vec<String>,
    /// Whether its tools reach the model right now.
    pub active: bool,
    /// Names the configuration asked for that the server does not publish.
    pub filtered: Vec<String>,
}""",
    "type",
)

# The switch, next to the status reader.
sub(
    """    /// The MCP servers that could not be started, for the startup line.""",
    """    /// Turns one MCP server off or on for the rest of the process.
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

    /// The MCP servers that could not be started, for the startup line.""",
    "switch",
)

path.write_text(text)
print("ok")
