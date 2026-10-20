//! The headless `norte-code` command.
//!
//! `norte-code run "prompt"` builds the service in memory, sends one message
//! and streams the answer. Text goes to stdout; diagnostics, reasoning and
//! tool activity go to stderr. `--jsonl` writes one protocol event per line on
//! stdout instead, for scripts and tests.
//!
//! `norte-code models` lists what the catalogue knows, filtered by provider,
//! tool calling or reasoning.

mod style;

use std::collections::VecDeque;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use futures_util::StreamExt;
use norte_code_agent::{
    DEFAULT_SYSTEM_PROMPT, EventStream, ModelSpec, Service, ServiceError, VendorModels, model_list,
};
use norte_code_catalog::{Catalog, LoadOptions, Source, cache_dir, load_catalog};
use norte_code_config::{ApprovalMode, Config, user_config_path};
use norte_code_cost::{CostSummary, by_day, by_session, by_turn, summarize};
use norte_code_protocol::{
    Budget, BudgetAction, BudgetScope, BudgetUnit, ClientRequest, ClientRequestKind, Delta,
    ErrorCode, GrantScope, ModelInfo, Phase, Reasoning, ReasoningInfo, ServerEvent,
    ServerEventKind, SessionKey,
};
use norte_code_skills::{SkillRegistry, discovery_roots};
use norte_code_store_sqlite::{SessionDirectory, SqliteStore};

use crate::style::Style;

#[derive(Parser)]
#[command(
    name = "norte-code",
    version,
    about = "A terminal-based agentic coding tool"
)]
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
}

#[derive(Args)]
struct RunArgs {
    /// The prompt.
    prompt: String,
    /// A `provider/model` reference (defaults to `default_model` in config).
    #[arg(long, short = 'm', value_name = "PROVIDER/MODEL")]
    model: Option<String>,
    /// A base URL override for the provider.
    #[arg(long, value_name = "URL")]
    endpoint: Option<String>,
    /// A spend limit for the whole run, in USD.
    #[arg(long, value_name = "USD")]
    budget: Option<f64>,
    /// A reasoning effort such as `low`, `medium` or `high`.
    #[arg(long, value_name = "EFFORT")]
    reasoning: Option<String>,
    /// What needs approval: `ask` (default), `auto-edit` or `yolo`.
    #[arg(long, value_name = "MODE", value_parser = ["ask", "auto-edit", "yolo"])]
    approval_mode: Option<String>,
    /// Write one JSON event per line on stdout.
    #[arg(long)]
    jsonl: bool,
    /// Do not print reasoning, tool activity or usage.
    #[arg(long)]
    quiet: bool,
    /// Continue an existing session by key instead of creating one.
    #[arg(long, value_name = "KEY")]
    session: Option<String>,
}

#[derive(Args)]
struct ResumeArgs {
    /// The session key to continue.
    session: String,
    /// Replace the session budget before resuming.
    #[arg(long, value_name = "LIMIT")]
    budget: Option<f64>,
    /// What the budget measures.
    #[arg(long, value_name = "UNIT", value_parser = ["usd", "tokens", "requests"], default_value = "usd")]
    budget_unit: String,
    /// What to do when the budget is exhausted.
    #[arg(long, value_name = "ACTION", value_parser = ["warn", "ask", "stop"], default_value = "stop")]
    budget_action: String,
    /// Write one JSON event per line on stdout.
    #[arg(long)]
    jsonl: bool,
    /// Do not print reasoning, tool activity or usage.
    #[arg(long)]
    quiet: bool,
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

async fn run(cli: Cli) -> Result<u8, String> {
    let config_path = cli.config.clone();
    let offline = cli.offline;
    let refresh = cli.refresh;
    let verbose = cli.verbose;
    let config = load_config(config_path)?;
    match cli.command {
        Command::Skills(args) => skills_command(&config, &args),
        Command::Sessions(args) => sessions_command(&args).await,
        Command::Cost(args) => cost_command(&config, &args).await,
        command => {
            let (catalog, source) = load_catalogue(&config, offline, refresh).await?;
            match command {
                Command::Run(args) => run_prompt(config, catalog, source, args, verbose).await,
                Command::Resume(args) => {
                    resume_session(config, catalog, source, args, verbose).await
                }
                Command::Models(args) => list_models(&config, &catalog, source, &args),
                Command::Skills(_) | Command::Sessions(_) | Command::Cost(_) => {
                    unreachable!("these commands do not need the catalogue")
                }
            }
        }
    }
}

fn skills_command(config: &Config, args: &SkillsArgs) -> Result<u8, String> {
    let workspace = std::env::current_dir().map_err(|error| error.to_string())?;
    let roots = discovery_roots(&workspace, &config.skills.paths, config.skills.interop);
    let registry = SkillRegistry::discover(&roots, config.skills.max_skills);
    match args.command {
        SkillsCommand::List { json } => {
            for skill in registry.skills() {
                if json {
                    let line = serde_json::json!({
                        "name": skill.name,
                        "description": skill.description,
                        "root": skill.root.display().to_string(),
                        "allowed_tools": skill.allowed_tools,
                    });
                    println!("{line}");
                } else {
                    println!(
                        "{}\t{}\t{}",
                        skill.name,
                        skill.root.display(),
                        skill.description
                    );
                }
            }
            Ok(0)
        }
        SkillsCommand::Check => {
            for skill in registry.skills() {
                println!(
                    "ok\t{}\t{}\t{}",
                    skill.name,
                    skill.root.display(),
                    skill.description
                );
            }
            for warning in registry.warnings() {
                eprintln!("warning: {}: {}", warning.path.display(), warning.message);
            }
            if registry.warnings().is_empty() {
                Ok(0)
            } else {
                Ok(1)
            }
        }
    }
}

/// Where Norte Code keeps its database.
fn database_path() -> PathBuf {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data).join("norte-code").join("sessions.db");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("norte-code")
            .join("sessions.db");
    }
    PathBuf::from(".norte-code").join("sessions.db")
}

