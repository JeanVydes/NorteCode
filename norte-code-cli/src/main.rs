//! The headless `norte-code` command.
//!
//! `norte-code run "prompt"` builds the service in memory, sends one message
//! and streams the answer. With `--interactive` the same session stays open and
//! reads the next message from the terminal. Text goes to stdout; diagnostics,
//! reasoning and tool activity go to stderr. `--jsonl` writes one protocol
//! event per line on stdout instead, for scripts and tests.
//!
//! `norte-code models` lists what the catalogue knows, filtered by provider,
//! tool calling or reasoning.

mod app;
mod art;
mod commands;
mod commands_registry;
mod input;
mod interaction;
mod markdown;
mod modal;
mod models;
mod plugin_loader;
mod plugins;
mod preview;
mod prompt;
mod render;
mod select;
mod startup;
mod style;
mod telemetry_panel;
mod terminal;
mod text;
mod transcript;

use commands::{apply_extensions, database_path, duration_label, enter_workdir, run};
use input::{InteractiveInput, Pinned};
use interaction::{drive_turns, interactive_loop, refresh_effort};
use models::{list_models, resolve_spec};
use preview::{clip_text, compact_json};
use render::{
    Format, HostFacts, Renderer, Verbosity, cache_percent_label, find_project_config, human_tokens,
    parse_budget_action, parse_budget_unit, parse_reasoning_setting, reasoning_label,
    reasoning_setting_label, select_system_prompt, short,
};
use startup::Open;

#[cfg(test)]
#[cfg(test)]
use commands::cost_summary;
#[cfg(test)]
use prompt::compose_selection;
#[cfg(test)]
use render::budget_label;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::any::Any;

use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use futures_util::StreamExt;
use norte::{
    CacheMode, Feature, Modality, ReasoningEffort, StreamingGranularity, StructuredOutputMode,
    ToolKind,
};
use norte_code_agent::{
    EventStream, ModelSpec, RuleScope, Service, VendorModels, effective_reasoning, model_list,
};
use norte_code_protocol::catalog::{Catalog, WireSchema};
use norte_code_protocol::config::{Config, DiffStyle, user_config_path};
use norte_code_protocol::costs::{CostSummary, by_day, by_session, by_turn, summarize};
use norte_code_protocol::skills::{SkillRegistry, discovery_roots};
use norte_code_protocol::sqlite::{RequestRecord, SessionDirectory, SqliteStore};
use norte_code_protocol::plugins::PluginContext;
use norte_code_protocol::{
    Budget, BudgetAction, BudgetScope, BudgetUnit, ClientRequest, ClientRequestKind, Delta,
    ErrorCode, JobInfo, ModelInfo, Phase, Reasoning, ReasoningInfo, ReasoningScope, ServerEvent,
    ServerEventKind, SessionKey,
};
use tokio::sync::mpsc;

use crate::style::Style;

#[derive(Parser)]
#[command(
    name = "norte-code",
    version,
    about = "A terminal-based agentic coding tool"
)]
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    /// The directory to work in. It is where the tools read and write, and it
    /// is the whole command line a person needs to work somewhere else: a bare
    /// `norte-code` works where the shell already is.
    ///
    /// It is a positional and not a flag, because it is the one thing a person
    /// names when they name anything: `norte-code ~/src/thing` says what every
    /// other command line means by a leading path.
    #[arg(value_name = "DIR")]
    dir: Option<PathBuf>,
    /// The user configuration file.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// How much of the conversation to show: `low` (the answer alone),
    /// `medium` (also that tools run and that the model is thinking) or `high`
    /// (the reasoning, what each tool was asked and what it returned).
    ///
    /// Left out, `ui.verbosity` from the configuration decides, and `medium`
    /// when the configuration says nothing either: the flag is an override for
    /// one run, not the only way to set it.
    #[arg(long, global = true, value_name = "LEVEL")]
    verbose: Option<String>,
    /// What to do. Omitted, the startup flow asks: new session, last session,
    /// or one from the list.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start a new session and stay in it.
    New(NewArgs),
    /// Continue a parked, stopped or interrupted session.
    Resume(ResumeArgs),
    /// List the models the catalogue knows.
    Models(ModelsArgs),
    /// Inspect the installed Agent Skills.
    Skills(SkillsArgs),
    /// List the sessions stored on disk, or delete one.
    Sessions(SessionsArgs),
    /// Report the provider-reported cost ledger.
    Cost(CostArgs),
    /// Report local request telemetry: counts, tokens, cache, tools and latency.
    Telemetry(TelemetryArgs),
    /// Manage the MCP servers in the config file.
    Mcp(McpArgs),
}

