//! The headless `norte-code` command.
//!
//! `norte-code run "prompt"` sends one message to the configured RAEP service
//! and renders the returned events. With `--interactive` the same remote
//! session stays open and reads the next message from the terminal. Text goes
//! to stdout; diagnostics, reasoning and tool activity go to stderr.
//! `--jsonl` writes one protocol event per line on stdout instead, for scripts
//! and tests.
//!
//! `norte-code models` lists what the catalogue knows, filtered by provider,
//! tool calling or reasoning.

mod app;
mod art;
mod commands;
mod commands_registry;
mod input;
mod installs;
mod interaction;
mod markdown;
mod modal;
mod models;
mod plugins;
mod preview;
mod prompt;
mod remote_state;
mod render;
mod select;
mod startup;
mod style;
mod telemetry_panel;
mod terminal;
mod text;
mod transcript;
mod ui;

use commands::{apply_extensions, duration_label, run};
use input::{InteractiveInput, Pinned};
use interaction::{drive_turns, headless_messages, interactive_loop, refresh_effort};
use models::{ModelChoice, list_models, remote_model_list, resolve_remote_spec, session_choice};
use preview::{clip_text, compact_json};
use render::{
    Format, HostFacts, Renderer, Verbosity, cache_percent_label, human_tokens, parse_budget_action,
    parse_budget_unit, parse_reasoning_setting, reasoning_label, reasoning_setting_label,
    select_system_prompt, short,
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

use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use futures_util::StreamExt;
use norte::{
    CacheMode, Feature, Modality, ReasoningEffort, StreamingGranularity, StructuredOutputMode,
    ToolKind,
};
use norte_code_client::{AgentClient, EventStream, RemoteClient};
use norte_code_protocol::ModelInfo as ModelSpec;
use norte_code_protocol::catalog::{Catalog, WireSchema};
use norte_code_protocol::config::{Config, DiffStyle};
use norte_code_protocol::costs::{CostSummary, by_day, by_session, by_turn, summarize};
use norte_code_protocol::skills::{SkillRegistry, discovery_roots};
#[cfg(test)]
use norte_code_protocol::sqlite::StoredSession;
use norte_code_protocol::{
    Budget, BudgetAction, BudgetScope, BudgetUnit, ClientRequest, ClientRequestKind, Delta,
    ErrorCode, JobInfo, MessageRole, ModelInfo, OutputStream, Phase, Reasoning, ReasoningInfo,
    ReasoningScope, ServerEvent, ServerEventKind, SessionInfo, SessionKey,
};
use tokio::sync::mpsc;

use crate::remote_state::{ModelInfoExt, RemoteState, effective_reasoning};
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
    /// The client configuration file.
    #[arg(long = "cli-config", global = true, value_name = "PATH")]
    config_path: Option<PathBuf>,
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
    /// Manage the installed plugins.
    Plugins(PluginsArgs),
    /// Manage or inspect the RAEP service.
    Service(ServiceArgs),
}