async fn sessions_command(args: &SessionsArgs) -> Result<u8, String> {
    let path = database_path();
    if !path.exists() {
        if !args.json {
            println!("no sessions yet (the database is {})", path.display());
        }
        return Ok(0);
    }
    let store = SqliteStore::open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    if let Err(error) = store.repair_interrupted().await {
        eprintln!("warning: could not repair interrupted sessions: {error}");
    }
    let sessions = store
        .sessions()
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    for session in sessions {
        if args.json {
            let line = serde_json::json!({
                "key": session.key,
                "title": session.title,
                "model": session.model,
                "created_at": session.created_at.to_rfc3339(),
                "phase": session.phase,
            });
            println!("{line}");
        } else {
            println!(
                "{}\t{}\t{}\t{}",
                session.key,
                session.title.as_deref().unwrap_or("(untitled)"),
                session.model,
                session.phase
            );
        }
    }
    Ok(0)
}

async fn cost_command(config: &Config, args: &CostArgs) -> Result<u8, String> {
    if !config.cost.show {
        eprintln!("cost display is off (cost.show = false)");
        return Ok(0);
    }
    let path = database_path();
    if !path.exists() {
        println!("no cost records yet (the database is {})", path.display());
        return Ok(0);
    }
    let store = SqliteStore::open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let records = store
        .costs(args.session.as_deref())
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if records.is_empty() {
        println!("no cost records yet");
        return Ok(0);
    }
    let total = summarize(&records);
    let sessions = by_session(&records);
    let days = by_day(&records);
    let turns = by_turn(&records);
    if args.json {
        let sessions: Vec<_> = sessions
            .iter()
            .map(|(session, summary)| serde_json::json!({"session": session, "summary": summary}))
            .collect();
        let days: Vec<_> = days
            .iter()
            .map(|(day, summary)| serde_json::json!({"day": day, "summary": summary}))
            .collect();
        let turns: Vec<_> = turns
            .iter()
            .map(|((session, turn), summary)| {
                serde_json::json!({"session": session, "turn": turn, "summary": summary})
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "total": total,
                "sessions": sessions,
                "days": days,
                "turns": turns,
            })
        );
        return Ok(0);
    }
    let style = Style::stdout();
    println!("{}  {}", style.bold("total"), cost_summary(&total));
    if args.session.is_some() {
        for ((_, turn), summary) in &turns {
            println!("turn {turn:<4}     {}", cost_summary(summary));
        }
    } else if !args.daily {
        for (session, summary) in &sessions {
            println!(
                "{}  {}",
                style.bold(format!("session {}", short(session))),
                cost_summary(summary)
            );
        }
    }
    if !args.daily && args.session.is_none() {
        println!();
    }
    for (day, summary) in &days {
        println!("{}  {}", style.bold(day), cost_summary(summary));
    }
    Ok(0)
}

/// One line of token counts, reported cost and missing-cost count.
fn cost_summary(summary: &CostSummary) -> String {
    let mut parts = vec![
        format!("{} request(s)", summary.requests),
        format!("{} in", human_tokens(summary.input_tokens)),
        format!("{} out", human_tokens(summary.output_tokens)),
    ];
    if summary.not_reported == summary.requests {
        parts.push("cost not reported".to_owned());
    } else {
        parts.push(format!("${:.6}", summary.reported_cost_usd));
        if summary.not_reported > 0 {
            parts.push(format!("{} not reported", summary.not_reported));
        }
    }
    parts.join(" · ")
}

