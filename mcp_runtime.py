"""Let a server be turned off and on while the agent runs.

A server is connected once at startup, so switching one off is not a matter of
stopping the process: it is a matter of keeping its tools out of the registry.
The process stays up, which is what makes turning it back on instant.

A server that was configured but is switched off here is remembered, so the
switch survives the turn; what does not survive is the process ending.
"""

import pathlib

path = pathlib.Path("norte-code-agent/src/service/mcp.rs")
text = path.read_text()


def sub(old, new, why, count=1):
    global text
    found = text.count(old)
    assert found == count, (why, found, count)
    text = text.replace(old, new)


sub(
    """    /// Names the configuration asked for that the server does not publish.
    pub(super) filtered: Vec<String>,
}""",
    """    /// Names the configuration asked for that the server does not publish.
    pub(super) filtered: Vec<String>,
    /// Whether its tools reach the model right now.
    pub(super) active: bool,
}

impl McpServer {
    /// Whether the server is on.
    pub(super) fn is_active(&self) -> bool {
        self.active
    }
}""",
    "active",
)

sub(
    """            command: config.command.clone(),
            tools,
            filtered,
        })""",
    """            command: config.command.clone(),
            tools,
            filtered,
            active: true,
        })""",
    "init",
)

# The tools only reach the registry when the server is on.
sub(
    """    /// Every tool of every connected server, in order.
    pub(super) fn tools(&self) -> Vec<Arc<dyn AnyFunction>> {
        self.servers
            .iter()
            .flat_map(|server| server.tools.iter().cloned())
            .collect()
    }""",
    """    /// Every tool of every server that is on, in order.
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
        let server = self
            .servers
            .iter_mut()
            .find(|server| server.name == name)?;
        server.active = active;
        Some(active)
    }""",
    "tools and switch",
)

path.write_text(text)
print("ok")