#[derive(Args)]
struct McpArgs {
    /// What to do with the configured MCP servers.
    #[command(subcommand)]
    command: McpCommand,
}

#[derive(Subcommand)]
enum McpCommand {
    /// List the configured MCP servers.
    List,
    /// Add a server, or replace one of the same name.
    Add {
        /// The name the server is known by.
        name: String,
        /// The program to run.
        command: String,
        /// Its arguments, in order.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Turn a configured server on.
    Enable {
        /// The name of the server.
        name: String,
    },
    /// Turn a configured server off, keeping its entry.
    Disable {
        /// The name of the server.
        name: String,
    },
    /// Remove a configured server.
    Remove {
        /// The name of the server.
        name: String,
    },
}

/// The flags that shape a new session.
///
/// A new session is created and then read from the terminal, so there is no
/// prompt to pass: the first line typed is the first message. Everything here
/// overrides what the configuration would have used, and `--model`/`--provider`
/// are what make a session open on something other than `default_provider` and
/// `default_model`.
#[derive(Args)]
// A command line has flags: clap needs each one as a bool field, and they are
// not a state machine to model. `SkillsConfig` carries the same allow.
#[allow(clippy::struct_excessive_bools)]
struct NewArgs {
    /// The model id (defaults to `default_model` in config). It is an opaque
    /// string and may contain slashes (e.g. `nvidia/nemotron`).
    #[arg(long, short = 'm', value_name = "MODEL")]
    model: Option<String>,
    /// The provider id (e.g. `openai`, `groq`). Defaults to `default_provider`
    /// in config. A model is never taken apart to find one.
    #[arg(long, value_name = "PROVIDER")]
    provider: Option<String>,
    /// Replace the built-in system prompt for this run.
    #[arg(long, value_name = "TEXT")]
    system_prompt: Option<String>,
    /// A base URL override for the provider.
    #[arg(long, value_name = "URL")]
    endpoint: Option<String>,
    /// The schema to speak, picking among the wires the provider serves.
    ///
    /// Left out, the provider's preferred one is used: its `OpenAI` surface when
    /// it has one. A provider that serves several is where this matters, and
    /// naming one it does not serve is an error that lists what it has.
    #[arg(long, value_name = "SCHEMA", value_parser = parse_schema)]
    wire: Option<WireSchema>,
    /// A limit for this message.
    #[arg(long, value_name = "LIMIT")]
    budget: Option<f64>,
    /// What `--budget` measures.
    #[arg(long, value_name = "UNIT", value_parser = ["usd", "tokens", "requests"], default_value = "usd")]
    budget_unit: String,
    /// What to do when `--budget` is exhausted.
    #[arg(long, value_name = "ACTION", value_parser = ["warn", "ask", "stop"], default_value = "stop")]
    budget_action: String,
    /// The used-context fraction that triggers compaction, in (0, 1].
    #[arg(long, value_name = "FRACTION")]
    compaction_threshold: Option<f64>,
    /// Sandbox overrides for this run.
    #[command(flatten)]
    sandbox: SandboxOptions,
    /// A reasoning setting: `auto`, `off`, or an effort such as `low`.
    #[arg(long, value_name = "SETTING")]
    reasoning: Option<String>,
    /// Stop after this many model iterations; unlimited by default.
    #[arg(long, value_name = "N")]
    max_iterations: Option<usize>,
    /// A vendor request extra as `KEY=JSON` (repeatable). For `OpenRouter`,
    /// `openai.chat:provider={"order":["deepinfra"]}` chooses the upstream.
    #[arg(long, value_name = "KEY=JSON")]
    extension: Vec<String>,
}

#[derive(Args)]
struct ResumeArgs {
    /// The session key to continue. Omitted, the startup flow asks which
    /// session to open.
    #[arg(value_name = "SESSION")]
    session: Option<String>,
    /// Replace the built-in system prompt for this resume.
    #[arg(long, value_name = "TEXT")]
    system_prompt: Option<String>,
    /// The model id to resume with, replacing the one the session holds. It is
    /// an opaque string and may contain slashes (e.g. `nvidia/nemotron`).
    #[arg(long, short = 'm', value_name = "MODEL")]
    model: Option<String>,
    /// Replace the session budget before resuming.
    #[arg(long, value_name = "LIMIT")]
    budget: Option<f64>,
    /// What the budget measures.
    #[arg(long, value_name = "UNIT", value_parser = ["usd", "tokens", "requests"], default_value = "usd")]
    budget_unit: String,
    /// What to do when the budget is exhausted.
    #[arg(long, value_name = "ACTION", value_parser = ["warn", "ask", "stop"], default_value = "stop")]
    budget_action: String,
    /// The used-context fraction that triggers compaction, in (0, 1].
    #[arg(long, value_name = "FRACTION")]
    compaction_threshold: Option<f64>,
    /// Sandbox overrides for this resume.
    #[command(flatten)]
    sandbox: SandboxOptions,
    /// Stop after this many model iterations; keeps the config's limit when
    /// omitted.
    #[arg(long, value_name = "N")]
    max_iterations: Option<usize>,
    /// A vendor request extra as `KEY=JSON` (repeatable).
    #[arg(long, value_name = "KEY=JSON")]
    extension: Vec<String>,
    /// Write one JSON event per line on stdout.
    #[arg(long)]
    jsonl: bool,
    /// Keep the session open and read the next message after each turn.
    #[arg(long, short = 'i')]
    interactive: bool,
}

impl NewArgs {
    /// The flags a new session gets when nothing was passed.
    ///
    /// The startup flow reaches `new_session` with no flags of its own: the
    /// model was decided by the menu, and everything else comes from the
    /// configuration.
    pub(crate) fn bare() -> Self {
        Self {
            model: None,
            provider: None,
            system_prompt: None,
            endpoint: None,
            wire: None,
            budget: None,
            budget_unit: "usd".to_owned(),
            budget_action: "stop".to_owned(),
            compaction_threshold: None,
            sandbox: SandboxOptions {
                sandbox: false,
                no_sandbox: false,
                sandbox_network: None,
            },
            reasoning: None,
            max_iterations: None,
            extension: Vec::new(),
        }
    }
}

impl ResumeArgs {
    /// The flags a bare `norte-code` runs with.
    ///
    /// A bare invocation names nothing, so every switch is at its default and
    /// the startup flow supplies the one decision that is missing. The global
    /// flags — `--config`, `--offline`, `--verbose` — still apply: they are
    /// parsed before the subcommand is chosen.
    pub(crate) fn bare() -> Self {
        Self {
            session: None,
            system_prompt: None,
            model: None,
            budget: None,
            budget_unit: "usd".to_owned(),
            budget_action: "stop".to_owned(),
            compaction_threshold: None,
            sandbox: SandboxOptions {
                sandbox: false,
                no_sandbox: false,
                sandbox_network: None,
            },
            max_iterations: None,
            extension: Vec::new(),
            jsonl: false,
            interactive: true,
        }
    }
}

/// Flags that override the command sandbox for one run or resume.
#[derive(Args)]
struct SandboxOptions {
    /// Run commands inside the OS sandbox, overriding `tools.confinement.sandbox.enabled`.
    #[arg(long, conflicts_with = "no_sandbox")]
    sandbox: bool,
    /// Run commands without the OS sandbox, overriding `tools.confinement.sandbox.enabled`.
    #[arg(long, conflicts_with_all = ["sandbox", "sandbox_network"])]
    no_sandbox: bool,
    /// Let the sandbox use the network: `on` or `off`.
    #[arg(
        long,
        value_name = "ON|OFF",
        value_parser = ["on", "off"],
        conflicts_with = "no_sandbox"
    )]
    sandbox_network: Option<String>,
}

