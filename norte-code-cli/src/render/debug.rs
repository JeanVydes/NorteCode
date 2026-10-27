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

use norte_code_catalog::{Catalog, Model};

use super::helpers::find_project_config;
use super::names::{
    cache_name, feature_set, modality_set, reasoning_capabilities, streaming_name, structured_set,
    tool_kind_set, yes_no,
};
use super::{ApprovalMode, RuleScope, SessionKey, ToolRule};
use super::{Config, ModelSpec, Path, Reasoning, Service, Source, Style};
use super::{approval_label, source_label};

/// The facts only the host knows, set once at startup.
///
/// The service cannot derive them: which file the catalogue came from, where
/// the session database is, and whether the system prompt was given on the
/// command line or read from the config are all decisions the CLI made before
/// the service existed. They travel together so `/debug` can report them
/// without the whole CLI being plumbed through every caller.
#[derive(Clone, Debug)]
pub(crate) struct HostFacts {
    /// Where the catalogue was loaded from.
    pub(crate) catalogue_source: Source,
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
pub(crate) fn report_start(
    style: Style,
    spec: &ModelSpec,
    configuration: &Config,
    config_path: Option<&Path>,
    session: Option<&str>,
    catalog: &Catalog,
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
        .capabilities
        .max_context
        .map(|context| (context as f64 * configuration.compaction.threshold).round() as u64);
    let provider_count = catalog.providers.len();
    let model_count: usize = catalog.providers.values().map(|p| p.models.len()).sum();
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
                if catalog.provider(&spec.provider).is_some() {
                    String::new()
                } else {
                    "  (not in the catalogue; from the config)".to_owned()
                }
            ),
        ),
        fact("wire", spec.wire.family()),
        fact("approval", approval_label(configuration.approval_mode)),
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
                } else if configuration.compaction.trigger == "eager" {
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
            format!(
                "{} effort(s) available",
                spec.capabilities.reasoning.efforts.len()
            ),
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
            "source",
            host.map_or_else(
                || "(unknown)".to_owned(),
                |host| source_label(host.catalogue_source).to_owned(),
            ),
        ),
        fact(
            "models",
            format!("{provider_count} provider(s) · {model_count} model(s)"),
        ),
    ];
    lines.extend(model_lines(
        style,
        spec,
        catalog.find(&spec.provider, &spec.model),
    ));
    lines
}

/// What the model itself can do, from the catalogue entry when there is one.
pub(crate) fn model_lines(style: Style, spec: &ModelSpec, model: Option<&Model>) -> Vec<String> {
    let capabilities = &spec.capabilities;
    let mut lines = vec![String::new(), heading(style, "model")];
    if let Some(model) = model {
        if !model.name.is_empty() {
            lines.push(fact("name", &model.name));
        }
        if let Some(family) = &model.family {
            lines.push(fact("family", family));
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
        format!("{} · provider {}", spec.wire.family(), spec.provider),
    ));
    lines.push(fact("context", token_limit_label(capabilities.max_context)));
    lines.push(fact("output", token_limit_label(capabilities.max_output)));
    lines.push(fact(
        "modalities",
        format!(
            "input {} · output {}",
            modality_set(capabilities.modalities.input.keys()),
            modality_set(capabilities.modalities.output.iter())
        ),
    ));
    lines.push(fact("tools", tool_kind_set(&capabilities.tool_kinds)));
    lines.push(fact(
        "reasoning",
        reasoning_capabilities(&capabilities.reasoning),
    ));
    lines.push(fact(
        "streaming",
        capabilities
            .streaming
            .map_or_else(|| "-".to_owned(), |value| streaming_name(value).to_owned()),
    ));
    lines.push(fact("cache", cache_name(capabilities.cache)));
    lines.push(fact("features", feature_set(&capabilities.features)));
    lines.push(fact(
        "structured",
        structured_set(&capabilities.structured_output),
    ));
    if let Some(model) = model {
        let mut metadata = Vec::new();
        if let Some(status) = &model.status {
            metadata.push(format!("status {status}"));
        }
        if let Some(knowledge) = &model.knowledge {
            metadata.push(format!("knowledge {knowledge}"));
        }
        if let Some(released) = &model.release_date {
            metadata.push(format!("released {released}"));
        }
        if let Some(updated) = &model.last_updated {
            metadata.push(format!("updated {updated}"));
        }
        if let Some(open_weights) = model.open_weights {
            metadata.push(format!("open weights {}", yes_no(open_weights)));
        }
        if model.attachment {
            metadata.push("attachments".to_owned());
        }
        if let Some(temperature) = model.temperature {
            metadata.push(format!("temperature {}", yes_no(temperature)));
        }
        if !metadata.is_empty() {
            lines.push(fact("catalogue", metadata.join(" · ")));
        }
    }
    lines
}

/// The tools the model can reach, one per line, with what each is for.
///
/// Built from the registry rather than from a list kept beside it, so a tool
/// that was registered a moment ago is in the report and a tool that failed to
/// register is not. The first line of a description is enough to say what the
/// tool is; the whole text is the model's, and it is long.
pub(crate) fn report_tools(style: Style, service: &Service) -> Vec<String> {
    let mut lines = vec![heading(style, "tools")];
    match service.tools() {
        Ok(registry) => {
            let specs = registry.specs();
            lines.push(fact("registered", specs.len()));
            for spec in &specs {
                let summary = spec
                    .description
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                lines.push(format!(
                    "  {} — {}",
                    style.bold(spec.name.to_string()),
                    style.dim(summary)
                ));
            }
        }
        Err(error) => lines.push(fact("error", error)),
    }
    lines
}

