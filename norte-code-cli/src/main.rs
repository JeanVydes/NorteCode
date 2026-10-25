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

mod approval;
mod commands;
mod input;
mod interaction;
mod markdown;
mod models;
mod prompt;
mod render;
mod style;
mod text;

use approval::{clip_text, compact_json};
use commands::{apply_extensions, database_path, duration_label, run};
use input::{InteractiveInput, Pinned};
use interaction::{drive_turns, echo_user_message, interactive_loop};
use models::{list_models, resolve_spec};
use render::{
    Format, Renderer, Verbosity, approval_label, cache_percent_label, debug_mcp, debug_model,
    debug_prompt_size, debug_reasoning, debug_start, fallback_env, find_project_config,
    human_tokens, parse_budget_action, parse_budget_unit, parse_reasoning_setting, reasoning_label,
    select_system_prompt, short, source_label, token_ratio,
};

#[cfg(test)]
use approval::{
    ApprovalChoice, PreviewLine, approval_request, argument_preview, parse_approval_choice,
};
#[cfg(test)]
use commands::cost_summary;
#[cfg(test)]
use interaction::parse_approval_command;
#[cfg(test)]
use prompt::{compose_selection, parse_selection};
#[cfg(test)]
use render::budget_label;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use futures_util::StreamExt;
use norte::{
    CacheMode, Feature, Modality, ReasoningEffort, StreamingGranularity, StructuredOutputMode,
    ToolKind,
};
use norte_code_agent::{
    EventStream, ModelSpec, Service, ServiceError, VendorModels, effective_reasoning, model_list,
};
use norte_code_catalog::{Catalog, LoadOptions, Source, cache_dir, load_catalog};
use norte_code_config::{ApprovalMode, AuthKind, Config, WireKind, user_config_path};
use norte_code_cost::{CostSummary, by_day, by_session, by_turn, summarize};
use norte_code_protocol::{
    Budget, BudgetAction, BudgetScope, BudgetUnit, ClientRequest, ClientRequestKind, Delta,
    DenyScope, ErrorCode, GrantInfo, GrantScope, ModelInfo, Phase, Reasoning, ReasoningInfo,
    ReasoningScope, ServerEvent, ServerEventKind, SessionKey,
};
use norte_code_skills::{SkillRegistry, discovery_roots};
use norte_code_store_sqlite::{RequestRecord, SessionDirectory, SqliteStore};
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
    /// The user configuration file.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Never touch the network for the catalogue.
    #[arg(long, global = true)]
    offline: bool,
    /// Revalidate the catalogue with the network even when the cache is fresh.
    #[arg(long, global = true)]
    refresh: bool,
    /// How much of the conversation to show: `low` (the answer alone),
    /// `medium` (also that tools run and that the model is thinking) or `high`
    /// (the reasoning, what each tool was asked and what it returned).
    #[arg(long, global = true, value_name = "LEVEL", default_value = "medium")]
    verbose: String,
    /// Print startup details, model metadata, paths and extensions.
    #[arg(long, global = true)]
    debug: bool,
    /// What to do.
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Send one prompt and stream the answer.
    Run(RunArgs),
    /// Continue a parked, stopped or interrupted session.
    Resume(ResumeArgs),
    /// List the models the catalogue knows.
    Models(ModelsArgs),
    /// Inspect the installed Agent Skills.
    Skills(SkillsArgs),
    /// List the sessions stored on disk.
    Sessions(SessionsArgs),
    /// Report the provider-reported cost ledger.
    Cost(CostArgs),
    /// Report local request telemetry: counts, tokens, cache, tools and latency.
    Telemetry(TelemetryArgs),
}

#[derive(Args)]
// A command line has flags: clap needs each one as a bool field, and they are
// not a state machine to model. `SkillsConfig` carries the same allow.
#[allow(clippy::struct_excessive_bools)]
struct RunArgs {
    /// The prompt. With --interactive it may be omitted, so the session starts
    /// at the prompt.
    #[arg(required_unless_present = "interactive")]
    prompt: Option<String>,
    /// A `provider/model` reference (defaults to `default_model` in config).
    #[arg(long, short = 'm', value_name = "PROVIDER/MODEL")]
    model: Option<String>,
    /// The provider id (e.g. `openai`, `groq`). Overrides the provider part
    /// of `--model`. When `--model` omits the provider, this is required.
    #[arg(long, value_name = "PROVIDER")]
    provider: Option<String>,
    /// Replace the built-in system prompt for this run.
    #[arg(long, value_name = "TEXT")]
    system_prompt: Option<String>,
    /// A base URL override for the provider.
    #[arg(long, value_name = "URL")]
    endpoint: Option<String>,
    /// The wire to speak, overriding the catalogue and the config.
    #[arg(long, value_name = "WIRE", value_parser = parse_wire)]
    wire: Option<WireKind>,
    /// The credential scheme, overriding the wire's default.
    #[arg(long, value_name = "AUTH", value_parser = parse_auth)]
    auth: Option<AuthKind>,
    /// Read the key from this environment variable instead of the configured one.
    #[arg(long, value_name = "VAR")]
    api_key_env: Option<String>,
    /// Omit `prompt_cache_key` for providers that reject it.
    #[arg(long)]
    disable_prompt_cache_key: bool,
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
    /// What needs approval: `ask` (default), `auto-edit` or `yolo`.
    #[arg(long, value_name = "MODE", value_parser = ["ask", "auto-edit", "yolo"])]
    approval_mode: Option<String>,
    /// Stop after this many model iterations; unlimited by default.
    #[arg(long, value_name = "N")]
    max_iterations: Option<usize>,
    /// A vendor request extra as `KEY=JSON` (repeatable). For `OpenRouter`,
    /// `openai.chat:provider={"order":["deepinfra"]}` chooses the upstream.
    #[arg(long, value_name = "KEY=JSON")]
    extension: Vec<String>,
    /// Write one JSON event per line on stdout.
    #[arg(long)]
    jsonl: bool,
    /// Continue an existing session by key instead of creating one.
    #[arg(long, value_name = "KEY")]
    session: Option<String>,
    /// Keep the session open and read the next message after each turn.
    #[arg(long, short = 'i')]
    interactive: bool,
}