#[derive(Args)]
struct ServiceArgs {
    /// The local-service operation.
    #[command(subcommand)]
    command: ServiceCommand,
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Start the configured local service.
    Start {
        /// Keep the terminal attached to the service log until Ctrl-C.
        #[arg(short = 'f', long)]
        follow: bool,
    },
    /// Gracefully stop the configured local service.
    Stop,
    /// Restart the configured local service.
    Restart,
    /// Show local process or remote endpoint status.
    Status,
    /// Query the server-owned model catalogue.
    Catalog {
        /// Emit one JSON object per model.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct McpArgs {
    /// What to do with the configured MCP servers.
    #[command(subcommand)]
    command: McpCommand,
}

/// The flags that shape the plugin commands.
#[derive(Args)]
struct PluginsArgs {
    /// What to do with the installed plugins.
    #[command(subcommand)]
    command: PluginsCommand,
}

/// What the plugin commands do.
///
/// The same operations the chat offers — `/plugins list`, `/plugins add` and
/// `/plugins rm` — so a plugin can be installed from a shell, where a build
/// that takes minutes is something a person can watch, and not only from
/// inside a session that is waiting for it.
#[derive(Subcommand)]
enum PluginsCommand {
    /// List the installed plugins.
    List,
    /// Install one plugin package from a git URL.
    Add {
        /// The git URL of the plugin's repository.
        url: String,
        /// Select a package from a multi-plugin Cargo workspace.
        #[arg(long)]
        package: Option<String>,
        /// Install every plugin package in the workspace.
        #[arg(long, conflicts_with = "package")]
        all: bool,
    },
    /// Remove an installed plugin.
    #[command(alias = "remove")]
    Rm {
        /// The name of the plugin, as `list` shows it.
        name: String,
    },
    /// Rebuild installed plugins from their source; with a name, just that one.
    Update {
        /// The plugin to update, as `list` shows it. Omitted, every plugin.
        name: Option<String>,
    },
    /// Load an installed plugin again.
    Enable {
        /// The name of the plugin, as `list` shows it.
        name: String,
    },
    /// Stop loading an installed plugin.
    Disable {
        /// The name of the plugin, as `list` shows it.
        name: String,
    },
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
    /// Write one JSON protocol event per line and avoid the TUI.
    #[arg(long)]
    jsonl: bool,
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
            jsonl: false,
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
/// Parses the wire spelling accepted by the command line for compatibility.
/// Remote sessions apply the wire on the server.
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
    .any(|budget| budget.unit == BudgetUnit::Usd);
    (budget_is_usd || configured).then(|| {
        format!(
            "warning: {provider} does not report cost, so a USD budget cannot be measured; \
             use --budget-unit tokens or requests"
        )
    })
}

pub(crate) fn remote_client(config: &Config) -> Result<RemoteClient, String> {
    RemoteClient::new(
        &config.ui.service_url,
        config
            .ui
            .service_auth
            .as_deref()
            .filter(|token| !token.trim().is_empty()),
        None,
    )
    .map(|client| client.with_agent_id(config.ui.agent_id.clone()))
    .map_err(|error| error.to_string())
}

async fn remote_session(client: &dyn AgentClient, key: &str) -> Result<SessionInfo, String> {
    let mut stream = client
        .submit(ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::ListSessions,
        })
        .await
        .map_err(|error| error.to_string())?;
    let mut sessions = Vec::new();
    while let Some(event) = stream.next().await {
        match event.kind {
            ServerEventKind::SessionList { sessions: listed } => sessions.extend(listed),
            ServerEventKind::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
    if let Some(session) = sessions
        .iter()
        .find(|session| session.session.as_str() == key)
    {
        return Ok(session.clone());
    }
    let mut matches = sessions
        .into_iter()
        .filter(|session| session.session.as_str().starts_with(key));
    let Some(session) = matches.next() else {
        return Err(format!("no remote session matches `{key}`"));
    };
    if matches.next().is_some() {
        return Err(format!("more than one remote session matches `{key}`"));
    }
    Ok(session)
}

#[cfg(test)]
fn matching_stored_session(sessions: Vec<StoredSession>, key: &str) -> Option<StoredSession> {
    if let Some(session) = sessions.iter().find(|session| session.key == key) {
        return Some(session.clone());
    }
    let mut matches = sessions
        .into_iter()
        .filter(|session| session.key.starts_with(key));
    let session = matches.next()?;
    matches.next().is_none().then_some(session)
}

async fn create_session(client: &dyn AgentClient) -> Result<SessionKey, String> {
    let mut stream = client
        .submit(ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::CreateSession { title: None },
        })
        .await
        .map_err(|error| error.to_string())?;
    while let Some(event) = stream.next().await {
        match event.kind {
            ServerEventKind::SessionCreated { .. } => {
                return event
                    .session
                    .ok_or_else(|| "the service did not return the new session".to_owned());
            }
            ServerEventKind::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
    Err("the service did not return the new session".to_owned())
}

/// Starts a new session and reads it from the terminal.
///
/// `choice` is the provider and the model the startup flow chose, when it had
/// to choose them; `args` carries what the command line said. The flags win,
/// which is what makes `norte-code new --model openai/gpt-5` open on that
/// model while a bare `norte-code` opens on `default_model`: the flag is an
/// override, not a requirement.
///
/// The pair is never joined: the provider is its own key and the model id may
/// carry slashes of its own, so the two travel as they are. A choice that came
/// from the flow is the same one the flow wrote into the configuration, so both
/// agree; the pair is what carries the choice when there was nowhere to write
/// it down.
#[allow(clippy::too_many_lines)]
pub(crate) async fn new_session(
    config: Config,
    catalog: Catalog,
    choice: Option<ModelChoice>,
    args: NewArgs,
    verbosity: &str,
    config_path: Option<PathBuf>,
) -> Result<u8, String> {
    if args.endpoint.is_some() || args.wire.is_some() {
        return Err(
            "--endpoint and --wire are server configuration; set them in service.json on the host"
                .to_owned(),
        );
    }
    let mut config = config;
    let (mut provider, mut model) =
        session_choice(choice.as_ref(), args.provider.clone(), args.model.clone());
    if provider.is_none() || model.is_none() {
        let client = remote_client(&config)?;
        let info = client.info().await.map_err(|error| error.to_string())?;
        if let Some(default_model) = info
            .get("default_model")
            .and_then(serde_json::Value::as_object)
        {
            provider = default_model
                .get("provider")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            model = default_model
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
        }
    }
    // Defaults and catalogue entries belong to the server. The CLI only
    // forwards explicit overrides and asks the server for their metadata.
    let effective_provider = provider.clone().filter(|p| !p.is_empty());
    let effective_model = model.clone().filter(|m| !m.is_empty());

    // No model is configured: do not fail here. Reach the interface and let the
    // person pick one with `/models`; a message sent in the meantime is refused
    // with a pointer to that command. A placeholder spec is built from the
    // first catalogue entry so the service can exist, and its name is not shown.
    let model_unset = effective_provider.is_none() || effective_model.is_none();
    let spec: Option<ModelSpec> = if model_unset {
        None
    } else {
        let client = remote_client(&config)?;
        let models = remote_model_list(&client).await?;
        Some(resolve_remote_spec(
            &models,
            effective_provider.as_deref().unwrap_or_default(),
            effective_model.as_deref().unwrap_or_default(),
        )?)
    };
    let model_label = spec.as_ref().map(ModelInfoExt::label);
    let provider = spec
        .as_ref()
        .map(|s| s.provider.clone())
        .unwrap_or_default();
    if let Some(spec) = &spec {
        config.default_provider = Some(spec.provider.clone());
        config.default_model = Some(spec.model.clone());
    }
    CommonOptions {
        max_iterations: args.max_iterations,
        compaction_threshold: args.compaction_threshold,
        sandbox: &args.sandbox,
        extensions: &args.extension,
        interactive: !args.jsonl,
        jsonl: args.jsonl,
    }
    .apply(&mut config)?;
    let (system_prompt_source, _system_prompt) =
        select_system_prompt(args.system_prompt, config.system_prompt.clone());
    let reasoning = args
        .reasoning
        .as_deref()
        .map(parse_reasoning_setting)
        .transpose()?;
    let effective = if let Some(spec) = &spec {
        effective_reasoning(
            reasoning.as_ref(),
            &config.reasoning.default,
            &spec.reasoning,
        )
        .map_err(|error| error.clone())?
    } else {
        norte_code_protocol::Reasoning::Automatic
    };
    if let Some(limit) = args.budget
        && (!limit.is_finite() || limit <= 0.0)
    {
        return Err(format!("--budget must be a positive number, got {limit}"));
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
        if model_unset {
            eprintln!(
                "{} {} {} {}",
                style.magenta("◆"),
                style.bold("no model configured"),
                style.dim("·"),
                style.dim(format!("catalogue: {} models", catalog.model_count()))
            );
        } else if let Some(spec) = &spec {
            eprintln!(
                "{} {} {} {}",
                style.magenta("◆"),
                style.bold(spec.label()),
                style.dim("·"),
                style.dim(format!("catalogue: {} models", catalog.model_count()))
            );
        }
    }
    let budget_is_usd = message_budget
        .as_ref()
        .is_some_and(|budget| budget.unit == BudgetUnit::Usd);
    if let Some(warning) = budget_warning(&provider, &config, budget_is_usd)
        && verbosity.at_least(Verbosity::Medium)
    {
        eprintln!("{}", style.red(warning));
    }
    let show_cost = config.cost.show;
    // The footer's context dial needs the window and the compaction threshold
    // before the config and spec are moved into the service.
    let max_context = spec.as_ref().and_then(|s| s.max_context);
    let compaction_threshold = config.compaction.threshold;
    let diff_style = config.ui.diff_style;
    let client = Arc::new(remote_client(&config)?);
    let remote_models = remote_model_list(client.as_ref()).await?;
    let remote_model_count = remote_models.len();
    let state = RemoteState::new(
        Arc::clone(&client),
        remote_models,
        config.clone(),
        config_path,
    );
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
    let diagnostic_model = model_label.clone();
    renderer.set_cost_silent(matches!(provider.as_str(), "anthropic" | "google"));
    renderer.set_diff_style(diff_style);
    renderer.set_chart_config(&config.ui.charts);
    renderer.model_name = model_label;
    renderer.model_unset = model_unset;
    renderer.max_context = max_context;
    renderer.compaction_threshold = compaction_threshold;
    renderer.diagnostic(
        "connected",
        &serde_json::json!({
            "service_url": config.ui.service_url,
            "model": diagnostic_model,
            "catalogue_models": remote_model_count,
        }),
    );
    // The facts `/debug` reports that only the CLI knows: which file the
    // catalogue came from, the database it opened and where the system prompt
    // came from. Set once here, like the model name and the context window.
    renderer.host_facts = Some(HostFacts {
        database: PathBuf::from("(remote host)"),
        providers_path: PathBuf::from("(remote host)"),
        models_path: PathBuf::from("(remote host)"),
        prompt_source: system_prompt_source,
    });
    // The effort the next turn will ask for, on the footer where the session
    // and the totals are: nothing else on screen says it, and after a resume it
    // is not the flag that was passed but what the config and the model resolve.
    renderer.set_reasoning(reasoning_setting_label(&effective));
    // The catalogue names the completion offers. A new session has no key yet,
    // so only the provider list is offered until one exists: `/model` needs a
    // session to know which provider's models to list.
    crate::interaction::refresh_completions(&state, None, &mut renderer);
    // The terminal belongs to the renderer: it is what knows when something
    // changed, so it is what draws. A process with no terminal keeps the
    // plain-text path and draws nothing.
    if !args.jsonl && crate::terminal::is_interactive() {
        renderer.screen = Some(crate::terminal::Guard::enter()?);
        renderer.set_input_ready(false);
        renderer.mark_working();
        renderer.notice(format!("⟳ connecting to {}", config.ui.service_url));
        renderer.draw()?;
    }
    // The session exists before the first line is read, so the line's message
    // names it: a session created as a side effect of a message cannot be shown
    // in the footer before the message is sent.
    renderer.diagnostic("session_create_started", &serde_json::json!({}));
    let session = match create_session(client.as_ref()).await {
        Ok(session) => session,
        Err(error) => {
            renderer.mark_ready();
            renderer.failure(format!("✗ connection failed: {error}"));
            renderer.draw()?;
            return Err(error);
        }
    };
    renderer.diagnostic(
        "session_created",
        &serde_json::json!({
            "session": session,
        }),
    );
    if let (Some(provider), Some(model)) = (effective_provider, effective_model) {
        let events = match client
            .submit(ClientRequest {
                request_id: 1,
                kind: ClientRequestKind::SetModel {
                    session: session.clone(),
                    provider,
                    model,
                },
            })
            .await
        {
            Ok(stream) => stream.collect::<Vec<_>>().await,
            Err(error) => {
                renderer.mark_ready();
                renderer.failure(format!("✗ connection failed: {error}"));
                renderer.draw()?;
                return Err(error.to_string());
            }
        };
        if let Some(message) = events.into_iter().find_map(|event| match event.kind {
            ServerEventKind::Error { message, .. } => Some(message),
            _ => None,
        }) {
            renderer.diagnostic(
                "session_configuration_failed",
                &serde_json::json!({
                    "operation": "set_model",
                    "error": message,
                }),
            );
            renderer.mark_ready();
            renderer.failure(format!("✗ connection failed: {message}"));
            renderer.draw()?;
            return Err(format!("could not configure remote session: {message}"));
        }
        renderer.diagnostic(
            "session_configured",
            &serde_json::json!({
                "operation": "set_model",
            }),
        );
    }
    if verbosity.at_least(Verbosity::Medium) && !args.jsonl {
        // The alternate screen owns the terminal once Guard has entered:
        // a bare write here lands at the cursor and the next frame's diff
        // never repaints it, which is what left fragments doubled on
        // screen. With a screen up the line goes through the transcript;
        // without one (pipe, headless) the plain write stands.
        if renderer.screen.is_some() {
            renderer.notice(format!("session {}", short(session.as_str())));
        } else {
            eprintln!(
                "{} {}",
                style.dim("session"),
                style.faint(short(session.as_str()).to_owned())
            );
        }
    }
    // Hand the service and session to the plugins so they can reach them when
    // they run a command or redraw. The plugins were loaded before the
    // catalogue, because what they offer belongs in it; the handle they were
    // given is the same one, and it now knows where to look.
    crate::plugins::attach(Some(session.clone()));
    if !args.jsonl && crate::terminal::is_interactive() {
        renderer.mark_ready();
        renderer.notice(format!("✓ connected to {}", config.ui.service_url));
        renderer.set_input_ready(true);
        renderer.draw()?;
    }
    let mut input = if args.jsonl {
        None
    } else {
        Some(InteractiveInput::start(Arc::clone(&pinned))?)
    };
    if args.jsonl {
        use std::io::Read as _;
        let mut text = String::new();
        if let Err(error) = std::io::stdin().read_to_string(&mut text) {
            let message = format!("could not read headless input: {error}");
            renderer.diagnostic(
                "client_failed",
                &serde_json::json!({
                    "stage": "input",
                    "error": message,
                }),
            );
            return Err(message);
        }
        renderer.diagnostic(
            "input_stream_finished",
            &serde_json::json!({
                "messages": text.lines().count(),
                "bytes": text.len(),
            }),
        );
        if let Err(error) = headless_messages(
            &state,
            client.as_ref(),
            session.clone(),
            &mut renderer,
            style,
            text.lines().map(str::to_owned),
        )
        .await
        {
            renderer.diagnostic(
                "client_failed",
                &serde_json::json!({
                    "stage": "request",
                    "error": error,
                }),
            );
            return Err(error);
        }
        renderer.diagnostic("client_finished", &serde_json::json!({}));
    } else if let Some(receiver) = input.as_mut() {
        interactive_loop(
            &state,
            client.as_ref(),
            Some(session),
            &mut renderer,
            style,
            receiver,
            message_budget,
            reasoning,
        )
        .await?;
    }
    renderer.ensure_newline();
    renderer.summary();
    Ok(u8::from(renderer.failed))
}

/// The row a stored session left behind.
///
/// It reads the row before the service is built, because two things have to be
/// settled first: the directory the tools are rooted at — a service built in the
/// wrong tree has already decided what its tools point at — and the provider and
/// the model the session was on, which are the session's own and not whatever
/// the configuration names now.
///
/// A database that cannot be read is not a reason to refuse the resume: the
/// session is usable here, and the directory is where it would have gone.
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
    let mut config = config;
    let client = Arc::new(remote_client(&config)?);
    let requested_key = args
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
    let stored = remote_session(client.as_ref(), &requested_key).await?;
    let session_key = stored.session.as_str().to_owned();
    CommonOptions {
        max_iterations: args.max_iterations,
        compaction_threshold: args.compaction_threshold,
        sandbox: &args.sandbox,
        extensions: &args.extension,
        interactive: args.interactive,
        jsonl: args.jsonl,
    }
    .apply(&mut config)?;
    let (system_prompt_source, _system_prompt) =
        select_system_prompt(args.system_prompt, config.system_prompt.clone());
    // A session resumes on the provider and the model it was saved with: the
    // configuration may have been changed since, and a resume that silently
    // moved the session to another model is not a resume. The two travel as
    // the pair they are — a model id may carry slashes of its own.
    //
    // A row written before the provider had a column of its own has none, and
    // so does a pair the catalogue does not carry anymore: the configuration's
    // answer stands, which is what a session on a provider the catalogue lost
    // does.
    let saved = stored.provider.clone().zip(stored.model.clone());
    let saved_provider = saved.as_ref().map(|(p, _)| p.clone());
    let saved_model = saved.as_ref().map(|(_, m)| m.clone());
    let effective_provider = saved_provider.clone().filter(|p| !p.is_empty());
    let effective_model = saved_model.clone().filter(|m| !m.is_empty());

    let model_unset = effective_provider.is_none() || effective_model.is_none();

    let remote_models = remote_model_list(client.as_ref()).await?;
    let spec: Option<ModelSpec> = if model_unset {
        None
    } else {
        Some(resolve_remote_spec(
            &remote_models,
            effective_provider.as_deref().unwrap_or_default(),
            effective_model.as_deref().unwrap_or_default(),
        )?)
    };
    let model_label = spec.as_ref().map(ModelInfoExt::label);
    let provider = spec
        .as_ref()
        .map(|s| s.provider.clone())
        .unwrap_or_default();
    let effective = if let Some(spec) = &spec {
        effective_reasoning(None, &config.reasoning.default, &spec.reasoning)
            .map_err(|error| error.clone())?
    } else {
        norte_code_protocol::Reasoning::Automatic
    };
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
    if let Some(warning) = budget_warning(&provider, &config, budget_is_usd)
        && verbosity.at_least(Verbosity::Medium)
        && !args.jsonl
    {
        eprintln!("{}", style.red(warning));
    }
    let show_cost = config.cost.show;
    // The footer's context dial needs the window and the compaction threshold
    // before the config and spec are moved into the service.
    let max_context = spec.as_ref().and_then(|s| s.max_context);
    let compaction_threshold = config.compaction.threshold;
    let diff_style = config.ui.diff_style;
    let state = RemoteState::new(
        Arc::clone(&client),
        remote_models,
        config.clone(),
        config_path,
    );
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
    renderer.set_chart_config(&config.ui.charts);
    renderer.model_name = model_label;
    renderer.model_unset = model_unset;
    renderer.max_context = max_context;
    renderer.compaction_threshold = compaction_threshold;
    renderer.host_facts = Some(HostFacts {
        database: PathBuf::from("(remote host)"),
        providers_path: PathBuf::from("(remote host)"),
        models_path: PathBuf::from("(remote host)"),
        prompt_source: system_prompt_source,
    });
    renderer.set_reasoning(reasoning_setting_label(&effective));
    let session = SessionKey::new(session_key);
    // Hand the service and session to the plugins so they can reach them when
    // they run a command or redraw. The plugins were loaded before the
    // catalogue, because what they offer belongs in it; the handle they were
    // given is the same one, and it now knows where to look.
    crate::plugins::attach(Some(session.clone()));
    // The session may carry a reasoning override of its own — `/reasoning max`
    // leaves one — and that is what the next turn sends. The line above is the
    // configuration's answer, which is what a session with no override is on;
    // this is the session's, which is what the footer has to show.
    refresh_effort(&state, &session, &mut renderer);
    // The catalogue names the completion offers: `/provider` and `/model` take
    // one, and the list lives here rather than in the registry, which cannot
    // read the catalogue and is built before this point.
    crate::interaction::refresh_completions(&state, Some(&session), &mut renderer);
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
        let stream = client
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
        drive_turns(
            &state,
            client.as_ref(),
            stream,
            &mut renderer,
            style,
            Some(session.clone()),
            input.as_mut(),
        )
        .await?;
    }
    // The override goes through the same request the interactive `/model` sends,
    // so the session's own rules judge it and a model it refuses is reported.
    // The provider is the session's own: a resume names a model, not a pair.
    if let Some(model) = args.model.clone() {
        let stream = client
            .submit(ClientRequest {
                request_id: 2,
                kind: ClientRequestKind::SetModel {
                    session: session.clone(),
                    provider: provider.clone(),
                    model,
                },
            })
            .await
            .map_err(|e| e.to_string())?;
        drive_turns(
            &state,
            client.as_ref(),
            stream,
            &mut renderer,
            style,
            Some(session.clone()),
            input.as_mut(),
        )
        .await?;
    }
    renderer.mark_working();
    let stream = client
        .submit(ClientRequest {
            request_id: 2,
            kind: ClientRequestKind::Resume {
                session: session.clone(),
                replay: true,
            },
        })
        .await
        .map_err(|e| e.to_string())?;
    let outcome = drive_turns(
        &state,
        client.as_ref(),
        stream,
        &mut renderer,
        style,
        Some(session.clone()),
        input.as_mut(),
    )
    .await?;
    renderer.ensure_newline();
    if interactive
        && !outcome.parked
        && !renderer.budget_stopped
        && let Some(receiver) = input.as_mut()
    {
        interactive_loop(
            &state,
            client.as_ref(),
            Some(session),
            &mut renderer,
            style,
            receiver,
            None,
            None,
        )
        .await?;
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
///
/// The configuration the flow settles is the one the session is built from: the
/// flow writes the choice it made into the config, and a clone of the config
/// thrown away afterwards left the run with the defaults it had before the
/// question — which is how a first run that had just named a provider died on
/// "no provider specified".
#[allow(clippy::large_futures)]
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
    let mut config = config;
    let client = remote_client(&config)?;
    let chosen = startup::startup(
        &mut config,
        &catalog,
        &client,
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
                chosen.choice,
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
