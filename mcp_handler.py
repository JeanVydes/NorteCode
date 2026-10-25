"""Add the /mcp handlers where /reload is handled."""

import pathlib

path = pathlib.Path("norte-code-cli/src/interaction.rs")
text = path.read_text()


def sub(old, new, why, count=1):
    global text
    found = text.count(old)
    assert found == count, (why, found, count)
    text = text.replace(old, new)


HELPER = """/// Reports the MCP servers, or switches one off or on.
///
/// A server's tools ride in every prompt, so the count is worth seeing; a server
/// that is off keeps its process, which is what makes turning it back on
/// instant.
fn report_mcp(service: &Arc<Service>, action: McpAction, server: Option<&str>, style: Style) {
    if let Some(name) = server {
        let active = action == McpAction::Enable;
        match service.set_mcp_active(name, active) {
            Some(state) => {
                let word = if state { "enabled" } else { "disabled" };
                eprintln!("{}", style.green(format!("mcp server `{name}` {word}")));
                if !state {
                    eprintln!(
                        "{}",
                        style.dim("its tools leave the next turn; the process stays up")
                    );
                }
            }
            None => eprintln!(
                "{}",
                style.yellow(format!("no mcp server named `{name}` connected"))
            ),
        }
        return;
    }

    let status = service.mcp_status();
    if status.is_empty() {
        eprintln!("{}", style.dim("no MCP server connected"));
        return;
    }
    for server in &status {
        let mark = if server.active {
            style.green("on")
        } else {
            style.faint("off")
        };
        eprintln!(
            "{} {} {} {}",
            mark,
            style.bold(server.name.clone()),
            style.dim(server.command.clone()),
            style.dim(format!("{} tool(s)", server.tools.len()))
        );
        if server.active {
            for tool in &server.tools {
                eprintln!("  {}", style.dim(tool.clone()));
            }
        }
        for warning in &server.filtered {
            eprintln!("  {}", style.yellow(warning.clone()));
        }
    }
}

fn print_chat_help(style: Style) {"""

if "fn report_mcp(" not in text:
    sub("fn print_chat_help(style: Style) {", HELPER, "helper")

sub(
    """        ChatCommand::Reload => report_reload(service, style),""",
    """        ChatCommand::Reload => report_reload(service, style),
        ChatCommand::Mcp { action, server } => {
            report_mcp(service, action, server.as_deref(), style);
        }""",
    "idle",
)

sub(
    """                ChatCommand::Reload => report_reload(service, style),""",
    """                ChatCommand::Reload => report_reload(service, style),
                ChatCommand::Mcp { action, server } => {
                    report_mcp(service, action, server.as_deref(), style);
                }""",
    "turn",
)

sub(
    """    eprintln!("  /reload                   re-read the configuration");""",
    """    eprintln!("  /reload                   re-read the configuration");
    eprintln!("  /mcp [list|enable|disable] [server]   the MCP servers and their tools");""",
    "help",
)

path.write_text(text)
print("ok")