#[derive(Args)]
struct ResumeArgs {
    /// The session key to continue.
    session: String,
    /// Replace the built-in system prompt for this resume.
    #[arg(long, value_name = "TEXT")]
    system_prompt: Option<String>,
    /// A `provider/model` reference to resume with, replacing the one the
    /// session holds.
    #[arg(long, short = 'm', value_name = "PROVIDER/MODEL")]
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
    /// Write one JSON object per line.
    #[arg(long)]
    json: bool,
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
    match run(cli).await {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("norte-code: {}", text::terminal(&message));
            ExitCode::from(2)
        }
    }
}

/// Parses `--wire`, the wire a run speaks.
fn parse_wire(value: &str) -> Result<WireKind, String> {
    match value {
        "responses" => Ok(WireKind::Responses),
        "chat" => Ok(WireKind::Chat),
        "messages" => Ok(WireKind::Messages),
        "generate-content" => Ok(WireKind::GenerateContent),
        other => Err(format!(
            "unknown wire `{other}`; expected responses, chat, messages or generate-content"
        )),
    }
}

/// Parses `--auth`, the credential scheme a run uses.
fn parse_auth(value: &str) -> Result<AuthKind, String> {
    match value {
        "bearer" => Ok(AuthKind::Bearer),
        "x-api-key" => Ok(AuthKind::XApiKey),
        "x-goog-api-key" => Ok(AuthKind::XGoogApiKey),
        "disabled" => Ok(AuthKind::Disabled),
        other => Err(format!(
            "unknown auth `{other}`; expected bearer, x-api-key, x-goog-api-key or disabled"
        )),
    }
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

#[allow(clippy::too_many_lines)]
async fn run_prompt(
    config: Config,
    catalog: Catalog,
    source: Source,
    args: RunArgs,
    verbosity: &str,
    debug: bool,
    config_path: Option<PathBuf>,
) -> Result<u8, String> {
    let spec = resolve_spec(
        &config,
        &catalog,
        args.provider.clone(),
        args.model.clone(),
        args.endpoint.clone(),
        args.wire,
        args.auth,
        args.api_key_env.clone(),
        args.disable_prompt_cache_key,
    )?;
    let model_label = spec.label();
    let provider = spec.provider.clone();
    let mut config = config;
    if let Some(mode) = args.approval_mode.as_deref() {
        config.approval_mode = match mode {
            "ask" => ApprovalMode::Ask,
            "auto-edit" => ApprovalMode::AutoEdit,
            "yolo" => ApprovalMode::Yolo,
            other => return Err(format!("unknown approval mode `{other}`")),
        };
    }
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
    let style = if args.jsonl {
        Style::plain()
    } else {
        Style::stderr()
    };
    let verbosity = Verbosity::parse(verbosity)?;
    if verbosity.at_least(Verbosity::Medium) && !args.jsonl {
        eprintln!(
            "{} {} {} {} {}",
            style.magenta("◆"),
            style.bold(spec.label()),
            style.dim("·"),
            style.dim(format!("catalogue from {}", source_label(source))),
            style.dim(format!(
                "· approval {}",
                approval_label(config.approval_mode)
            ))
        );
    }
    let budget_is_usd = message_budget
        .as_ref()
        .is_some_and(|budget| budget.unit == BudgetUnit::Usd);
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
    let database = database_path();
    if debug {
        debug_start(
            style,
            &spec,
            &config,
            source,
            &database,
            config_path.as_deref(),
            args.session.as_deref(),
            &catalog,
        );
        debug_model(style, &spec, catalog.find(&spec.provider, &spec.model));
        debug_reasoning(style, &effective);
    }
    let store = Arc::new(
        SqliteStore::open(&database)
            .map_err(|error| format!("could not open {}: {error}", database.display()))?,
    );
    let session_store: Arc<dyn norte::harness::Store> = store.clone();
    let blobs: Arc<dyn norte::BlobStore> = store.clone();
    let directory: Arc<dyn SessionDirectory> = store;
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
    if debug {
        debug_prompt_size(style, &service, system_prompt_source);
        debug_mcp(style, &service);
    }
    service.set_config_path(config_path);

    let pinned = Pinned::new();
    let mut renderer = Renderer::new(
        if args.jsonl {
            Format::Jsonl
        } else {
            Format::Text
        },
        verbosity,
        debug,
        show_cost,
        style,
        Arc::clone(&pinned),
    );
    renderer.set_cost_silent(matches!(provider.as_str(), "anthropic" | "google"));
    renderer.model_name = Some(model_label);
    renderer.max_context = max_context;
    renderer.compaction_threshold = compaction_threshold;
    let interactive = args.interactive;
    let mut input = if interactive {
        Some(InteractiveInput::start(style, Arc::clone(&pinned))?)
    } else {
        None
    };
    let mut session = args
        .session
        .as_ref()
        .map(|key| SessionKey::new(key.clone()));
    let mut parked = false;
    if let Some(prompt) = args.prompt {
        if interactive {
            echo_user_message(style, &prompt);
        }
        // `--session` names a session that already existed, so the client is
        // starting blank against a history and asks for it.
        let attaching = args.session.is_some();
        let request = ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::SendMessage {
                session: session.clone(),
                text: prompt,
                budget: message_budget.clone(),
                reasoning: reasoning.clone(),
                replay: attaching,
            },
        };
        renderer.mark_working();
        let stream = service.submit(request).await.map_err(|e| e.to_string())?;
        let outcome = drive_turns(
            &service,
            stream,
            &mut renderer,
            style,
            input.as_mut().map(InteractiveInput::lines),
        )
        .await?;
        session = outcome.session.or(session);
        parked = outcome.parked;
        renderer.ensure_newline();
    }
    if interactive && !parked && !renderer.budget_stopped {
        if let Some(receiver) = input.as_mut().map(InteractiveInput::lines) {
            interactive_loop(
                &service,
                session,
                &mut renderer,
                style,
                receiver,
                message_budget,
                reasoning,
            )
            .await?;
        }
    }
    renderer.ensure_newline();
    renderer.summary();
    Ok(u8::from(renderer.failed))
}