impl SandboxOptions {
    /// Applies the flags over the loaded config.
    fn apply(&self, config: &mut Config) -> Result<(), String> {
        if self.sandbox && self.no_sandbox {
            return Err("--sandbox cannot be combined with --no-sandbox".to_owned());
        }
        if self.no_sandbox && self.sandbox_network.is_some() {
            return Err("--sandbox-network cannot be combined with --no-sandbox".to_owned());
        }
        if self.sandbox {
            config.tools.confinement.sandbox.enabled = true;
        }
        if self.no_sandbox {
            config.tools.confinement.sandbox.enabled = false;
        }
        if let Some(network) = self.sandbox_network.as_deref() {
            if !config.tools.confinement.sandbox.enabled {
                return Err(
                    "--sandbox-network needs the sandbox; pass --sandbox or set \
                     tools.confinement.sandbox.enabled"
                        .to_owned(),
                );
            }
            config.tools.confinement.sandbox.network = match network {
                "on" => true,
                "off" => false,
                other => return Err(format!("unknown sandbox network setting `{other}`")),
            };
        }
        Ok(())
    }
}

/// Options shared by `run` and `resume` that only change the config.
struct CommonOptions<'a> {
    max_iterations: Option<usize>,
    compaction_threshold: Option<f64>,
    sandbox: &'a SandboxOptions,
    extensions: &'a [String],
    interactive: bool,
    jsonl: bool,
}

