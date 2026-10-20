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

mod commands;
mod interaction;
mod render;
mod style;

use commands::{apply_extensions, database_path, duration_label, run};
use interaction::{
    clip_text, compact_json, drive_turns, interactive_loop, list_models, resolve_spec, stdin_lines,
};
use render::{
    Block, Format, Renderer, approval_label, debug_model, debug_prompt_size, debug_reasoning,
    debug_start, fallback_env, find_project_config, human_tokens, parse_budget_action,
    parse_budget_unit, parse_reasoning_setting, percent_label, reasoning_label,
    select_system_prompt, short, source_label, token_ratio,
};

#[cfg(test)]
use commands::cost_summary;
#[cfg(test)]
use interaction::{
    ApprovalChoice, PreviewLine, approval_request, argument_preview, compose_selection,
    parse_approval_choice, parse_approval_command, parse_selection,
};
#[cfg(test)]
use render::budget_label;

use std::collections::{BTreeMap, VecDeque};
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
use norte_code_config::{ApprovalMode, Config, user_config_path};
use norte_code_cost::{CostSummary, by_day, by_session, by_turn, summarize};
use norte_code_protocol::{
    Budget, BudgetAction, BudgetScope, BudgetUnit, ClientRequest, ClientRequestKind, Delta,
    DenyScope, ErrorCode, GrantScope, ModelInfo, Phase, Reasoning, ReasoningInfo, ServerEvent,
    ServerEventKind, SessionKey,
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
    /// Show tool failures and other details.
    #[arg(long, global = true)]
    verbose: bool,
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
struct RunArgs {
    /// The prompt. With --interactive it may be omitted, so the session starts
    /// at the prompt.
    #[arg(required_unless_present = "interactive")]
    prompt: Option<String>,
    /// A `provider/model` reference (defaults to `default_model` in config).
    #[arg(long, short = 'm', value_name = "PROVIDER/MODEL")]
    model: Option<String>,
    /// Replace the built-in system prompt for this run.
    #[arg(long, value_name = "TEXT")]
    system_prompt: Option<String>,
    /// A base URL override for the provider.
    #[arg(long, value_name = "URL")]
    endpoint: Option<String>,
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
    /// Redact reasoning and hide tool activity and usage.
    #[arg(long)]
    quiet: bool,
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
    /// Redact reasoning and hide tool activity and usage.
    #[arg(long)]
    quiet: bool,
    /// Keep the session open and read the next message after each turn.
    #[arg(long, short = 'i')]
    interactive: bool,
}

/// Flags that override the command sandbox for one run or resume.
#[derive(Args)]
struct SandboxOptions {
    /// Run commands inside the OS sandbox, overriding `tools.sandbox.enabled`.
    #[arg(long, conflicts_with = "no_sandbox")]
    sandbox: bool,
    /// Run commands without the OS sandbox, overriding `tools.sandbox.enabled`.
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
            config.tools.sandbox.enabled = true;
        }
        if self.no_sandbox {
            config.tools.sandbox.enabled = false;
        }
        if let Some(network) = self.sandbox_network.as_deref() {
            if !config.tools.sandbox.enabled {
                return Err(
                    "--sandbox-network needs the sandbox; pass --sandbox or set \
                     tools.sandbox.enabled"
                        .to_owned(),
                );
            }
            config.tools.sandbox.network = match network {
                "on" => true,
                "off" => false,
                other => return Err(format!("unknown sandbox network setting `{other}`")),
            };
        }
        Ok(())
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
    #[arg(long, value_name = "BUCKET", value_parser = ["hour", "day", "month", "year", "total"], default_value = "day")]
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
            eprintln!("norte-code: {message}");
            ExitCode::from(2)
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run_prompt(
    config: Config,
    catalog: Catalog,
    source: Source,
    args: RunArgs,
    verbose: bool,
    debug: bool,
    config_path: Option<PathBuf>,
) -> Result<u8, String> {
    let spec = resolve_spec(&config, &catalog, args.model.clone(), args.endpoint.clone())?;
    let mut config = config;
    if let Some(mode) = args.approval_mode.as_deref() {
        config.approval_mode = match mode {
            "ask" => ApprovalMode::Ask,
            "auto-edit" => ApprovalMode::AutoEdit,
            "yolo" => ApprovalMode::Yolo,
            other => return Err(format!("unknown approval mode `{other}`")),
        };
    }
    if let Some(limit) = args.max_iterations {
        if limit == 0 {
            return Err("--max-iterations must be at least 1".to_owned());
        }
        config.max_iterations = Some(limit);
    }
    if let Some(threshold) = args.compaction_threshold {
        if !threshold.is_finite() || threshold <= 0.0 || threshold > 1.0 {
            return Err(format!(
                "--compaction-threshold must be a fraction in (0, 1], got {threshold}"
            ));
        }
        config.compaction.threshold = threshold;
    }
    args.sandbox.apply(&mut config)?;
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
    if args.interactive && args.jsonl {
        return Err("--interactive cannot be combined with --jsonl".to_owned());
    }
    if args.interactive && !std::io::stdin().is_terminal() {
        return Err("--interactive needs a terminal on stdin".to_owned());
    }
    apply_extensions(&mut config, &args.extension)?;
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
    if !args.quiet && !args.jsonl {
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
    let show_cost = config.cost.show;
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
        );
        debug_model(style, &spec, catalog.find(&spec.provider, &spec.model));
        debug_reasoning(style, &effective);
    }
    let store = Arc::new(
        SqliteStore::open(&database)
            .map_err(|error| format!("could not open {}: {error}", database.display()))?,
    );
    let session_store: Arc<dyn norte::Store> = store.clone();
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
    }
    service.set_config_path(config_path);

    let mut renderer = Renderer {
        format: if args.jsonl {
            Format::Jsonl
        } else {
            Format::Text
        },
        quiet: args.quiet,
        verbose,
        debug,
        debug_session: false,
        show_cost,
        style,
        block: Block::None,
        reasoning_line_start: false,
        reasoning_placeholder: false,
        last_char: None,
        failed: false,
        cancelled: false,
        cache_read: 0,
        cache_write: 0,
        cache_reported: false,
        budget_stopped: false,
        requests: 0,
        input: 0,
        output: 0,
        cost: 0.0,
        unpriced: 0,
        tool_counts: BTreeMap::new(),
    };
    let interactive = args.interactive;
    let mut input = interactive.then(stdin_lines);
    let mut session = args
        .session
        .as_ref()
        .map(|key| SessionKey::new(key.clone()));
    let mut parked = false;
    if let Some(prompt) = args.prompt {
        let request = ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::SendMessage {
                session: session.clone(),
                text: prompt,
                budget: message_budget.clone(),
                reasoning: reasoning.clone(),
            },
        };
        let stream = service.submit(request).await.map_err(|e| e.to_string())?;
        let outcome = drive_turns(&service, stream, &mut renderer, style, input.as_mut()).await?;
        session = outcome.session.or(session);
        parked = outcome.parked;
        renderer.ensure_newline();
    }
    if interactive && !parked && !renderer.budget_stopped {
        if let Some(receiver) = input.as_mut() {
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
    verbose: bool,
    debug: bool,
    config_path: Option<PathBuf>,
) -> Result<u8, String> {
    let mut config = config;
    if let Some(limit) = args.max_iterations {
        if limit == 0 {
            return Err("--max-iterations must be at least 1".to_owned());
        }
        config.max_iterations = Some(limit);
    }
    if let Some(threshold) = args.compaction_threshold {
        if !threshold.is_finite() || threshold <= 0.0 || threshold > 1.0 {
            return Err(format!(
                "--compaction-threshold must be a fraction in (0, 1], got {threshold}"
            ));
        }
        config.compaction.threshold = threshold;
    }
    args.sandbox.apply(&mut config)?;
    let (system_prompt_source, system_prompt) =
        select_system_prompt(args.system_prompt, config.system_prompt.clone());
    if args.interactive && args.jsonl {
        return Err("--interactive cannot be combined with --jsonl".to_owned());
    }
    if args.interactive && !std::io::stdin().is_terminal() {
        return Err("--interactive needs a terminal on stdin".to_owned());
    }
    apply_extensions(&mut config, &args.extension)?;
    let spec = resolve_spec(&config, &catalog, None, None)?;
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
    if !args.quiet && !args.jsonl {
        eprintln!(
            "{} {} {} {}",
            style.magenta("◆"),
            style.bold(format!("resume {}", short(&args.session))),
            style.dim("·"),
            style.dim(format!("catalogue from {}", source_label(source)))
        );
    }
    let show_cost = config.cost.show;
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
        );
        debug_model(style, &spec, catalog.find(&spec.provider, &spec.model));
        debug_reasoning(style, &effective);
    }
    let store = Arc::new(
        SqliteStore::open(&database)
            .map_err(|error| format!("could not open {}: {error}", database.display()))?,
    );
    let session_store: Arc<dyn norte::Store> = store.clone();
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
    }
    service.set_config_path(config_path);
    let mut renderer = Renderer {
        format: if args.jsonl {
            Format::Jsonl
        } else {
            Format::Text
        },
        quiet: args.quiet,
        verbose,
        debug,
        debug_session: false,
        show_cost,
        style,
        block: Block::None,
        reasoning_line_start: false,
        reasoning_placeholder: false,
        last_char: None,
        failed: false,
        cancelled: false,
        cache_read: 0,
        cache_write: 0,
        cache_reported: false,
        budget_stopped: false,
        requests: 0,
        input: 0,
        output: 0,
        cost: 0.0,
        unpriced: 0,
        tool_counts: BTreeMap::new(),
    };
    let session = SessionKey::new(args.session);
    let interactive = args.interactive;
    let mut input = interactive.then(stdin_lines);
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
        drive_turns(&service, stream, &mut renderer, style, input.as_mut()).await?;
    }
    let stream = service
        .submit(ClientRequest {
            request_id: 2,
            kind: ClientRequestKind::Resume {
                session: session.clone(),
            },
        })
        .await
        .map_err(|e| e.to_string())?;
    let outcome = drive_turns(&service, stream, &mut renderer, style, input.as_mut()).await?;
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
