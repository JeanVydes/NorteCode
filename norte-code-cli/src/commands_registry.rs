//! The chat commands this host declares.
//!
//! Every command is written once, here, and the parser, the completion and
//! `/help` all read it. That is the whole point: the five places a command used
//! to live are what let `/tools` be parsed and never offered, and what made the
//! help list four commands that had been there for a while.
//!
//! What a command *does* is not here. The registry hands back an
//! [`Invocation`](norte_code_plugin_host::commands::Invocation) naming the command, and
//! [`crate::interaction`] matches on that name; a registry a plugin can write
//! into cannot be keyed by the binary's own enum.

use norte_code_plugin_host::commands::{ArgSpec, CommandSpec, Registry};

/// The efforts a model may declare, offered while typing `/reasoning`.
///
/// The list is a suggestion and not a rule: which efforts exist depends on the
/// model in play, and the host refuses an unknown one with the model's own
/// answer. It is here so nobody has to remember the spelling of `xhigh`.
pub(crate) const REASONING_EFFORTS: &[&str] = &[
    "auto", "off", "minimal", "low", "medium", "high", "xhigh", "max",
];

/// The scopes every command that takes one accepts.
pub(crate) const SCOPES: &[&str] = &["session", "message"];

/// The two scopes an approval rule takes, and what they mean.
pub(crate) const TOOL_SCOPES: &[&str] = &["session", "global"];

/// The actions `/jobs` takes.
pub(crate) const JOB_ACTIONS: &[&str] = &["list", "check", "stop"];

/// The actions `/connection` takes.
pub(crate) const CONNECTION_ACTIONS: &[&str] = &["list", "add", "remove"];

/// The actions `/mcp` takes.
pub(crate) const MCP_ACTIONS: &[&str] = &["list", "add", "enable", "disable", "remove"];

/// The tabs `/tab` can show, as the footer names them.
/// The registry every part of the client reads.
///
/// It is built once, with the plugins already in it, and handed out by
/// reference. The alternative — every caller building its own — is nine places
/// that would each have to remember the plugins, which is nine places for a
/// command to be parsed but not completed.
static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();

/// The registry, built on first use.
///
/// The first caller builds it, and everything after that shares it. A plugin
/// registered after this has been read is not in it: plugins load at startup,
/// before the first command is typed, and a registry that changed under a frame
/// would need a lock in the reading path.
#[must_use]
pub(crate) fn shared() -> &'static Registry {
    REGISTRY.get_or_init(built)
}

/// The plugins this client loaded, and the commands they add.
///
/// A plugin's command is a command: it parses, completes and lists exactly like
/// the client's own, and whoever types it cannot tell the difference. That is
/// what the registry was made for — it knows nothing about what a command does,
/// so a plugin is not a special case in it.
///
/// The commands come from the registry the client loaded at startup, not from
/// the directory: a plugin installed while the client runs is not one the
/// client can use, and a registry that offered a command nothing answered
/// would be a lie.
fn plugin_commands() -> Vec<norte_code_plugin_host::commands::CommandSpec> {
    crate::plugins::loaded().commands()
}

