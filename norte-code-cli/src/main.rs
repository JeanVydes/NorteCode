//! The headless `norte-code` command.
//!
//! `norte-code run "prompt"` builds the service in memory, sends one message
//! and streams the answer. Text goes to stdout; diagnostics, reasoning and
//! tool activity go to stderr. `--jsonl` writes one protocol event per line on
//! stdout instead, for scripts and tests.
//!
//! `norte-code models` lists what the catalogue knows, filtered by provider,
//! tool calling or reasoning.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use futures_util::StreamExt;
use norte_code_agent::{
    DEFAULT_SYSTEM_PROMPT, ModelSpec, Service, ServiceError, VendorModels, model_list,
};
use norte_code_catalog::{Catalog, LoadOptions, Source, cache_dir, load_catalog};
use norte_code_config::{ApprovalMode, Config, user_config_path};
use norte_code_protocol::{
    Budget, BudgetAction, BudgetUnit, ClientRequest, ClientRequestKind, Delta, ErrorCode,
    ModelInfo, Reasoning, ReasoningInfo, ServerEvent, ServerEventKind, SessionKey,
};
use norte_code_skills::{SkillRegistry, discovery_roots};
use norte_code_store_sqlite::{SessionDirectory, SqliteStore};

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
    /// List the models the catalogue knows.
    Models(ModelsArgs),
    /// Inspect the installed Agent Skills.
    Skills(SkillsArgs),
    /// List the sessions stored on disk.
    Sessions(SessionsArgs),
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
struct SessionsArgs {
    /// Write one JSON object per line.
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
        command => {
            let (catalog, source) = load_catalogue(&config, offline, refresh).await?;
            match command {
                Command::Run(args) => run_prompt(config, catalog, source, args, verbose).await,
                Command::Models(args) => list_models(&config, &catalog, source, &args),
                Command::Skills(_) => unreachable!("skills were handled before the catalogue"),
                Command::Sessions(_) => {
                    unreachable!("sessions were handled before the catalogue")
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
    if !args.quiet && !args.jsonl {
        eprintln!(
            "· {} · catalogue from {} · approval {}",
            spec.label(),
            source_label(source),
            approval_label(config.approval_mode)
        );
    }
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
    let mut stream = service.submit(request).await.map_err(|e| e.to_string())?;

    let mut renderer = Renderer {
        format: if args.jsonl {
            Format::Jsonl
        } else {
            Format::Text
        },
        quiet: args.quiet,
        verbose,
        last_char: None,
        failed: false,
    };
    while let Some(event) = stream.next().await {
        answer_if_asked(&service, &event).await?;
        renderer.event(&event)?;
    }
    if matches!(renderer.format, Format::Text)
        && renderer.last_char.is_some_and(|last| last != '\n')
    {
        println!();
    }
    Ok(u8::from(renderer.failed))
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
struct Renderer {
    format: Format,
    quiet: bool,
    verbose: bool,
    last_char: Option<char>,
    failed: bool,
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
            ServerEventKind::ToolStarted { tool, call_id, .. } => {
                if !self.quiet {
                    eprintln!("· {tool} ({})", short(call_id));
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
                    let outcome = if *is_error { "failed" } else { "ok" };
                    eprintln!("· {tool} {outcome} in {duration_ms} ms");
                }
                if *is_error && self.verbose {
                    if let Some(error) = error {
                        eprintln!("  error: {error}");
                    }
                }
            }
            ServerEventKind::RequestReconstructed {
                usage,
                cost_usd,
                truncated,
                ..
            } => {
                if !self.quiet {
                    let mut parts = Vec::new();
                    if let Some(input) = usage.input_tokens {
                        parts.push(format!("{input} in"));
                    }
                    if let Some(output) = usage.output_tokens {
                        parts.push(format!("{output} out"));
                    }
                    if let Some(cost) = cost_usd {
                        parts.push(format!("${cost:.6}"));
                    }
                    if !parts.is_empty() {
                        eprintln!("· {}", parts.join(", "));
                    }
                    if *truncated {
                        eprintln!("warning: the output hit the token limit");
                    }
                }
            }
            ServerEventKind::CostNotReported { requests } => {
                eprintln!(
                    "warning: {requests} request(s) carried no cost; the total is incomplete"
                );
            }
            ServerEventKind::Error { code, message } => {
                eprintln!("error: {message} ({})", code_name(*code));
                self.failed = true;
            }
            ServerEventKind::ApprovalRequested { approval, tool, .. } => {
                if !self.quiet {
                    eprintln!(
                        "approval needed for {tool} ({approval}); resume from a client that can approve"
                    );
                }
            }
            ServerEventKind::ApprovalResolved { approved, .. } => {
                if !self.quiet {
                    eprintln!("approval {}", if *approved { "granted" } else { "denied" });
                }
            }
            ServerEventKind::TurnFinished {
                committed: false, ..
            } => {
                if self.last_char.is_none() {
                    eprintln!("norte-code: the turn ended without an answer");
                }
                self.failed = true;
            }
            _ => {}
        }
        Ok(())
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
                    eprint!("{text}");
                }
            }
            Delta::Refusal { text, .. } => {
                eprintln!("refusal: {text}");
                self.failed = true;
            }
            Delta::ToolArguments { .. } | Delta::Media { .. } => {}
        }
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