impl CommonOptions<'_> {
    /// Applies the options to a loaded config and checks their combinations.
    fn apply(&self, config: &mut Config) -> Result<(), String> {
        if let Some(limit) = self.max_iterations {
            if limit == 0 {
                return Err("--max-iterations must be at least 1".to_owned());
            }
            config.max_iterations = Some(limit);
        }
        if let Some(threshold) = self.compaction_threshold {
            if !threshold.is_finite() || threshold <= 0.0 || threshold > 1.0 {
                return Err(format!(
                    "--compaction-threshold must be a fraction in (0, 1], got {threshold}"
                ));
            }
            config.compaction.threshold = threshold;
        }
        self.sandbox.apply(config)?;
        if self.interactive && self.jsonl {
            return Err("--interactive cannot be combined with --jsonl".to_owned());
        }
        if self.interactive && !std::io::stdin().is_terminal() {
            return Err("--interactive needs a terminal on stdin".to_owned());
        }
        apply_extensions(config, self.extensions)
    }
}

#[derive(Args)]
struct SessionsArgs {
    /// What to do with the stored sessions.
    #[command(subcommand)]
    command: Option<SessionsCommand>,
    /// Write one JSON object per line.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum SessionsCommand {
    /// Delete a stored session and every row it left behind.
    Delete {
        /// The session key, as `sessions` prints it.
        key: String,
        /// Answer yes to the confirmation.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
struct CostArgs {
    /// Only this session, with its turns.
    #[arg(long, value_name = "KEY")]
    session: Option<String>,
    /// Group by UTC day only.
    #[arg(long)]
    daily: bool,
    /// Write one JSON object.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct TelemetryArgs {
    /// Only this session key.
    #[arg(long, value_name = "KEY")]
    session: Option<String>,
    /// Only this provider id.
    #[arg(long, value_name = "PROVIDER")]
    provider: Option<String>,
    /// Only this model id.
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,
    /// The time bucket for the series.
    #[arg(long, value_name = "BUCKET", value_parser = ["hour", "day", "week", "month", "year", "total"], default_value = "day")]
    bucket: String,
    /// Write one JSON object.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct SkillsArgs {
    /// What to do with the installed skills.
    #[command(subcommand)]
    command: SkillsCommand,
}

#[derive(Subcommand)]
enum SkillsCommand {
    /// List the valid skills.
    List {
        /// Write one JSON object per line.
        #[arg(long)]
        json: bool,
    },
    /// Validate every skill and report what was skipped.
    Check,
}

#[derive(Args)]
struct ModelsArgs {
    /// Only models from this provider.
    #[arg(long, value_name = "PROVIDER")]
    provider: Option<String>,
    /// Only models that call tools.
    #[arg(long)]
    tools: bool,
    /// Only models that reason.
    #[arg(long)]
    reasoning: bool,
    /// Write one JSON object per line.
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    // The startup flow makes this future large — a menu, a picker and the
    // service build all sit inside it — so it is boxed: it is awaited once, on
    // the outermost frame, where the size buys nothing.
    match Box::pin(run(cli)).await {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("norte-code: {}", text::terminal(&message));
            ExitCode::from(2)
        }
    }
}

/// Parses `--wire`, the schema a run speaks.
///
/// The names are the ones `providers.json` writes, so a person who reads the
/// file knows what to type here. `openai` is the chat completions surface and
/// not the Responses API: this host speaks the shape the three schemas share.
fn parse_schema(value: &str) -> Result<WireSchema, String> {
    for schema in WireSchema::ALL {
        if schema.label() == value {
            return Ok(schema);
        }
    }
    let expected = WireSchema::ALL
        .iter()
        .map(|schema| schema.label())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!("unknown wire `{value}`; expected {expected}"))
}

/// A warning when a USD budget cannot be measured because the provider never
/// reports cost.
fn budget_warning(provider: &str, config: &Config, budget_is_usd: bool) -> Option<String> {
    if !matches!(provider, "anthropic" | "google") {
        return None;
    }
    let configured = [
        &config.budgets.message,
        &config.budgets.session,
        &config.budgets.global_daily,
    ]
    .into_iter()
    .flatten()
    .any(|budget| budget.unit.eq_ignore_ascii_case("usd"));
    (budget_is_usd || configured).then(|| {
        format!(
            "warning: {provider} does not report cost, so a USD budget cannot be measured; \
             use --budget-unit tokens or requests"
        )
    })
}