fn load_config(path: Option<PathBuf>) -> Result<Config, String> {
    let user = path.or_else(user_config_path);
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let project = find_project_config(&cwd);
    let loaded = Config::load(user.as_deref(), project.as_deref()).map_err(|e| e.to_string())?;
    for warning in &loaded.warnings {
        eprintln!("warning: unknown config key `{warning}`");
    }
    Ok(loaded.config)
}

async fn load_catalogue(
    config: &Config,
    offline: bool,
    refresh: bool,
) -> Result<(Catalog, Source), String> {
    let dir = cache_dir().unwrap_or_else(|| PathBuf::from(".norte-code").join("cache"));
    let ttl = if refresh {
        Duration::ZERO
    } else {
        Duration::from_secs(config.catalog.ttl_hours.saturating_mul(60 * 60))
    };
    let loaded = load_catalog(&LoadOptions {
        url: &config.catalog.url,
        ttl,
        offline: offline || config.catalog.offline,
        cache_dir: &dir,
    })
    .await
    .map_err(|e| e.to_string())?;
    for warning in &loaded.warnings {
        eprintln!("warning: {warning}");
    }
    Ok((loaded.catalog, loaded.source))
}

#[allow(clippy::too_many_lines)]
async fn run_prompt(
    config: Config,
    catalog: Catalog,
    source: Source,
    args: RunArgs,
    verbose: bool,
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
    if let Some(limit) = args.budget {
        if !limit.is_finite() || limit <= 0.0 {
            return Err(format!("--budget must be a positive number, got {limit}"));
        }
    }
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
            DEFAULT_SYSTEM_PROMPT,
        )
        .await
        .map_err(|e| e.to_string())?,
    );

    let request = ClientRequest {
        request_id: 1,
        kind: ClientRequestKind::SendMessage {
            session: args
                .session
                .as_ref()
                .map(|key| SessionKey::new(key.clone())),
            text: args.prompt,
            budget: args.budget.map(|limit| Budget {
                unit: BudgetUnit::Usd,
                limit,
                action: BudgetAction::Stop,
            }),
            reasoning: args.reasoning.map(|effort| Reasoning::Effort { effort }),
        },
    };
    let stream = service.submit(request).await.map_err(|e| e.to_string())?;

    let mut renderer = Renderer {
        format: if args.jsonl {
            Format::Jsonl
        } else {
            Format::Text
        },
        quiet: args.quiet,
        verbose,
        show_cost,
        style,
        last_char: None,
        failed: false,
        budget_stopped: false,
        requests: 0,
        input: 0,
        output: 0,
        cost: 0.0,
        unpriced: 0,
    };

    drive_turns(&service, stream, &mut renderer, style).await?;
    if matches!(renderer.format, Format::Text)
        && renderer.last_char.is_some_and(|last| last != '\n')
    {
        println!();
    }
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
) -> Result<u8, String> {
    let spec = resolve_spec(&config, &catalog, None, None)?;
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
            DEFAULT_SYSTEM_PROMPT,
        )
        .await
        .map_err(|e| e.to_string())?,
    );
    let mut renderer = Renderer {
        format: if args.jsonl {
            Format::Jsonl
        } else {
            Format::Text
        },
        quiet: args.quiet,
        verbose,
        show_cost,
        style,
        last_char: None,
        failed: false,
        budget_stopped: false,
        requests: 0,
        input: 0,
        output: 0,
        cost: 0.0,
        unpriced: 0,
    };
    let session = SessionKey::new(args.session);
    if let Some(limit) = args.budget {
        if !limit.is_finite() || limit <= 0.0 {
            return Err(format!("--budget must be a positive number, got {limit}"));
        }
        let unit = match args.budget_unit.as_str() {
            "usd" => BudgetUnit::Usd,
            "tokens" => BudgetUnit::Tokens,
            "requests" => BudgetUnit::Requests,
            other => return Err(format!("unknown budget unit `{other}`")),
        };
        let action = match args.budget_action.as_str() {
            "warn" => BudgetAction::Warn,
            "ask" => BudgetAction::Ask,
            "stop" => BudgetAction::Stop,
            other => return Err(format!("unknown budget action `{other}`")),
        };
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
        drive_turns(&service, stream, &mut renderer, style).await?;
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
    drive_turns(&service, stream, &mut renderer, style).await?;
    if matches!(renderer.format, Format::Text)
        && renderer.last_char.is_some_and(|last| last != '\n')
    {
        println!();
    }
    renderer.summary();
    Ok(u8::from(renderer.failed))
}

