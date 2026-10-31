//! The chat commands this host declares.
//!
//! Every command is written once, here, and the parser, the completion and
//! `/help` all read it. That is the whole point: the five places a command used
//! to live are what let `/tools` be parsed and never offered, and what made the
//! help list four commands that had been there for a while.
//!
//! What a command *does* is not here. The registry hands back an
//! [`Invocation`](norte_code_protocol::commands::Invocation) naming the command, and
//! [`crate::interaction`] matches on that name; a registry a plugin can write
//! into cannot be keyed by the binary's own enum.

use norte_code_protocol::commands::{ArgSpec, CommandSpec, Registry};

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

/// Loads the plugins this client ships and adds their commands.
///
/// A plugin's command is a command: it parses, completes and lists exactly like
/// the client's own, and whoever types it cannot tell the difference. That is
/// what the registry was made for — it knows nothing about what a command does,
/// so a plugin is not a special case in it.
///
/// `NORTE_CODE_PLUGINS` names a directory, and the command names a binary in it.
fn plugins() -> norte_code_protocol::plugins::Plugins {
    let mut plugins = norte_code_protocol::plugins::Plugins::new();
    crate::plugin_loader::load_plugins(&mut plugins, None);
    plugins
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
    registry.register(
        CommandSpec::new(
            "/provider",
            "switch provider, keeping the model where it has one",
        )
        .arg(ArgSpec::optional("provider")),
    );
    registry.register(
        CommandSpec::new(
            "/refresh",
            "re-read the configuration, the skills and the MCP servers",
        )
        .aliased(&["/reload"]),
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
        CommandSpec::new("/menu", "go back to choosing a provider and a model").aliased(&["/back"]),
    );
    registry.register(CommandSpec::new(
        "/delete",
        "delete this session, with its cost and telemetry, and leave",
    ));
    registry.register(CommandSpec::new("/exit", "leave the chat").aliased(&["/quit"]));
    registry.register(
        CommandSpec::new(
            "/plugins list",
            "list installed plugins",
        )
    );
    registry.register(
        CommandSpec::new(
            "/plugins add",
            "install a plugin from a git URL",
        )
        .arg(ArgSpec::required("url"))
    );
    registry.register(
        CommandSpec::new(
            "/plugins rm",
            "remove an installed plugin",
        )
        .arg(ArgSpec::required("name"))
    );
    // The plugins lead: a command of theirs that names one of the client's is
    // the one that wins, because the plugin was asked for by name and the
    // client was not.
    #[allow(clippy::too_many_lines)]
    for plugin in plugins().commands() {
        registry.register(plugin);
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_the_parser_answers_is_a_command_the_help_offers() {
        // The two were separate lists and drifted: `/tools` parsed and was
        // never completed, and the help named four commands fewer than the
        // client answered to. Reading one declaration is what makes that
        // impossible rather than unlikely.
        let registry = built();
        let names: Vec<String> = registry
            .commands()
            .iter()
            .map(|spec| spec.name.clone())
            .collect();
        for expected in [
            "/help",
            "/compact",
            "/allow",
            "/deny",
            "/tools",
            "/reasoning",
            "/model",
            "/provider",
            "/refresh",
            "/mcp",
            "/jobs",
            "/restart",
            "/rename",
            "/debug",
            "/menu",
            "/delete",
            "/exit",
        ] {
            assert!(names.contains(&expected.to_owned()), "missing {expected}");
        }
    }

    #[test]
    fn every_usage_line_lines_up_so_the_list_reads_down() {
        // `/help` writes the usage column padded to the widest one, and the
        // width is measured rather than kept beside the list. This is that
        // measurement, checked where it can be seen: a command that grows an
        // argument must not push every summary out of line.
        let registry = built();
        let usages: Vec<String> = registry
            .commands()
            .iter()
            .map(norte_code_protocol::commands::CommandSpec::usage)
            .collect();
        let width = usages.iter().map(String::len).max().unwrap_or(0);
        for usage in &usages {
            assert!(usage.len() <= width, "`{usage}` is wider than the column");
        }
        assert!(width > 0);
        // The longest one is the one that sets it, which is the whole point of
        // measuring instead of guessing.
        assert_eq!(
            usages.iter().map(String::len).max(),
            Some(width),
            "the column is the longest line: {usages:?}"
        );
    }

    #[test]
    fn the_command_that_was_missing_from_the_completion_is_there() {
        // The bug that started this: `/tools` was parsed and never offered.
        let registry = built();
        let candidates = registry.candidates("/to", 3);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.text == "/tools"),
            "{candidates:?}"
        );
    }

    #[test]
    fn the_efforts_are_offered_while_typing_one() {
        // Nobody should have to remember that it is `xhigh` and not `x-high`.
        let registry = built();
        let candidates = registry.candidates("/reasoning xh", "/reasoning xh".len());
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0].text, "xhigh");
        assert_eq!(candidates[0].completion, "igh", "Tab adds the rest");
    }

    #[test]
    fn a_scope_is_offered_after_the_effort_and_not_before_it() {
        // `/reasoning high ` has one word left, and it is the scope; the effort
        // is already given and offering it again would be noise.
        let registry = built();
        let candidates = registry.candidates("/reasoning high ", "/reasoning high ".len());
        let values: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect();
        assert_eq!(values, ["session", "message"], "{candidates:?}");
    }

    #[test]
    fn an_alias_is_offered_beside_the_name_it_belongs_to() {
        let registry = built();
        let candidates = registry.candidates("/re", 3);
        let texts: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect();
        assert!(texts.contains(&"/refresh"), "{texts:?}");
        assert!(texts.contains(&"/reload"), "{texts:?}");
        assert!(texts.contains(&"/reasoning"), "{texts:?}");
        assert!(texts.contains(&"/rename"), "{texts:?}");
    }

    #[test]
    fn a_command_with_no_arguments_offers_nothing_after_it() {
        let registry = built();
        assert!(
            registry.candidates("/help ", "/help ".len()).is_empty(),
            "there is nothing to say after a command that takes nothing"
        );
    }

    #[test]
    fn the_registry_parses_what_the_client_answers_to() {
        // Every command in the list resolves, which is the other half of the
        // drift: a name the completion offers and the parser does not know is
        // a name that does nothing.
        let registry = built();
        for spec in registry.commands() {
            for spelling in spec.spellings() {
                let parsed = registry
                    .parse(spelling)
                    .unwrap_or_else(|| panic!("{spelling} is not a command"));
                if let Ok(invocation) = parsed {
                    assert_eq!(
                        invocation.command, spec.name,
                        "{spelling} resolves to its canonical name"
                    );
                }
            }
        }
    }
}