/// Starts a new session and reads it from the terminal.
///
/// `model` is the reference the startup flow chose, when it had to choose one;
/// `args` carries what the command line said. The flags win, which is what makes
/// `norte-code new --model openai/gpt-5` open on that model while a bare
/// `norte-code` opens on `default_model`: the flag is an override, not a
/// requirement.
#[allow(clippy::too_many_lines)]
pub(crate) async fn new_session(
    config: Config,
    catalog: Catalog,
    model: Option<String>,
    args: NewArgs,
    verbosity: &str,
    config_path: Option<PathBuf>,
) -> Result<u8, String> {
    let spec = resolve_spec(
        &config,
        &catalog,
        args.provider.clone(),
        args.model.clone().or(model),
        args.endpoint.clone(),
        args.wire,
    )?;
    let model_label = spec.label();
    let provider = spec.provider.clone();
    let mut config = config;
    CommonOptions {
        max_iterations: args.max_iterations,
        compaction_threshold: args.compaction_threshold,
        sandbox: &args.sandbox,
        extensions: &args.extension,
        interactive: true,
        jsonl: false,
    }
    .apply(&mut config)?;
    let (system_prompt_source, system_prompt) =
        select_system_prompt(args.system_prompt, config.system_prompt.clone());
    let reasoning = args
        .reasoning
        .as_deref()
        .map(parse_reasoning_setting)
        .transpose()?;
    let effective = effective_reasoning(
        reasoning.as_ref(),
        &config.reasoning.default,
        &spec.capabilities.reasoning,
    )
    .map_err(|error| error.to_string())?;
    if let Some(limit) = args.budget {
        if !limit.is_finite() || limit <= 0.0 {
            return Err(format!("--budget must be a positive number, got {limit}"));
        }
    }
    let message_budget = match args.budget {
        Some(limit) => Some(Budget {
            unit: parse_budget_unit(&args.budget_unit)?,
            limit,
            action: parse_budget_action(&args.budget_action)?,
        }),
        None => None,
    };
    let style = Style::stderr();
    let verbosity = Verbosity::parse(verbosity)?;
    if verbosity.at_least(Verbosity::Medium) {
        eprintln!(
            "{} {} {} {}",
            style.magenta("◆"),
            style.bold(spec.label()),
            style.dim("·"),
            style.dim(format!("catalogue: {} models", catalog.model_count()))
        );
    }
    let budget_is_usd = message_budget
        .as_ref()
        .is_some_and(|budget| budget.unit == BudgetUnit::Usd);
    if let Some(warning) = budget_warning(&provider, &config, budget_is_usd) {
        if verbosity.at_least(Verbosity::Medium) {
            eprintln!("{}", style.red(warning));
        }
    }
    let show_cost = config.cost.show;
    // The footer's context dial needs the window and the compaction threshold
    // before the config and spec are moved into the service.
    let max_context = spec.capabilities.max_context;
    let compaction_threshold = config.compaction.threshold;
    let database = database_path()?;
    let store = Arc::new(
        SqliteStore::open(&database)
            .map_err(|error| format!("could not open {}: {error}", database.display()))?,
    );
    let session_store: Arc<dyn norte::harness::Store> = store.clone();
    let blobs: Arc<dyn norte::BlobStore> = store.clone();
    let directory: Arc<dyn SessionDirectory> = store;
    // Read before the config moves into the service: the renderer draws the diff
    // and the configuration is what says in which shape.
    let diff_style = config.ui.diff_style;
    let service = Arc::new(
        Service::with_directory(
            config,
            Arc::new(catalog),
            spec,
            Arc::new(VendorModels),
            session_store,
            blobs,
            directory,
            system_prompt,
        )
        .await
        .map_err(|e| e.to_string())?,
    );
    service.set_config_path(config_path);
    let pinned = Pinned::new();

    let mut renderer = Renderer::new(
        Format::Text,
        verbosity,
        show_cost,
        style,
        Arc::clone(&pinned),
    );
    renderer.set_cost_silent(matches!(provider.as_str(), "anthropic" | "google"));
    renderer.set_diff_style(diff_style);
    // What the plugins draw, asked for once: the columns and the status field
    // are theirs, and the frame draws what it was given.
    renderer.sidebar = crate::plugins::all(None)
        .sidebar()
        .into_iter()
        .map(|(name, rows)| (name.to_owned(), rows))
        .collect();
    renderer.model_name = Some(model_label);
    renderer.max_context = max_context;
    renderer.compaction_threshold = compaction_threshold;
    // The facts `/debug` reports that only the CLI knows: which file the
    // catalogue came from, the database it opened and where the system prompt
    // came from. Set once here, like the model name and the context window.
    renderer.host_facts = Some(HostFacts {
        database: database.clone(),
        providers_path: norte_code_protocol::catalog::providers_path().unwrap_or_default(),
        models_path: norte_code_protocol::catalog::models_path().unwrap_or_default(),
        prompt_source: system_prompt_source,
    });
    // The effort the next turn will ask for, on the footer where the session
    // and the totals are: nothing else on screen says it, and after a resume it
    // is not the flag that was passed but what the config and the model resolve.
    renderer.set_reasoning(reasoning_setting_label(&effective));
    // The catalogue names the completion offers. A new session has no key yet,
    // so only the provider list is offered until one exists: `/model` needs a
    // session to know which provider's models to list.
    crate::interaction::refresh_completions(&service, None, &mut renderer);
    // The terminal belongs to the renderer: it is what knows when something
    // changed, so it is what draws. A process with no terminal keeps the
    // plain-text path and draws nothing.
    if crate::terminal::is_interactive() {
        renderer.screen = Some(crate::terminal::Guard::enter()?);
    }
    let mut input = Some(InteractiveInput::start(Arc::clone(&pinned))?);
    // The session exists before the first line is read, so the line's message
    // names it: a session created as a side effect of a message cannot be shown
    // in the footer before the message is sent.
    let session = service
        .start_session(None)
        .await
        .map_err(|error| error.to_string())?;
    if verbosity.at_least(Verbosity::Medium) {
        eprintln!(
            "{} {}",
            style.dim("session"),
            style.faint(short(session.as_str()).to_owned())
        );
    }
    // Hand the service and session to plugins so they can reach them when they
    // run a command or redraw. Plugins that do not override set_context ignore it.
    let plugin_ctx = PluginContext {
        session_key: Some(session.clone()),
        service: Some(Arc::clone(&service) as Arc<dyn Any + Send + Sync>),
    };
    renderer.sidebar = crate::plugins::all(Some(&plugin_ctx))
        .sidebar()
        .into_iter()
        .map(|(name, rows)| (name.to_owned(), rows))
        .collect();
    if let Some(receiver) = input.as_mut() {
        interactive_loop(
            &service,
            Some(session),
            &mut renderer,
            style,
            receiver,
            message_budget,
            reasoning,
            Some(plugin_ctx),
        )
        .await?;
    }
    renderer.ensure_newline();
    renderer.summary();
    Ok(u8::from(renderer.failed))
}