/// The one-row tools summary the footer shows.
///
/// Counts, not names: the footer has a row and there are a dozen tools, so the
/// count is what fits and `/tools` is where the names are. The counts are of
/// what a call meets — deny before allow before the approval mode — which is
/// why they come from the service and not from the config file.
pub(crate) fn tools_summary(
    style: Style,
    service: &Service,
    session: Option<&SessionKey>,
) -> String {
    let statuses = service.tool_rules(session);
    if statuses.is_empty() {
        return String::new();
    }
    let count = |rule: ToolRule| statuses.iter().filter(|it| it.rule == rule).count();
    let (allow, deny, ask) = (
        count(ToolRule::Allow),
        count(ToolRule::Deny),
        count(ToolRule::Ask),
    );
    let mut parts: Vec<String> = Vec::new();
    // Colour carries the meaning here: these three are what a person scans the
    // row for, and the count beside each says how much of the registry is
    // behind it. A state nobody is in is left out, so a short row means the
    // tools are mostly one thing.
    if allow > 0 {
        parts.push(style.green(format!("{allow} allow")));
    }
    if deny > 0 {
        parts.push(style.red(format!("{deny} deny")));
    }
    if ask > 0 {
        parts.push(style.yellow(format!("{ask} ask")));
    }
    format!(
        "{} {} {}",
        style.faint("tools"),
        style.bold(statuses.len()),
        parts.join(&style.faint(" · "))
    )
}

/// The one-row MCP summary the footer shows.
///
/// Servers and tools, and a failure count when there is one: a server that did
/// not start is the one fact that must not be left off the row, because nothing
/// else on screen would say it.
pub(crate) fn mcp_summary(style: Style, service: &Service) -> String {
    let connected = service.mcp_status();
    let failures = service.mcp_failures();
    if connected.is_empty() && failures.is_empty() {
        return style.faint("mcp · none configured");
    }
    let tools: usize = connected.iter().map(|server| server.tools.len()).sum();
    let active = connected.iter().filter(|server| server.active).count();
    let mut line = format!(
        "{} {} {}",
        style.faint("mcp"),
        style.bold(connected.len()),
        style.faint(format!("({active} on) · {tools} tool(s)"))
    );
    if !failures.is_empty() {
        line.push_str(&style.faint(" · "));
        line.push_str(&style.red(format!("{} failed", failures.len())));
    }
    line
}

/// Every tool with how a call to it is decided, for `/tools`.
///
/// The footer has counts and this is where the names are: a summary saying
/// three tools are denied is useless without saying which three. The scope is
/// printed beside each one because a rule of this session and a rule of every
/// session read the same otherwise, and which it is decides who else it binds.
pub(crate) fn report_tool_rules(
    style: Style,
    service: &Service,
    session: Option<&SessionKey>,
) -> Vec<String> {
    let statuses = service.tool_rules(session);
    if statuses.is_empty() {
        return vec![style.yellow("no tools are registered")];
    }
    let mut lines = vec![heading(style, "tools")];
    lines.push(fact(
        "approval mode",
        match service.config().approval_mode {
            ApprovalMode::Ask => "ask (writes and commands are approved)",
            ApprovalMode::AutoEdit => "auto-edit (commands are approved)",
            ApprovalMode::Yolo => "yolo (nothing is approved)",
        },
    ));
    // The widest name decides the column, so the rule beside each one lines up
    // and the list reads down rather than across.
    let widest = statuses
        .iter()
        .map(|status| status.name.chars().count())
        .max()
        .unwrap_or(0);
    for status in &statuses {
        let (mark, word) = match status.rule {
            ToolRule::Allow => (style.green("✓"), "allow"),
            ToolRule::Deny => (style.red("✗"), "deny"),
            ToolRule::Ask => (style.yellow("?"), "ask"),
        };
        let scope = match status.scope {
            RuleScope::Session => "this session",
            RuleScope::Global => "every session",
            RuleScope::Mode => "by the mode",
        };
        lines.push(format!(
            "  {mark} {name:<widest$}  {word}  {scope}",
            name = style.bold(status.name.clone()),
            word = style.dim(word),
            scope = style.faint(scope),
        ));
    }
    lines
}

/// The MCP servers, connected or not, with their tools.
pub(crate) fn report_mcp(style: Style, service: &Service) -> Vec<String> {
    let connected = service.mcp_status();
    let failures = service.mcp_failures();
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
    for (name, reason) in &failures {
        lines.push(fact(&style.red(name.clone()), format!("failed: {reason}")));
    }
    lines
}

/// The static prompt size in characters. Tokens are provider-reported, so no
/// estimate is made here.
pub(crate) fn report_prompt(style: Style, service: &Service, source: &str) -> Vec<String> {
    let size = match service.prompt_size() {
        Ok(size) => size,
        Err(error) => {
            return vec![fact(
                "prompt",
                style.yellow(format!("could not measure it: {error}")),
            )];
        }
    };
    let mut lines = vec![heading(style, "prompt material")];
    lines.push(fact("source", source));
    lines.push(fact("system", format!("{} chars", size.system_chars)));
    if let Some(path) = &size.project_path {
        lines.push(fact(
            "project",
            format!("{} chars ({})", size.project_chars, path.display()),
        ));
    }
    lines.push(fact("tools", format!("{} chars", size.tool_chars)));
    lines.push(fact("total", format!("{} chars", size.total_chars())));
    lines
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