/// Streams the primary turn, then any turns resumed by an approval.
///
/// A turn may park for several approvals; each decision resumes on its own
/// stream, which can park again, so the loop drains a queue of streams.
async fn drive_turns(
    service: &Arc<Service>,
    primary: EventStream,
    renderer: &mut Renderer,
    style: Style,
) -> Result<(), String> {
    let interactive = matches!(renderer.format, Format::Text) && std::io::stdin().is_terminal();
    let mut streams: VecDeque<EventStream> = VecDeque::new();
    streams.push_back(primary);
    let mut pending: VecDeque<(SessionKey, String, ApprovalChoice)> = VecDeque::new();
    let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::channel::<()>(4);
    let signal = tokio::spawn(async move {
        loop {
            if tokio::signal::ctrl_c().await.is_err() || interrupt_tx.send(()).await.is_err() {
                break;
            }
        }
    });
    let mut current: Option<SessionKey> = None;
    let mut interrupted = false;
    while let Some(mut stream) = streams.pop_front() {
        loop {
            let event = tokio::select! {
                biased;
                Some(()) = interrupt_rx.recv() => {
                    let Some(session) = current.clone() else { continue };
                    if interrupted {
                        eprintln!("{}", style.red("interrupted"));
                        signal.abort();
                        return Err("interrupted".to_owned());
                    }
                    interrupted = true;
                    eprintln!(
                        "{}",
                        style.yellow(
                            "cancelling the running turn… (press Ctrl-C again to quit)"
                        )
                    );
                    let request = ClientRequest {
                        request_id: 0,
                        kind: ClientRequestKind::CancelTurn { session },
                    };
                    if let Err(error) = service.submit(request).await {
                        eprintln!(
                            "{}",
                            style.yellow(format!("warning: could not cancel the turn: {error}"))
                        );
                    }
                    continue;
                }
                event = stream.next() => event,
            };
            let Some(event) = event else { break };
            if let Some(session) = &event.session {
                current = Some(session.clone());
            }
            if let Some(approval) = ask_approval(&event, style, interactive) {
                pending.push_back(approval);
            }
            answer_if_asked(service, &event).await?;
            renderer.event(&event)?;
        }
        while let Some((session, approval, choice)) = pending.pop_front() {
            let request = approval_request(session, approval.clone(), choice);
            match service.submit(request).await {
                Ok(next) => streams.push_back(next),
                Err(error) => {
                    eprintln!(
                        "{}",
                        style.yellow(format!(
                            "warning: could not resolve approval {}: {error}",
                            short(&approval)
                        ))
                    );
                }
            }
        }
    }
    signal.abort();
    Ok(())
}

/// What the human chose for a parked call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApprovalChoice {
    /// Run this call once.
    Once,
    /// Run any call to this tool for the rest of the session.
    Session,
    /// Refuse the call.
    Deny,
}

/// Prints an approval request and asks for a decision when stdin is a terminal.
///
/// Returns the decision to submit later, or `None` when the call stays parked
/// (no interactive client) or the event is not an approval request.
fn ask_approval(
    event: &ServerEvent,
    style: Style,
    interactive: bool,
) -> Option<(SessionKey, String, ApprovalChoice)> {
    let ServerEventKind::ApprovalRequested {
        approval,
        tool,
        call_id,
        arguments,
        ..
    } = &event.kind
    else {
        return None;
    };
    let session = event.session.clone()?;
    eprintln!();
    eprintln!(
        "{} {} {}",
        style.yellow("approval required"),
        style.bold(tool),
        style.dim(format!("#{}", short(call_id)))
    );
    if let Some(arguments) = arguments {
        for line in argument_preview(arguments) {
            eprintln!("  {}", style.dim(line));
        }
    }
    if !interactive {
        eprintln!(
            "  {}",
            style.dim(
                "not an interactive terminal; the session stays parked. Use --approval-mode \
                 auto-edit or yolo to run without asking."
            )
        );
        return None;
    }
    loop {
        eprint!(
            "{} ",
            style.cyan("[y]es / [n]o / [a]lways for this session ›")
        );
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        let read = std::io::stdin().read_line(&mut line);
        if read.is_err() || read.is_ok_and(|bytes| bytes == 0) {
            return Some((session, approval.clone(), ApprovalChoice::Deny));
        }
        if let Some(choice) = parse_approval_choice(&line) {
            return Some((session, approval.clone(), choice));
        }
        eprintln!("{}", style.dim("answer y, n or a"));
    }
}

/// Parses one line of the approval prompt. `None` means ask again.
fn parse_approval_choice(line: &str) -> Option<ApprovalChoice> {
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Some(ApprovalChoice::Once),
        "a" | "always" => Some(ApprovalChoice::Session),
        "n" | "no" | "" => Some(ApprovalChoice::Deny),
        _ => None,
    }
}

/// A compact rendering of a tool call's arguments for the approval prompt.
fn argument_preview(arguments: &serde_json::Value) -> Vec<String> {
    const MAX_LINES: usize = 10;
    const MAX_WIDTH: usize = 120;
    let compact = serde_json::to_string(arguments).unwrap_or_default();
    if compact.chars().count() <= MAX_WIDTH {
        return vec![compact];
    }
    let pretty = serde_json::to_string_pretty(arguments).unwrap_or(compact);
    let total = pretty.lines().count();
    let mut lines: Vec<String> = pretty
        .lines()
        .take(MAX_LINES)
        .map(|line| {
            if line.chars().count() > MAX_WIDTH {
                let mut shortened: String = line.chars().take(MAX_WIDTH).collect();
                shortened.push('…');
                shortened
            } else {
                line.to_owned()
            }
        })
        .collect();
    if total > MAX_LINES {
        lines.push("…".to_owned());
    }
    lines
}