/// The directory a stored session was working in, when it recorded one.
///
/// It reads the row before the service is built, because the directory has to
/// change before the tools are rooted: a service built in the wrong tree has
/// already decided what its tools point at.
///
/// A database that cannot be read is not a reason to refuse the resume: the
/// session is usable here, and the directory is where it would have gone.
async fn stored_workdir(database: &Path, key: &str) -> Option<String> {
    if !database.exists() {
        return None;
    }
    let store = SqliteStore::open(database).ok()?;
    let sessions = store.sessions().await.ok()?;
    sessions
        .into_iter()
        .find(|session| session.key == key)
        .and_then(|session| session.workdir)
        .filter(|dir| !dir.trim().is_empty())
}

/// Continues a session named on the command line.
///
/// The name is required: without one there is nothing to continue, and the
/// caller that has no name is `open_session`, which asks the startup flow
/// instead.
async fn resume_named(
    config: Config,
    catalog: Catalog,
    key: String,
    mut args: ResumeArgs,
    verbosity: &str,
    config_path: Option<PathBuf>,
) -> Result<u8, String> {
    args.session = Some(key);
    resume_session(config, catalog, args, verbosity, config_path).await
}

#[allow(clippy::too_many_lines)]
async fn resume_session(
    config: Config,
    catalog: Catalog,
    args: ResumeArgs,
    verbosity: &str,
    config_path: Option<PathBuf>,
) -> Result<u8, String> {
    let session_key = args
        .session
        .clone()
        .ok_or("resume needs a session key; run `norte-code` to pick one")?;
    // A session is about a tree, and the tools re-root at the process's own
    // directory. Resuming one from somewhere else pointed every read, write and
    // command at a tree the work was not about, which is a session that looks
    // resumed and answers about the wrong files. The directory it was stored
    // with is where it goes back to.
    //
    // A session stored before the field existed has none, and the directory the
    // command was typed in stands — which is what `resume` always did.
    if let Some(dir) = stored_workdir(&database_path()?, &session_key).await {
        let here = std::env::current_dir().ok();
        let there = std::fs::canonicalize(&dir).ok();
        let elsewhere = match (&here, &there) {
            (Some(here), Some(there)) => here != there,
            _ => false,
        };
        if elsewhere {
            if there.is_some() {
                enter_workdir(Path::new(&dir))?;
                eprintln!("{}", crate::style::Style::stderr().dim(format!("in {dir}")));
            } else {
                eprintln!(
                    "warning: this session worked in {dir}, which is gone; \
                     staying in {}",
                    here.as_ref().map_or_else(
                        || "the current directory".to_owned(),
                        |here| here.display().to_string()
                    )
                );
            }
        }
    }
    let mut config = config;
    CommonOptions {
        max_iterations: args.max_iterations,
        compaction_threshold: args.compaction_threshold,
        sandbox: &args.sandbox,
        extensions: &args.extension,
        interactive: args.interactive,
        jsonl: args.jsonl,
    }
    .apply(&mut config)?;
    let (system_prompt_source, system_prompt) =
        select_system_prompt(args.system_prompt, config.system_prompt.clone());
    let spec = resolve_spec(&config, &catalog, None, None, None, None)?;
    let model_label = spec.label();
    let provider = spec.provider.clone();
    let effective = effective_reasoning(
        None,
        &config.reasoning.default,
        &spec.capabilities.reasoning,
    )
    .map_err(|error| error.to_string())?;
    let style = if args.jsonl {
        Style::plain()
    } else {
        Style::stderr()
    };
    let verbosity = Verbosity::parse(verbosity)?;
    if verbosity.at_least(Verbosity::Medium) && !args.jsonl {
        eprintln!(
            "{} {} {} {}",
            style.magenta("◆"),
            style.bold(format!("resume {}", short(&session_key))),
            style.dim("·"),
            style.dim(format!("catalogue: {} models", catalog.model_count()))
        );
    }
    let budget_is_usd = args.budget.is_some() && args.budget_unit.eq_ignore_ascii_case("usd");
    if let Some(warning) = budget_warning(&provider, &config, budget_is_usd) {
        if verbosity.at_least(Verbosity::Medium) && !args.jsonl {
            eprintln!("{}", style.red(warning));
        }
    }
    let show_cost = config.cost.show;
    // The footer's context dial needs the window and the compaction threshold
    // before the config and spec are moved into the service.
    let max_context = spec.capabilities.max_context;
    let compaction_threshold = config.compaction.threshold;
    let database = database_path()?;
    let store = Arc::new(
        SqliteStore::open(&database)
            .map_err(|error| format!("could not open {}: {error}", database.display()))?,
    );
    let session_store: Arc<dyn norte::harness::Store> = store.clone();
    let blobs: Arc<dyn norte::BlobStore> = store.clone();
    let directory: Arc<dyn SessionDirectory> = store;
    // Read before the config moves into the service: the renderer draws the diff
    // and the configuration is what says in which shape.
    let diff_style = config.ui.diff_style;
    let service = Arc::new(
        Service::with_directory(
            config,
            Arc::new(catalog),
            spec,
            Arc::new(VendorModels),
            session_store,
            blobs,
            directory,
            system_prompt,
        )
        .await
        .map_err(|e| e.to_string())?,
    );
    service.set_config_path(config_path);
    let pinned = Pinned::new();
    let mut renderer = Renderer::new(
        if args.jsonl {
            Format::Jsonl
        } else {
            Format::Text
        },
        verbosity,
        show_cost,
        style,
        Arc::clone(&pinned),
    );
    renderer.set_cost_silent(matches!(provider.as_str(), "anthropic" | "google"));
    renderer.set_diff_style(diff_style);
    // What the plugins draw, asked for once: the columns and the status field
    // are theirs, and the frame draws what it was given.
    renderer.sidebar = crate::plugins::all(None)
        .sidebar()
        .into_iter()
        .map(|(name, rows)| (name.to_owned(), rows))
        .collect();
    renderer.model_name = Some(model_label);
    renderer.max_context = max_context;
    renderer.compaction_threshold = compaction_threshold;
    // The connected MCP servers, for the sidebar. Read once here, and again
    // after every `/refresh`: those are the moments the set can change.
    renderer.set_mcp(crate::interaction::mcp_rows(&service));
    renderer.host_facts = Some(HostFacts {
        database: database.clone(),
        providers_path: norte_code_protocol::catalog::providers_path().unwrap_or_default(),
        models_path: norte_code_protocol::catalog::models_path().unwrap_or_default(),
        prompt_source: system_prompt_source,
    });
    renderer.set_reasoning(reasoning_setting_label(&effective));
    let session = SessionKey::new(session_key);
    // Hand the service and session to plugins so they can reach them when they
    // run a command or redraw. Plugins that do not override set_context ignore it.
    let plugin_ctx = PluginContext {
        session_key: Some(session.clone()),
        service: Some(Arc::clone(&service) as Arc<dyn Any + Send + Sync>),
    };
    renderer.sidebar = crate::plugins::all(Some(&plugin_ctx))
        .sidebar()
        .into_iter()
        .map(|(name, rows)| (name.to_owned(), rows))
        .collect();
    // The session may carry a reasoning override of its own — `/reasoning max`
    // leaves one — and that is what the next turn sends. The line above is the
    // configuration's answer, which is what a session with no override is on;
    // this is the session's, which is what the footer has to show.
    refresh_effort(&service, &session, &mut renderer);
    // The catalogue names the completion offers: `/provider` and `/model` take
    // one, and the list lives here rather than in the registry, which cannot
    // read the catalogue and is built before this point.
    crate::interaction::refresh_completions(&service, Some(&session), &mut renderer);
    let interactive = args.interactive;
    // The terminal belongs to the renderer, exactly as it does for a new
    // session: it is what knows when something changed, so it is what draws,
    // and the editor reads its keys from the raw mode this sets. Without it a
    // resume never entered the alternate screen or raw mode, so `draw` returned
    // early and the reader blocked on a keystroke in canonical mode: the
    // interface stayed on the terminal, the input was never cleared and the
    // session could not be continued. A resume with no terminal (`--jsonl`, a
    // pipe) keeps the plain path and draws nothing.
    if interactive && crate::terminal::is_interactive() {
        renderer.screen = Some(crate::terminal::Guard::enter()?);
    }
    let mut input = if interactive {
        Some(InteractiveInput::start(Arc::clone(&pinned))?)
    } else {
        None
    };
    if let Some(limit) = args.budget {
        if !limit.is_finite() || limit <= 0.0 {
            return Err(format!("--budget must be a positive number, got {limit}"));
        }
        let unit = parse_budget_unit(&args.budget_unit)?;
        let action = parse_budget_action(&args.budget_action)?;
        let stream = service
            .submit(ClientRequest {
                request_id: 1,
                kind: ClientRequestKind::SetBudget {
                    session: session.clone(),
                    budget: Budget {
                        unit,
                        limit,
                        action,
                    },
                },
            })
            .await
            .map_err(|e| e.to_string())?;
        drive_turns(&service, stream, &mut renderer, style, Some(session.clone()), input.as_mut(), Some(plugin_ctx.clone())).await?;
    }
    // The override goes through the same request the interactive `/model` sends,
    // so the session's own rules judge it and a model it refuses is reported.
    if let Some(model) = args.model.clone() {
        let stream = service
            .submit(ClientRequest {
                request_id: 2,
                kind: ClientRequestKind::SetModel {
                    session: session.clone(),
                    model,
                },
            })
            .await
            .map_err(|e| e.to_string())?;
        drive_turns(&service, stream, &mut renderer, style, Some(session.clone()), input.as_mut(), Some(plugin_ctx.clone())).await?;
    }
    renderer.mark_working();
    let stream = service
        .submit(ClientRequest {
            request_id: 2,
            kind: ClientRequestKind::Resume {
                session: session.clone(),
            },
        })
        .await
        .map_err(|e| e.to_string())?;
    let outcome = drive_turns(&service, stream, &mut renderer, style, Some(session.clone()), input.as_mut(), Some(plugin_ctx.clone())).await?;
    renderer.ensure_newline();
    if interactive && !outcome.parked && !renderer.budget_stopped {
        if let Some(receiver) = input.as_mut() {
            interactive_loop(
                &service,
                Some(session),
                &mut renderer,
                style,
                receiver,
                None,
                None,
                Some(plugin_ctx),
            )
            .await?;
        }
    }
    renderer.ensure_newline();
    renderer.summary();
    Ok(u8::from(renderer.failed))
}