#[allow(clippy::too_many_lines)]
async fn resume_session(
    config: Config,
    catalog: Catalog,
    source: Source,
    args: ResumeArgs,
    verbosity: &str,
    debug: bool,
    config_path: Option<PathBuf>,
) -> Result<u8, String> {
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
    let spec = resolve_spec(&config, &catalog, None, None, None, None, None, None, false)?;
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
            style.bold(format!("resume {}", short(&args.session))),
            style.dim("·"),
            style.dim(format!("catalogue from {}", source_label(source)))
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
    let database = database_path();
    if debug {
        debug_start(
            style,
            &spec,
            &config,
            source,
            &database,
            config_path.as_deref(),
            Some(args.session.as_str()),
            &catalog,
        );
        debug_model(style, &spec, catalog.find(&spec.provider, &spec.model));
        debug_reasoning(style, &effective);
    }
    let store = Arc::new(
        SqliteStore::open(&database)
            .map_err(|error| format!("could not open {}: {error}", database.display()))?,
    );
    let session_store: Arc<dyn norte::harness::Store> = store.clone();
    let blobs: Arc<dyn norte::BlobStore> = store.clone();
    let directory: Arc<dyn SessionDirectory> = store;
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
    if debug {
        debug_prompt_size(style, &service, system_prompt_source);
        debug_mcp(style, &service);
    }
    service.set_config_path(config_path);
    let pinned = Pinned::new();
    let mut renderer = Renderer::new(
        if args.jsonl {
            Format::Jsonl
        } else {
            Format::Text
        },
        verbosity,
        debug,
        show_cost,
        style,
        Arc::clone(&pinned),
    );
    renderer.set_cost_silent(matches!(provider.as_str(), "anthropic" | "google"));
    renderer.model_name = Some(model_label);
    renderer.max_context = max_context;
    renderer.compaction_threshold = compaction_threshold;
    let session = SessionKey::new(args.session);
    let interactive = args.interactive;
    let mut input = if interactive {
        Some(InteractiveInput::start(style, Arc::clone(&pinned))?)
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
        drive_turns(
            &service,
            stream,
            &mut renderer,
            style,
            input.as_mut().map(InteractiveInput::lines),
        )
        .await?;
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
        drive_turns(
            &service,
            stream,
            &mut renderer,
            style,
            input.as_mut().map(InteractiveInput::lines),
        )
        .await?;
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
    let outcome = drive_turns(
        &service,
        stream,
        &mut renderer,
        style,
        input.as_mut().map(InteractiveInput::lines),
    )
    .await?;
    renderer.ensure_newline();
    if interactive && !outcome.parked && !renderer.budget_stopped {
        if let Some(receiver) = input.as_mut().map(InteractiveInput::lines) {
            interactive_loop(
                &service,
                Some(session),
                &mut renderer,
                style,
                receiver,
                None,
                None,
            )
            .await?;
        }
    }
    renderer.ensure_newline();
    renderer.summary();
    Ok(u8::from(renderer.failed))
}

#[cfg(test)]
mod tests;
