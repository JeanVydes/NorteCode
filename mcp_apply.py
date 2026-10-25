"""Apply the allow/deny lists when the server connects.

Filtering the definitions before wrapping them is what keeps a denied tool out
of the prompt: the wrapper is what the model sees, so a tool that is never
wrapped is never paid for.
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
    """        let tools = client
            .tools(name)
            .await
            .map_err(|error| format!("could not list its tools: {error}"))?;
        Ok(McpServer {
            name: name.to_owned(),
            command: config.command.clone(),
            tools,
        })
    }""",
    """        let published = client
            .list_tools()
            .await
            .map_err(|error| format!("could not list its tools: {error}"))?;
        let names: Vec<&str> = published.iter().map(|tool| tool.name.as_str()).collect();
        let kept: Vec<&str> = config.select(names.iter());
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
            command: config.command.clone(),
            tools,
            filtered,
        })
    }""",
    "filter",
)

# The server carries the warnings the filter found.
sub(
    """    /// The tools the server exposed, wrapped as host tools.
    pub(super) tools: Vec<Arc<dyn AnyFunction>>,
}""",
    """    /// The tools the server exposed and the filter kept, as host tools.
    pub(super) tools: Vec<Arc<dyn AnyFunction>>,
    /// Names the configuration asked for that the server does not publish.
    pub(super) filtered: Vec<String>,
}""",
    "field",
)

sub(
    "use norte_mcp::{ClientCapabilities, ClientOptions, Implementation, McpClient, StdioTransport};",
    "use norte_mcp::{\n    ClientCapabilities, ClientOptions, Implementation, McpClient, RemoteTool, StdioTransport,\n};",
    "import",
)

path.write_text(text)
print("ok")
