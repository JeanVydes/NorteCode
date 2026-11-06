//! `/debug`: what the process knows about itself, laid out to be read.
//!
//! Plain text, one fact per line: no boxes, no fixed widths, nothing that
//! breaks when the terminal is narrow or a path is long. A box needs a fixed
//! width and a terminal does not have one, so a panel that lined up at 120
//! columns wrapped into noise at 60. This is a list instead — indented under a
//! heading — and it reads the same at any width.
//!
//! Everything here is built when it is asked for: the tools, the model the
//! session is on and the servers that are connected are all things that change
//! while the process runs, so a report captured at startup would be a snapshot
//! of the wrong moment.

use std::path::PathBuf;

use norte_code_protocol::config::CompactionTrigger;

use super::helpers::find_project_config;
use crate::modal::{Action, Row};

use super::SessionKey;
use super::{Config, ModelSpec, Path, Reasoning, Style};
use crate::remote_state::{ModelInfoExt, RemoteState};

/// The facts only the host knows, set once at startup.
///
/// The service cannot derive them: which file the catalogue came from, where
/// the session database is, and whether the system prompt was given on the
/// command line or read from the config are all decisions the CLI made before
/// the service existed. They travel together so `/debug` can report them
/// without the whole CLI being plumbed through every caller.
#[derive(Clone, Debug)]
pub(crate) struct HostFacts {
    /// The providers file the catalogue was read from.
    pub(crate) providers_path: PathBuf,
    /// The models file the catalogue was read from.
    pub(crate) models_path: PathBuf,
    /// The session database the process opened.
    pub(crate) database: PathBuf,
    /// Where the system prompt came from: `flag`, `config` or `default`.
    pub(crate) prompt_source: &'static str,
}

/// One line of the report: a fact about something.
///
/// The left side is the name and the right side is the value; they are joined
/// with a colon rather than padded into columns, because padding needs a width
/// and the terminal has one only at the moment it is drawn.
fn fact(label: &str, value: impl std::fmt::Display) -> String {
    format!("  {label}: {value}")
}

/// The heading that opens a section.
fn heading(style: Style, title: &str) -> String {
    style.bold(title.to_owned())
}

/// The first line of every run: where it is, what it is on, where it writes.
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn report_start(
    style: Style,
    spec: &ModelSpec,
    configuration: &Config,
    config_path: Option<&Path>,
    session: Option<&str>,
    provider_count: usize,
    model_count: usize,
    provider_known: bool,
    host: Option<&HostFacts>,
) -> Vec<String> {
    let workspace =
        std::env::current_dir().map_or_else(|_| "?".to_owned(), |path| path.display().to_string());
    let config_label =
        config_path.map_or_else(|| "(default)".to_owned(), |path| path.display().to_string());
    let project_config = find_project_config(&std::env::current_dir().unwrap_or_default())
        .map_or_else(|| "(none)".to_owned(), |path| path.display().to_string());
    let sandbox = if configuration.tools.confinement.sandbox.enabled {
        let name = if cfg!(target_os = "linux") {
            "bwrap"
        } else if cfg!(target_os = "macos") {
            "seatbelt"
        } else {
            "unsupported platform"
        };
        format!(
            "enabled · {name} · network {}",
            if configuration.tools.confinement.sandbox.network {
                "on"
            } else {
                "off"
            }
        )
    } else {
        "disabled".to_owned()
    };
    // The threshold is a fraction between 0 and 1, so rounding it to whole
    // percent never drops a meaningful digit.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let compaction_pct = (configuration.compaction.threshold * 100.0).round() as u64;
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let compaction_tokens = spec
        .max_context
        .map(|context| (context as f64 * configuration.compaction.threshold).round() as u64);
    let max_iterations = configuration
        .max_iterations
        .map_or_else(|| "unlimited".to_owned(), |limit| limit.to_string());

    let mut lines = vec![
        heading(style, "session"),
        fact("key", session.unwrap_or("(new; shown on the first event)")),
        fact("model", spec.label()),
        fact(
            "provider",
            format!(
                "{}{}",
                spec.provider,
                if provider_known {
                    String::new()
                } else {
                    "  (not in the catalogue; from the config)".to_owned()
                }
            ),
        ),
        fact("wire", "configured on remote host"),
        fact("iterations", &max_iterations),
        fact(
            "compaction",
            format!(
                "{}%{}{}",
                compaction_pct,
                compaction_tokens
                    .map(|tokens| format!(" (~{tokens} tokens)"))
                    .unwrap_or_default(),
                if !configuration.compaction.enabled {
                    " (disabled)"
                } else if configuration.compaction.trigger == CompactionTrigger::Eager {
                    " (eager)"
                } else {
                    " (idle)"
                }
            ),
        ),
        fact(
            "keep recent",
            format!(
                "{} message(s) after a compaction",
                configuration.compaction.keep_recent
            ),
        ),
        fact(
            "reasoning",
            format!("{} effort(s) available", spec.reasoning.efforts.len()),
        ),
        String::new(),
        heading(style, "paths"),
        fact("workspace", &workspace),
        fact("config", &config_label),
        fact("project config", &project_config),
        fact(
            "database",
            host.map_or_else(
                || "(unknown)".to_owned(),
                |host| host.database.display().to_string(),
            ),
        ),
        fact("sandbox", &sandbox),
        String::new(),
        heading(style, "catalogue"),
        fact(
            "providers",
            host.map_or_else(
                || "(unknown)".to_owned(),
                |host| host.providers_path.display().to_string(),
            ),
        ),
        fact(
            "models file",
            host.map_or_else(
                || "(unknown)".to_owned(),
                |host| host.models_path.display().to_string(),
            ),
        ),
        fact(
            "models",
            format!("{provider_count} provider(s) · {model_count} model(s)"),
        ),
    ];
    lines.extend(model_lines(style, spec, None));
    lines
}