/// Runs the startup flow and opens what it chose.
///
/// This is where a bare `norte-code`, and `resume` with no key, arrive. The flow
/// runs first because the model has to be settled before the service is built —
/// the service is built around a spec, and there is no spec without a model —
/// while the session itself may come after: `new_session` creates one, and a key
/// goes to the resume path.
async fn open_session(
    config: Config,
    catalog: Catalog,
    mut args: ResumeArgs,
    verbosity: &str,
    config_path: Option<PathBuf>,
) -> Result<u8, String> {
    let style = Style::stderr();
    if !startup::has_terminal() {
        return Err(
            "the startup flow asks which session to open, so it needs a terminal; \
             pass a session key (`norte-code resume <KEY>`) or name a model \
             (`norte-code new --provider PROVIDER --model MODEL`) instead"
                .to_owned(),
        );
    }
    let mut opening = config.clone();
    let chosen = startup::startup(
        &mut opening,
        &catalog,
        &database_path()?,
        config_path.as_deref(),
        style,
    )
    .await?;
    match chosen.open {
        // A key is resumed on the path a named session always takes, which
        // replays the history and makes no request while the turn is over.
        Open::Existing { key, .. } => {
            args.session = Some(key.as_str().to_owned());
            resume_session(config, catalog, args, verbosity, config_path).await
        }
        // A new session is created and read, never resumed: it holds no answer
        // yet, and a resume of that is an empty turn the engine answers by
        // asking the model.
        Open::New => {
            new_session(
                config,
                catalog,
                chosen.model,
                NewArgs::bare(),
                verbosity,
                config_path,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests;