/// The protocol request that resolves a parked approval.
fn approval_request(
    session: SessionKey,
    approval: String,
    choice: ApprovalChoice,
) -> ClientRequest {
    let kind = match choice {
        ApprovalChoice::Once => ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: None,
            remember: None,
        },
        ApprovalChoice::Session => ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: Some(GrantScope::Session),
            remember: Some(true),
        },
        ApprovalChoice::Deny => ClientRequestKind::Deny {
            session,
            approval,
            reason: Some("denied from the command line".to_owned()),
        },
    };
    ClientRequest {
        request_id: 0,
        kind,
    }
}

/// Answers a `QuestionAsked` event, prompting on a terminal and leaving a note
/// when the client is not interactive.
async fn answer_if_asked(service: &Arc<Service>, event: &ServerEvent) -> Result<(), String> {
    let ServerEventKind::QuestionAsked {
        question, prompt, ..
    } = &event.kind
    else {
        return Ok(());
    };
    let session = event
        .session
        .clone()
        .ok_or("a question arrived without a session")?;
    let question = question.clone();
    let answer = if std::io::stdin().is_terminal() {
        eprintln!("? {prompt}");
        eprint!("answer> ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        line.trim_end().to_owned()
    } else {
        eprintln!("? {prompt}");
        eprintln!("(no interactive client; answering with a note)");
        "(no interactive client is attached; continue with your best judgment)".to_owned()
    };
    let response = ClientRequest {
        request_id: 0,
        kind: ClientRequestKind::AnswerQuestion {
            session,
            question,
            answer,
        },
    };
    let mut acknowledgement = service.submit(response).await.map_err(|e| e.to_string())?;
    while acknowledgement.next().await.is_some() {}
    Ok(())
}

fn list_models(
    config: &Config,
    catalog: &Catalog,
    source: Source,
    args: &ModelsArgs,
) -> Result<u8, String> {
    let mut models = model_list(config, catalog);
    models.retain(|model| {
        args.provider
            .as_ref()
            .is_none_or(|provider| &model.provider == provider)
    });
    if args.tools {
        models.retain(|model| model.tools);
    }
    if args.reasoning {
        models.retain(|model| model.reasoning.supported);
    }
    models.sort_by(|a, b| {
        (a.provider.as_str(), a.model.as_str()).cmp(&(b.provider.as_str(), b.model.as_str()))
    });

    if args.json {
        for model in &models {
            let line = serde_json::to_string(model).map_err(|error| error.to_string())?;
            println!("{line}");
        }
    } else {
        eprintln!(
            "catalogue: {} models from {}",
            models.len(),
            source_label(source)
        );
        for model in &models {
            print_model(model);
        }
    }
    Ok(0)
}

fn print_model(model: &ModelInfo) {
    let reference = format!("{}/{}", model.provider, model.model);
    let context = model
        .max_context
        .map_or_else(|| "-".to_owned(), human_tokens);
    let tools = if model.tools { "tools" } else { "-" };
    let reasoning = reasoning_label(&model.reasoning);
    let name = if model.deprecated {
        format!("{} (deprecated)", model.name)
    } else {
        model.name.clone()
    };
    println!("{reference:<54} {context:>8}  {tools:<5}  {reasoning:<18} {name}");
}

fn resolve_spec(
    config: &Config,
    catalog: &Catalog,
    model: Option<String>,
    endpoint: Option<String>,
) -> Result<ModelSpec, String> {
    let reference = model
        .or_else(|| config.default_model.clone())
        .ok_or_else(|| {
            "no model selected: pass `--model provider/model` or set `default_model` in the config"
                .to_owned()
        })?;
    match ModelSpec::from_catalog(config, catalog, &reference, endpoint.clone()) {
        Ok(spec) => Ok(spec),
        Err(ServiceError::UnknownModel { .. }) => {
            eprintln!("note: `{reference}` is not in the catalogue; using its provider's facts");
            ModelSpec::from_provider(config, catalog, &reference, endpoint)
                .map_err(|error| error.to_string())
        }
        Err(ServiceError::UnknownProvider(_)) => {
            eprintln!(
                "note: `{reference}` names a provider the catalogue does not list; using the fallback"
            );
            let provider = reference
                .split_once('/')
                .map_or("", |(provider, _)| provider);
            ModelSpec::resolve(config, &reference, endpoint, &fallback_env(provider))
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// How events are written.
#[derive(Clone, Copy)]
enum Format {
    /// Streamed text on stdout, diagnostics on stderr.
    Text,
    /// One JSON event per line on stdout.
    Jsonl,
}

/// Renders one server event to the terminal.
#[allow(clippy::struct_excessive_bools)]
struct Renderer {
    format: Format,
    quiet: bool,
    verbose: bool,
    show_cost: bool,
    style: Style,
    last_char: Option<char>,
    failed: bool,
    budget_stopped: bool,
    requests: u64,
    input: u64,
    output: u64,
    cost: f64,
    unpriced: u64,
}

impl Renderer {
    fn event(&mut self, event: &ServerEvent) -> Result<(), String> {
        if matches!(self.format, Format::Jsonl) {
            let line = serde_json::to_string(event).map_err(|error| error.to_string())?;
            println!("{line}");
            return Ok(());
        }
        match &event.kind {
            ServerEventKind::Delta { delta, .. } => self.delta(delta),
            ServerEventKind::ToolStarted { .. } | ServerEventKind::ToolFinished { .. } => {
                self.tool_event(&event.kind);
            }
            ServerEventKind::RequestReconstructed { .. }
            | ServerEventKind::CostNotReported { .. } => {
                self.usage_event(&event.kind);
            }
            kind => self.status_event(kind),
        }
        Ok(())
    }

    /// Renders tool activity: what started, and how it ended.
    fn tool_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::ToolStarted { tool, call_id, .. } => {
                if !self.quiet {
                    eprintln!(
                        "{} {} {}",
                        self.style.dim("→"),
                        self.style.bold(tool),
                        self.style.dim(format!("#{}", short(call_id)))
                    );
                }
            }
            ServerEventKind::ToolFinished {
                tool,
                is_error,
                error,
                duration_ms,
                ..
            } => {
                if !self.quiet {
                    if *is_error {
                        eprintln!(
                            "{} {} {}",
                            self.style.red("✗"),
                            self.style.red(tool),
                            self.style.dim(format!("failed in {duration_ms} ms"))
                        );
                    } else {
                        eprintln!(
                            "{} {} {}",
                            self.style.green("✓"),
                            tool,
                            self.style.dim(format!("in {duration_ms} ms"))
                        );
                    }
                }
                if *is_error && self.verbose {
                    if let Some(error) = error {
                        eprintln!("  {}", self.style.red(error));
                    }
                }
            }
            _ => {}
        }
    }

    /// Renders usage for one reconstructed request and keeps the run totals.
    fn usage_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::RequestReconstructed {
                usage,
                cost_usd,
                truncated,
                ..
            } => {
                self.requests += 1;
                self.input += usage.input_tokens.unwrap_or(0);
                self.output += usage.output_tokens.unwrap_or(0);
                if let Some(cost) = cost_usd {
                    self.cost += cost;
                } else {
                    self.unpriced += 1;
                }
                if self.quiet {
                    return;
                }
                let mut parts = Vec::new();
                if let Some(input) = usage.input_tokens {
                    parts.push(format!("{} in", human_tokens(input)));
                }
                if let Some(output) = usage.output_tokens {
                    parts.push(format!("{} out", human_tokens(output)));
                }
                if self.show_cost {
                    parts.push(match cost_usd {
                        Some(cost) => format!("${cost:.6}"),
                        None => "cost not reported".to_owned(),
                    });
                }
                eprintln!(
                    "  {} {}",
                    self.style.dim("↳"),
                    self.style.dim(parts.join(" · "))
                );
                if *truncated {
                    eprintln!(
                        "  {}",
                        self.style
                            .yellow("warning: the answer hit the output limit")
                    );
                }
            }
            ServerEventKind::CostNotReported { requests } if !self.quiet && self.show_cost => {
                eprintln!(
                    "  {}",
                    self.style.yellow(format!(
                        "warning: {requests} request(s) carried no cost; the total is incomplete"
                    ))
                );
            }
            _ => {}
        }
    }

    /// Renders errors, approval outcomes and the turn's final state.
    fn status_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::Error { code, message } => {
                eprintln!(
                    "{} {} {}",
                    self.style.red("error:"),
                    self.style.bold(message),
                    self.style.dim(format!("({})", code_name(*code)))
                );
                self.failed = true;
            }
            ServerEventKind::ApprovalResolved { approved, .. } => {
                if !self.quiet {
                    if *approved {
                        eprintln!("{}", self.style.green("✓ approval granted"));
                    } else {
                        eprintln!("{}", self.style.red("✗ approval denied"));
                    }
                }
            }
            ServerEventKind::TurnFinished {
                committed, phase, ..
            } => {
                if !*committed && self.budget_stopped {
                    if !self.quiet {
                        eprintln!(
                            "{}",
                            self.style.yellow(
                                "stopped by budget; raise it and resume the session to continue"
                            )
                        );
                    }
                } else if !*committed {
                    eprintln!("{}", self.style.red("the turn did not commit"));
                    self.failed = true;
                } else if *phase == Phase::AwaitingApproval && !self.quiet {
                    eprintln!("{}", self.style.yellow("paused: waiting for approval"));
                }
            }
            ServerEventKind::BudgetWarning {
                scope,
                unit,
                spent,
                limit,
            } => {
                if !self.quiet {
                    eprintln!(
                        "{} {}",
                        self.style.yellow("budget warning:"),
                        self.style.dim(format!(
                            "{} {} of {} {}",
                            scope_name(*scope),
                            format_measure(*unit, *spent),
                            format_measure(*unit, *limit),
                            unit_name(*unit)
                        ))
                    );
                }
            }
            ServerEventKind::BudgetExceeded {
                scope,
                unit,
                spent,
                limit,
                action,
            } => {
                self.budget_stopped = !matches!(action, BudgetAction::Warn);
                eprintln!(
                    "{} {}",
                    self.style.red("budget exceeded:"),
                    self.style.dim(format!(
                        "{} {} of {} {} ({})",
                        scope_name(*scope),
                        format_measure(*unit, *spent),
                        format_measure(*unit, *limit),
                        unit_name(*unit),
                        budget_action_name(*action)
                    ))
                );
            }
            ServerEventKind::BudgetChanged { budget } if !self.quiet => match budget {
                Some(budget) => eprintln!(
                    "{} {}",
                    self.style.dim("budget set:"),
                    self.style.dim(budget_label(budget))
                ),
                None => eprintln!("{}", self.style.dim("budget cleared")),
            },
            _ => {}
        }
    }

    fn delta(&mut self, delta: &Delta) {
        match delta {
            Delta::Text { text, .. } => {
                print!("{text}");
                if let Some(last) = text.chars().last() {
                    self.last_char = Some(last);
                }
                let _ = std::io::stdout().flush();
            }
            Delta::Reasoning { text, .. } => {
                if !self.quiet {
                    eprint!("{}", self.style.thought(text));
                    let _ = std::io::stderr().flush();
                }
            }
            Delta::Refusal { text, .. } => {
                eprintln!("{}", self.style.red(format!("refusal: {text}")));
                self.failed = true;
            }
            Delta::ToolArguments { .. } | Delta::Media { .. } => {}
        }
    }

    /// Writes the run's cost summary, if the run made any requests.
    fn summary(&self) {
        if matches!(self.format, Format::Jsonl) || self.quiet || self.requests == 0 {
            return;
        }
        let mut parts = vec![
            format!("{} request(s)", self.requests),
            format!("{} in", human_tokens(self.input)),
            format!("{} out", human_tokens(self.output)),
        ];
        if self.show_cost {
            if self.unpriced == self.requests {
                parts.push("cost not reported".to_owned());
            } else {
                parts.push(format!("${:.6}", self.cost));
                if self.unpriced > 0 {
                    parts.push(format!("{} not reported", self.unpriced));
                }
            }
        }
        eprintln!(
            "{} {}",
            self.style.dim("──"),
            self.style.dim(parts.join(" · "))
        );
    }
}