/// What the model itself can do, from the catalogue entry when there is one.
pub(crate) fn model_lines(
    style: Style,
    spec: &ModelSpec,
    model: Option<&norte_code_protocol::ModelInfo>,
) -> Vec<String> {
    let mut lines = vec![String::new(), heading(style, "model")];
    if let Some(model) = model {
        if !model.name.is_empty() {
            lines.push(fact("name", &model.name));
        }
        if let Some(description) = &model.description {
            lines.push(fact("about", description));
        }
    } else {
        lines.push(fact(
            "catalogue",
            "not found; the values below are the config's",
        ));
    }
    lines.push(fact(
        "wire",
        format!("remote catalogue · provider {}", spec.provider),
    ));
    lines.push(fact("context", token_limit_label(spec.max_context)));
    lines.push(fact("output", token_limit_label(spec.max_output)));
    lines.push(fact("modalities", "text"));
    lines.push(fact("tools", if spec.tools { "tools" } else { "-" }));
    lines.push(fact(
        "reasoning",
        if spec.reasoning.supported {
            format!("{} effort(s) available", spec.reasoning.efforts.len())
        } else {
            "not supported".to_owned()
        },
    ));
    lines
}

/// The tools panel, as rows a person can act on.
///
/// Every tool is a row whose action is the next state of its rule, and the key
/// walks the rule through the three it has: an allow becomes a deny, a deny
/// becomes an ask — the rule is forgotten, on both sides, and a call is decided
/// by the mode again — and an ask becomes an allow. Setting a rule and taking it
/// back are the same key, which is why the row carries an action rather than a
/// pair of buttons. The scope is the panel's, not the row's — a person denying
/// ten tools denies them for the same reach — which is why it is not on the row.
///
/// It reads the same `tool_rules` the report does, so the row and the rule
/// beside it cannot disagree.
pub(crate) async fn tools_panel(
    style: Style,
    state: &RemoteState,
    session: Option<&SessionKey>,
) -> Vec<Row> {
    let statuses = match state.tool_rules_remote(session.cloned()).await {
        Ok(statuses) => statuses,
        Err(error) => {
            return vec![Row::text(
                style.red(format!("could not read tools: {error}")),
            )];
        }
    };
    if statuses.is_empty() {
        return vec![Row::text(style.yellow("no tools are registered"))];
    }
    let mut rows = vec![Row::heading("tools: what a call to each may do")];
    let widest = statuses
        .iter()
        .map(|status| status.name.chars().count())
        .max()
        .unwrap_or(0);
    for status in &statuses {
        // The key walks the rule through its three states, so the row's own
        // word is what the next press does: a tool with no rule becomes allowed,
        // an allowed one becomes denied, and a denied one is forgotten so a call
        // to it runs again. Setting and clearing both, from the panel, is why
        // there are three: a rule a person set by hand is one they must be able
        // to take back.
        let (word, action) = match status.scope.as_str() {
            "default" => ("runs", Action::Allow(status.name.clone())),
            _ if status.allowed => ("allow", Action::Deny(status.name.clone())),
            _ => ("deny", Action::Forget(status.name.clone())),
        };
        let scope = match status.scope.as_str() {
            "session" => "this session",
            "global" => "every session",
            _ => "by default",
        };
        rows.push(
            Row::actable(format!("{:<widest$}", status.name), action)
                .noted(format!("{word}  {scope}")),
        );
    }
    rows
}