/// Every command the client answers to, in the order `/help` lists them.
#[must_use]
#[allow(clippy::too_many_lines)]
pub(crate) fn built() -> Registry {
    let mut registry = Registry::new();
    registry.register(CommandSpec::new("/help", "show this list"));
    registry.register(CommandSpec::new(
        "/compact",
        "summarise the session and continue on a new one",
    ));
    registry.register(
        CommandSpec::new("/allow", "allow a tool without asking")
            .arg(ArgSpec::required("tool"))
            .arg(ArgSpec::optional_choice("scope", TOOL_SCOPES)),
    );
    registry.register(
        CommandSpec::new("/deny", "refuse a tool without asking")
            .arg(ArgSpec::required("tool"))
            .arg(ArgSpec::optional_choice("scope", TOOL_SCOPES)),
    );
    registry.register(CommandSpec::new(
        "/tools",
        "every tool the model can reach, and how a call to it is decided",
    ));
    registry.register(
        CommandSpec::new("/reasoning", "set the reasoning override")
            .arg(ArgSpec::open_with(
                "setting",
                &[REASONING_EFFORTS, &["budget:<tokens>"]].concat(),
            ))
            .arg(ArgSpec::optional_choice("scope", SCOPES)),
    );
    registry.register(
        CommandSpec::new("/model", "switch the model at the next turn boundary")
            .arg(ArgSpec::required("model")),
    );
    registry.register(CommandSpec::new(
        "/models",
        "choose a provider and a model in a panel",
    ));
    registry.register(CommandSpec::new(
        "/sessions",
        "open another session, or start a new one",
    ));
    registry.register(CommandSpec::new(
        "/plans",
        "show this session's saved plans, status and file paths",
    ));
    registry.register(
        CommandSpec::new(
            "/provider",
            "switch provider, keeping the model where it has one",
        )
        .arg(ArgSpec::optional("provider")),
    );
    registry.register(
        CommandSpec::new("/refresh", "re-read the client cli.json").aliased(&["/reload"]),
    );
    registry.register(
        CommandSpec::new("/mcp", "list, add, remove or switch an MCP server")
            .arg(ArgSpec::optional_choice("action", MCP_ACTIONS))
            // The rest of the line, not one argument: `add` takes a name, a
            // command and its arguments, and how many there are is the person's
            // to decide.
            .arg(ArgSpec::rest("args")),
    );
    registry.register(
        CommandSpec::new(
            "/jobs",
            "list the session's background jobs, read one, or stop one",
        )
        .arg(ArgSpec::optional_choice("action", JOB_ACTIONS))
        .arg(ArgSpec::optional("job-id")),
    );
    registry.register(
        CommandSpec::new("/connection", "list, add or remove a saved provider key")
            .arg(ArgSpec::optional_choice("action", CONNECTION_ACTIONS))
            .arg(ArgSpec::optional("provider")),
    );
    registry.register(CommandSpec::new(
        "/restart",
        "re-read the configuration and rebuild the engine at the next boundary",
    ));
    registry.register(
        CommandSpec::new("/rename", "name the session; bare, ask the model for one")
            .arg(ArgSpec::rest("title")),
    );
    registry.register(CommandSpec::new(
        "/debug",
        "what the process knows: model, tools, paths, servers",
    ));
    registry.register(CommandSpec::new(
        "/telemetry",
        "the request ledger: this session, and every provider",
    ));
    registry.register(
        CommandSpec::new("/export", "export this session to a file").arg(ArgSpec::optional("path")),
    );
    registry.register(
        CommandSpec::new("/import", "import a session from a file").arg(ArgSpec::required("path")),
    );
    registry.register(
        CommandSpec::new("/menu", "go back to choosing a provider and a model").aliased(&["/back"]),
    );
    registry.register(CommandSpec::new(
        "/delete",
        "delete this session, with its cost and telemetry, and leave",
    ));
    registry.register(CommandSpec::new("/exit", "leave the chat").aliased(&["/quit"]));
    registry.register(CommandSpec::new("/plugins list", "list installed plugins"));
    registry.register(
        CommandSpec::new(
            "/plugins add",
            "install a plugin from a git URL or workspace",
        )
        .arg(ArgSpec::required("url"))
        .arg(ArgSpec::optional("package"))
        .arg(ArgSpec::optional_choice("all", &["--all"])),
    );
    registry.register(
        CommandSpec::new("/plugins rm", "remove an installed plugin")
            .arg(ArgSpec::required("name")),
    );
    registry.register(
        CommandSpec::new(
            "/plugins update",
            "rebuild installed plugins from their source, all or one",
        )
        .arg(ArgSpec::optional("name")),
    );
    registry.register(
        CommandSpec::new("/plugins enable", "load an installed plugin again")
            .arg(ArgSpec::required("name")),
    );
    registry.register(
        CommandSpec::new("/plugins disable", "stop loading an installed plugin")
            .arg(ArgSpec::required("name")),
    );
    // The plugins lead: a command of theirs that names one of the client's is
    // the one that wins, because the plugin was asked for by name and the
    // client was not.
    #[allow(clippy::too_many_lines)]
    for plugin in plugin_commands() {
        registry.register(plugin);
    }
    registry
}

#[cfg(test)]
#[path = "commands_registry_tests.rs"]
mod tests;