/// The per-project overlay config path, searched from `start` upwards.
fn find_project_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(".norte-code").join("config.json"))
        .find(|path| path.exists())
}

/// The environment variables a provider is likely to use.
///
/// The catalogue's `env` list is authoritative; this is only for a provider the
/// catalogue does not list.
fn fallback_env(provider: &str) -> Vec<String> {
    let upper: String = provider
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let mut names = vec![format!("{upper}_API_KEY")];
    if matches!(provider, "google" | "gemini") {
        for name in [
            "GOOGLE_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_GENERATIVE_AI_API_KEY",
        ] {
            if !names.iter().any(|existing| existing == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

fn human_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let whole = tokens / 1_000_000;
        let decimal = (tokens % 1_000_000) / 100_000;
        if decimal == 0 {
            format!("{whole}M")
        } else {
            format!("{whole}.{decimal}M")
        }
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

fn reasoning_label(reasoning: &ReasoningInfo) -> String {
    if !reasoning.supported {
        return "-".to_owned();
    }
    if reasoning.efforts.is_empty() {
        return if reasoning.can_disable {
            "auto/off".to_owned()
        } else {
            "auto".to_owned()
        };
    }
    reasoning.efforts.join(",")
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::Cache | Source::NotModified => "cache",
        Source::Network => "network",
        Source::StaleCache => "stale cache",
        Source::Bundled => "bundled snapshot",
    }
}

fn approval_label(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "ask",
        ApprovalMode::AutoEdit => "auto-edit",
        ApprovalMode::Yolo => "yolo",
    }
}

fn short(call_id: &str) -> &str {
    call_id.get(..8).unwrap_or(call_id)
}

fn scope_name(scope: BudgetScope) -> &'static str {
    match scope {
        BudgetScope::Message => "message",
        BudgetScope::Session => "session",
        BudgetScope::GlobalDaily => "today",
    }
}

fn unit_name(unit: BudgetUnit) -> &'static str {
    match unit {
        BudgetUnit::Usd => "USD",
        BudgetUnit::Tokens => "tokens",
        BudgetUnit::Requests => "requests",
    }
}