/// The tools the model can reach, one per line, with what each is for.
///
/// Built from the registry rather than from a list kept beside it, so a tool
/// that was registered a moment ago is in the report and a tool that failed to
/// register is not. The first line of a description is enough to say what the
/// tool is; the whole text is the model's, and it is long.
pub(crate) async fn report_tools(
    style: Style,
    state: &RemoteState,
    session: Option<&norte_code_protocol::SessionKey>,
) -> Vec<String> {
    let mut lines = vec![heading(style, "tools")];
    // The session's list and not the configuration's: a session keeps the tools
    // it was made with, so this is what the model can actually reach in it.
    match state.tool_rules_remote(session.cloned()).await {
        Ok(tools) => {
            lines.push(fact("registered", tools.len()));
            for tool in tools {
                let rule = if tool.scope == "default" {
                    "default"
                } else if tool.allowed {
                    "allow"
                } else {
                    "deny"
                };
                lines.push(fact(&tool.name, format!("{rule} ({})", tool.scope)));
            }
        }
        Err(error) => lines.push(fact("error", error)),
    }
    lines
}

/// The MCP panel, as rows a person can act on.
///
/// A server's row switches it: Enter turns off the one that is on and on the
/// one that is off, and the row's own word says which way it will go. The tools
/// the server publishes are rows that are not actable — they are what the server
/// brought, and a tool is refused with `/deny`, not by turning its server off.
///
/// A server that failed to start is a row too, and not an actable one: there is
/// nothing running to switch, and a key that did nothing would say the press was
/// wrong rather than the server.
pub(crate) async fn mcp_panel(state: &RemoteState) -> Vec<Row> {
    let (connected, failures) = match state.mcp_status_remote().await {
        Ok(status) => status,
        Err(error) => return vec![Row::text(format!("could not read MCP status: {error}"))],
    };
    if connected.is_empty() && failures.is_empty() {
        return Vec::new();
    }
    let mut rows = vec![Row::heading("mcp servers")];
    for server in &connected {
        let state = if server.active { "on" } else { "off" };
        rows.push(
            Row::actable(
                format!(
                    "{:<14} {} · {} tool(s)",
                    server.name,
                    server.command,
                    server.tools.len()
                ),
                Action::Toggle(server.name.clone()),
            )
            .noted(format!("{state}  (enter switches)")),
        );
        for tool in &server.tools {
            rows.push(Row::text(format!("    {tool}")));
        }
        for warning in &server.filtered {
            // Plain text: the panel draws the ink, and a row that carried an SGR
            // code would show it as the characters it is.
            rows.push(Row::text(format!("    {warning}")));
        }
    }
    for failure in &failures {
        rows.push(Row::text(format!(
            "{} failed: {}",
            failure.name, failure.reason
        )));
    }
    rows
}

/// The MCP servers, connected or not, with their tools.
pub(crate) async fn report_mcp(style: Style, state: &RemoteState) -> Vec<String> {
    let (connected, failures) = match state.mcp_status_remote().await {
        Ok(status) => status,
        Err(error) => return vec![fact("error", error)],
    };
    if connected.is_empty() && failures.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![heading(style, "mcp servers")];
    for server in &connected {
        lines.push(fact(
            &server.name,
            format!(
                "{} · {} tool(s){}",
                server.command,
                server.tools.len(),
                if server.active { "" } else { " (off)" }
            ),
        ));
        for tool in &server.tools {
            lines.push(format!("    {tool}"));
        }
        for warning in &server.filtered {
            lines.push(format!("    {}", style.yellow(warning.clone())));
        }
    }
    for failure in &failures {
        lines.push(fact(
            &style.red(failure.name.clone()),
            format!("failed: {}", failure.reason),
        ));
    }
    lines
}

/// The static prompt size in characters. Tokens are provider-reported, so no
/// estimate is made here.
pub(crate) fn report_prompt(style: Style, _state: &RemoteState, source: &str) -> Vec<String> {
    vec![
        heading(style, "prompt material"),
        fact("source", source),
        fact("prompt", "owned and evaluated by remote host"),
    ]
}

/// The reasoning setting the session is actually using.
///
/// The label is the one the footer shows, not one computed here: the two are the
/// same fact, and computing it twice is how they came to disagree — the footer
/// resolved the command line's setting and this resolved the session's override,
/// which is empty in a session started with `--reasoning`.
pub(crate) fn report_reasoning(style: Style, label: &str) -> Vec<String> {
    vec![heading(style, "reasoning"), fact("effective", label)]
}

/// Names the reasoning setting in the words the command line accepts.
pub(crate) fn reasoning_setting_label(reasoning: &Reasoning) -> String {
    match reasoning {
        Reasoning::Automatic => "auto".to_owned(),
        Reasoning::Disabled => "off".to_owned(),
        Reasoning::Effort { effort } => effort.clone(),
        Reasoning::Budget { tokens } => format!("budget {tokens} tokens"),
    }
}

/// Formats a token limit without guessing an unknown value.
pub(crate) fn token_limit_label(limit: Option<u64>) -> String {
    limit.map_or_else(|| "unknown".to_owned(), |value| format!("{value} tokens"))
}