fn budget_action_name(action: BudgetAction) -> &'static str {
    match action {
        BudgetAction::Warn => "warn",
        BudgetAction::Ask => "ask",
        BudgetAction::Stop => "stop",
    }
}

fn format_measure(unit: BudgetUnit, value: f64) -> String {
    match unit {
        BudgetUnit::Usd => format!("${value:.6}"),
        BudgetUnit::Tokens | BudgetUnit::Requests => format!("{value:.0}"),
    }
}

fn budget_label(budget: &Budget) -> String {
    format!(
        "{} {} ({})",
        format_measure(budget.unit, budget.limit),
        unit_name(budget.unit),
        budget_action_name(budget.action)
    )
}

fn code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidRequest => "invalid request",
        ErrorCode::NotFound => "not found",
        ErrorCode::Conflict => "conflict",
        ErrorCode::Capability => "capability",
        ErrorCode::Budget => "budget",
        ErrorCode::Provider => "provider",
        ErrorCode::Internal => "internal",
        ErrorCode::Unsupported => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApprovalChoice, GrantScope, approval_request, argument_preview, budget_label, cost_summary,
        parse_approval_choice,
    };
    use norte_code_cost::CostSummary;
    use norte_code_protocol::{Budget, BudgetAction, BudgetUnit, ClientRequestKind, SessionKey};

    #[test]
    fn approval_answers_are_parsed_leniently() {
        assert_eq!(parse_approval_choice("y"), Some(ApprovalChoice::Once));
        assert_eq!(parse_approval_choice(" YES "), Some(ApprovalChoice::Once));
        assert_eq!(parse_approval_choice("A"), Some(ApprovalChoice::Session));
        assert_eq!(
            parse_approval_choice("always"),
            Some(ApprovalChoice::Session)
        );
        assert_eq!(parse_approval_choice("n"), Some(ApprovalChoice::Deny));
        assert_eq!(parse_approval_choice(""), Some(ApprovalChoice::Deny));
        assert_eq!(parse_approval_choice("maybe"), None);
    }

    #[test]
    fn short_arguments_stay_on_one_line() {
        let arguments = serde_json::json!({"path": "src/main.rs"});
        assert_eq!(
            argument_preview(&arguments),
            vec![r#"{"path":"src/main.rs"}"#.to_owned()]
        );
    }

    #[test]
    fn long_arguments_are_bounded() {
        let arguments = serde_json::json!({
            "path": "src/main.rs",
            "content": "x".repeat(500),
        });
        let lines = argument_preview(&arguments);
        assert!(lines.len() <= 11);
        assert!(lines.iter().all(|line| line.chars().count() <= 121));
        assert!(lines.iter().any(|line| line.ends_with('…')));
    }

    #[test]
    fn cost_totals_name_the_requests_without_a_reported_cost() {
        let mixed = CostSummary {
            requests: 2,
            reported_cost_usd: 0.5,
            not_reported: 1,
            ..CostSummary::default()
        };
        assert_eq!(
            cost_summary(&mixed),
            "2 request(s) · 0 in · 0 out · $0.500000 · 1 not reported"
        );
        let unpriced = CostSummary {
            requests: 2,
            not_reported: 2,
            ..CostSummary::default()
        };
        assert_eq!(
            cost_summary(&unpriced),
            "2 request(s) · 0 in · 0 out · cost not reported"
        );
    }

    #[test]
    fn budget_labels_say_what_is_measured_and_what_happens() {
        assert_eq!(
            budget_label(&Budget {
                unit: BudgetUnit::Requests,
                limit: 12.0,
                action: BudgetAction::Stop,
            }),
            "12 requests (stop)"
        );
    }

    #[test]
    fn always_grants_the_session_and_deny_carries_a_reason() {
        let session = SessionKey::new("s");
        let always = approval_request(
            session.clone(),
            "approval".to_owned(),
            ApprovalChoice::Session,
        );
        match always.kind {
            ClientRequestKind::Approve {
                scope, remember, ..
            } => {
                assert!(matches!(scope, Some(GrantScope::Session)));
                assert_eq!(remember, Some(true));
            }
            other => panic!("expected an approval, got {other:?}"),
        }
        let deny = approval_request(session, "approval".to_owned(), ApprovalChoice::Deny);
        match deny.kind {
            ClientRequestKind::Deny { reason, .. } => assert!(reason.is_some()),
            other => panic!("expected a denial, got {other:?}"),
        }
    }
}
