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

mod style;

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
    DEFAULT_SYSTEM_PROMPT, EventStream, ModelSpec, Service, ServiceError, VendorModels,
    effective_reasoning, model_list,
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

async fn run(cli: Cli) -> Result<u8, String> {
    let config_path = cli.config.clone();
    let user_config = config_path.clone().or_else(user_config_path);
    let offline = cli.offline;
    let refresh = cli.refresh;
    let verbose = cli.verbose;
    let debug = cli.debug;
    let config = load_config(config_path)?;
    match cli.command {
        Command::Skills(args) => skills_command(&config, &args),
        Command::Sessions(args) => sessions_command(&args).await,
        Command::Cost(args) => cost_command(&config, &args).await,
        Command::Telemetry(args) => telemetry_command(&args).await,
        command => {
            let (catalog, source) = load_catalogue(&config, offline, refresh).await?;
            match command {
                Command::Run(args) => {
                    run_prompt(config, catalog, source, args, verbose, debug, user_config).await
                }
                Command::Resume(args) => {
                    resume_session(config, catalog, source, args, verbose, debug, user_config).await
                }
                Command::Models(args) => list_models(&config, &catalog, source, &args),
                Command::Skills(_)
                | Command::Sessions(_)
                | Command::Cost(_)
                | Command::Telemetry(_) => {
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
    if summary.cache_read_tokens > 0 && summary.input_tokens > 0 {
        parts.push(format!(
            "cache {}",
            percent_label(summary.cache_read_tokens, summary.input_tokens)
        ));
    }
    parts.push(format!(
        "in:out {}",
        token_ratio(summary.input_tokens, summary.output_tokens)
    ));
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

/// Counts and sums one slice of the request telemetry.
#[derive(Default, Clone)]
struct RequestSummary {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    cache_hits: u64,
    tool_calls: u64,
    cost_usd: f64,
    unpriced: u64,
    errors: u64,
    cancelled: u64,
    durations: Vec<u64>,
}

impl RequestSummary {
    fn add(&mut self, record: &RequestRecord) {
        self.requests += 1;
        self.input_tokens += record.input_tokens.unwrap_or(0);
        self.output_tokens += record.output_tokens.unwrap_or(0);
        self.reasoning_tokens += record.reasoning_tokens.unwrap_or(0);
        self.cache_read_tokens += record.cache_read_tokens.unwrap_or(0);
        self.cache_write_tokens += record.cache_write_tokens.unwrap_or(0);
        self.cache_hits += u64::from(record.cache_hit);
        self.tool_calls += u64::try_from(record.tool_calls.len()).unwrap_or(u64::MAX);
        if let Some(cost) = record.cost_usd {
            self.cost_usd += cost;
        } else {
            self.unpriced += 1;
        }
        match record.status.as_str() {
            "ok" => {}
            "cancelled" => self.cancelled += 1,
            _ => self.errors += 1,
        }
        self.durations.push(record.duration_ms);
    }

    fn average_ms(&self) -> u64 {
        if self.requests == 0 {
            return 0;
        }
        self.durations.iter().sum::<u64>() / self.requests
    }

    fn p95_ms(&mut self) -> u64 {
        if self.durations.is_empty() {
            return 0;
        }
        self.durations.sort_unstable();
        let index = (self.durations.len() * 95 / 100).min(self.durations.len() - 1);
        self.durations[index]
    }

    fn cost_label(&self) -> String {
        if self.requests == 0 {
            return "no requests".to_owned();
        }
        if self.unpriced == self.requests {
            return "cost not reported".to_owned();
        }
        if self.unpriced > 0 {
            return format!("${:.6} · {} not reported", self.cost_usd, self.unpriced);
        }
        format!("${:.6}", self.cost_usd)
    }

    fn line(&mut self) -> String {
        let mut parts = vec![
            format!("{} req", self.requests),
            format!("{} in", human_tokens(self.input_tokens)),
            format!("{} out", human_tokens(self.output_tokens)),
        ];
        if self.requests > 0 && self.cache_hits > 0 {
            parts.push(format!(
                "cache {} hit",
                percent_label(self.cache_hits, self.requests)
            ));
        }
        if self.tool_calls > 0 {
            parts.push(format!("{} tool(s)", self.tool_calls));
        }
        parts.push(format!(
            "latency {} avg / {} p95",
            duration_label(self.average_ms()),
            duration_label(self.p95_ms())
        ));
        parts.push(self.cost_label());
        if self.errors + self.cancelled > 0 {
            parts.push(format!("{} error(s)", self.errors + self.cancelled));
        }
        parts.join(" · ")
    }

    #[allow(clippy::cast_precision_loss)]
    fn json(&mut self) -> serde_json::Value {
        serde_json::json!({
            "requests": self.requests,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "reasoning_tokens": self.reasoning_tokens,
            "cache_read_tokens": self.cache_read_tokens,
            "cache_write_tokens": self.cache_write_tokens,
            "cache_hits": self.cache_hits,
            "cache_hit_rate": if self.requests == 0 { 0.0 } else { self.cache_hits as f64 / self.requests as f64 },
            "tool_calls": self.tool_calls,
            "cost_usd": self.cost_usd,
            "unpriced": self.unpriced,
            "errors": self.errors,
            "cancelled": self.cancelled,
            "average_ms": self.average_ms(),
            "p95_ms": self.p95_ms(),
        })
    }
}

#[allow(clippy::cast_precision_loss)]
fn duration_label(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{:.1}m", ms as f64 / 60_000.0)
    } else if ms >= 1_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{ms}ms")
    }
}

fn bucket_label(at: DateTime<Utc>, bucket: &str) -> String {
    match bucket {
        "hour" => at.format("%Y-%m-%d %H:00").to_string(),
        "month" => at.format("%Y-%m").to_string(),
        "year" => at.format("%Y").to_string(),
        "total" => "total".to_owned(),
        _ => at.format("%Y-%m-%d").to_string(),
    }
}

fn top(groups: &BTreeMap<String, RequestSummary>) -> Vec<(&String, &RequestSummary)> {
    let mut top: Vec<_> = groups.iter().collect();
    top.sort_by(|left, right| {
        right
            .1
            .requests
            .cmp(&left.1.requests)
            .then_with(|| left.0.cmp(right.0))
    });
    top
}

#[allow(clippy::too_many_lines)]
async fn telemetry_command(args: &TelemetryArgs) -> Result<u8, String> {
    let path = database_path();
    if !path.exists() {
        println!(
            "no request telemetry yet (the database is {})",
            path.display()
        );
        return Ok(0);
    }
    let store = SqliteStore::open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let records = store
        .requests(args.session.as_deref())
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let records: Vec<_> = records
        .into_iter()
        .filter(|record| args.provider.as_ref().is_none_or(|p| &record.provider == p))
        .filter(|record| args.model.as_ref().is_none_or(|m| &record.model == m))
        .collect();
    if records.is_empty() {
        println!("no matching request telemetry");
        return Ok(0);
    }
    let mut total = RequestSummary::default();
    let mut buckets: BTreeMap<String, RequestSummary> = BTreeMap::new();
    let mut providers: BTreeMap<String, RequestSummary> = BTreeMap::new();
    let mut models: BTreeMap<String, RequestSummary> = BTreeMap::new();
    let mut sessions: BTreeMap<String, RequestSummary> = BTreeMap::new();
    for record in &records {
        total.add(record);
        buckets
            .entry(bucket_label(record.started_at, &args.bucket))
            .or_default()
            .add(record);
        providers
            .entry(record.provider.clone())
            .or_default()
            .add(record);
        models
            .entry(format!("{}/{}", record.provider, record.model))
            .or_default()
            .add(record);
        sessions
            .entry(record.session.clone())
            .or_default()
            .add(record);
    }
    if args.json {
        let mut total_json = total.clone();
        let bucket_json: Vec<_> = buckets
            .iter_mut()
            .map(|(bucket, summary)| {
                serde_json::json!({
                    "bucket": bucket,
                    "summary": summary.json(),
                })
            })
            .collect();
        let provider_json: Vec<_> = top(&providers)
            .into_iter()
            .map(|(provider, summary)| {
                let mut summary = summary.clone();
                serde_json::json!({"provider": provider, "summary": summary.json()})
            })
            .collect();
        let model_json: Vec<_> = top(&models)
            .into_iter()
            .map(|(model, summary)| {
                let mut summary = summary.clone();
                serde_json::json!({"model": model, "summary": summary.json()})
            })
            .collect();
        let session_json: Vec<_> = top(&sessions)
            .into_iter()
            .map(|(session, summary)| {
                let mut summary = summary.clone();
                serde_json::json!({"session": session, "summary": summary.json()})
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "bucket": args.bucket,
                "total": total_json.json(),
                "series": bucket_json,
                "providers": provider_json,
                "models": model_json,
                "sessions": session_json,
            })
        );
        return Ok(0);
    }
    let style = Style::stdout();
    println!("{}  {}", style.bold("total"), total.line());
    println!();
    println!("{}", style.bold(format!("by {}", args.bucket)));
    for (bucket, summary) in &mut buckets {
        println!("  {bucket:<16}  {}", summary.line());
    }
    if args.session.is_none() {
        println!();
        println!("{}", style.bold("top providers"));
        for (provider, summary) in top(&providers).into_iter().take(10) {
            let mut summary = summary.clone();
            println!("  {provider:<16}  {}", summary.line());
        }
        println!();
        println!("{}", style.bold("top models"));
        for (model, summary) in top(&models).into_iter().take(10) {
            let mut summary = summary.clone();
            println!("  {model:<40}  {}", summary.line());
        }
        println!();
        println!("{}", style.bold("sessions"));
        for (session, summary) in top(&sessions).into_iter().take(20) {
            let mut summary = summary.clone();
            println!("  {}  {}", short(session), summary.line());
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

/// Applies `--extension KEY=JSON` values over the loaded config.
fn apply_extensions(config: &mut Config, values: &[String]) -> Result<(), String> {
    for value in values {
        let Some((key, raw)) = value.split_once('=') else {
            return Err(format!("--extension expects `KEY=JSON`, got `{value}`"));
        };
        if !key.contains(':') {
            return Err(format!(
                "--extension key `{key}` must name a wire family, as in `openai.chat:provider`"
            ));
        }
        let value =
            serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()));
        config.extensions.insert(key.to_owned(), value);
    }
    Ok(())
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

/// Reads and sends messages until the human exits, a turn parks or the budget
/// stops the session.
async fn interactive_loop(
    service: &Arc<Service>,
    mut session: Option<SessionKey>,
    renderer: &mut Renderer,
    style: Style,
    input: &mut mpsc::UnboundedReceiver<Option<String>>,
    message_budget: Option<Budget>,
    reasoning: Option<Reasoning>,
) -> Result<Option<SessionKey>, String> {
    if !renderer.quiet {
        eprintln!(
            "{}",
            style.dim("interactive: type /compact to summarise, /exit or Ctrl-D to leave")
        );
    }
    let mut request_id = 2_u64;
    loop {
        let Some(text) = read_repl_line(input, style).await? else {
            break;
        };
        if text == "/compact" {
            let Some(current) = session.clone() else {
                eprintln!(
                    "{}",
                    style.yellow("there is no session to compact yet; send a message first")
                );
                continue;
            };
            let stream = service
                .submit(ClientRequest {
                    request_id,
                    kind: ClientRequestKind::Compact { session: current },
                })
                .await
                .map_err(|error| error.to_string())?;
            request_id = request_id.saturating_add(1);
            let outcome = drive_turns(service, stream, renderer, style, Some(&mut *input)).await?;
            session = outcome.session.or(session);
            renderer.ensure_newline();
            if outcome.parked || renderer.budget_stopped {
                break;
            }
            continue;
        }
        let stream = service
            .submit(ClientRequest {
                request_id,
                kind: ClientRequestKind::SendMessage {
                    session: session.clone(),
                    text,
                    budget: message_budget.clone(),
                    reasoning: reasoning.clone(),
                },
            })
            .await
            .map_err(|error| error.to_string())?;
        request_id = request_id.saturating_add(1);
        let outcome = drive_turns(service, stream, renderer, style, Some(&mut *input)).await?;
        session = outcome.session.or(session);
        renderer.ensure_newline();
        if outcome.parked || renderer.budget_stopped {
            break;
        }
    }
    Ok(session)
}

/// Prints the interactive prompt and waits for the next message.
///
/// Ctrl-C, Ctrl-D, `/exit` and `quit` all leave the loop without inventing a
/// message. Empty lines are ignored.
async fn read_repl_line(
    input: &mut mpsc::UnboundedReceiver<Option<String>>,
    style: Style,
) -> Result<Option<String>, String> {
    loop {
        eprint!("{} ", style.cyan("›"));
        let _ = std::io::stderr().flush();
        let line = tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                eprintln!();
                return Ok(None);
            }
            line = input.recv() => line,
        };
        let Some(Some(line)) = line else {
            eprintln!();
            return Ok(None);
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line, "/exit" | "/quit" | "exit" | "quit") {
            return Ok(None);
        }
        return Ok(Some(line.to_owned()));
    }
}

#[derive(Debug, Default)]
struct DriveOutcome {
    /// The session seen on the stream, if the stream carried one.
    session: Option<SessionKey>,
    /// The turn stopped with an approval still waiting for a human.
    parked: bool,
}

/// Streams the primary turn, then any turns resumed by an approval.
///
/// A turn may park for several approvals; each decision resumes on its own
/// stream, which can park again, so the loop drains a queue of streams.
#[allow(clippy::too_many_lines)]
async fn drive_turns(
    service: &Arc<Service>,
    primary: EventStream,
    renderer: &mut Renderer,
    style: Style,
    mut input: Option<&mut mpsc::UnboundedReceiver<Option<String>>>,
) -> Result<DriveOutcome, String> {
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
    let mut parked = false;
    let mut cancel_requested: Option<std::time::Instant> = None;
    while let Some(mut stream) = streams.pop_front() {
        loop {
            let event = tokio::select! {
                biased;
                Some(()) = interrupt_rx.recv() => {
                    let Some(session) = current.clone() else { continue };
                    match cancel_requested {
                        None => {
                            cancel_requested = Some(std::time::Instant::now());
                            renderer.mark_cancelled();
                            eprintln!(
                                "{}",
                                style.yellow("cancelling the running turn…")
                            );
                            eprintln!(
                                "  {}",
                                style.dim(
                                    "the turn is saved and can be resumed; press Ctrl-C again \
                                     after 5s to force quit"
                                )
                            );
                            let request = ClientRequest {
                                request_id: 0,
                                kind: ClientRequestKind::CancelTurn { session },
                            };
                            if let Err(error) = service.submit(request).await {
                                eprintln!(
                                    "{}",
                                    style.yellow(format!(
                                        "warning: could not cancel the turn: {error}"
                                    ))
                                );
                            }
                        }
                        Some(at) if at.elapsed() < Duration::from_secs(5) => {
                            eprintln!(
                                "{}",
                                style.yellow("still cancelling…")
                            );
                            eprintln!(
                                "  {}",
                                style.dim(
                                    "a blocking tool may take a moment to stop; Ctrl-C again \
                                     after 5s will force quit"
                                )
                            );
                        }
                        Some(_) => {
                            eprintln!("{}", style.red("interrupted; forcing exit"));
                            signal.abort();
                            return Err("interrupted".to_owned());
                        }
                    }
                    continue;
                }
                event = stream.next() => event,
            };
            let Some(event) = event else { break };
            if let Some(session) = &event.session {
                current = Some(session.clone());
            }
            match &event.kind {
                ServerEventKind::TurnStarted { .. } => parked = false,
                ServerEventKind::TurnFinished { phase, .. } => {
                    parked = *phase == Phase::AwaitingApproval;
                }
                _ => {}
            }
            match ask_approval(&event, style, input.as_deref_mut(), &mut interrupt_rx).await? {
                ApprovalOutcome::Decision(decision) => pending.push_back(decision),
                ApprovalOutcome::None => {
                    if matches!(&event.kind, ServerEventKind::ApprovalRequested { .. }) {
                        parked = true;
                    }
                }
                ApprovalOutcome::Cancelled => {
                    renderer.mark_cancelled();
                    eprintln!(
                        "{}",
                        style.yellow(
                            "approval left pending; the session is saved and can be resumed"
                        )
                    );
                    signal.abort();
                    return Ok(DriveOutcome {
                        session: current,
                        parked: true,
                    });
                }
            }
            if answer_if_asked(service, &event, input.as_deref_mut(), &mut interrupt_rx).await? {
                renderer.mark_cancelled();
                eprintln!(
                    "{}",
                    style.yellow("question left pending; the session is saved and can be resumed")
                );
                signal.abort();
                return Ok(DriveOutcome {
                    session: current,
                    parked: true,
                });
            }
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
    Ok(DriveOutcome {
        session: current,
        parked,
    })
}

/// What the human chose for a parked call.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ApprovalChoice {
    /// Run this call once.
    Once,
    /// Run any call to this tool for the rest of the session.
    ToolSession,
    /// Run this tool in every session, stored in the user config.
    ToolGlobal,
    /// Refuse this call, with the reason the human typed, if any.
    DenyOnce(Option<String>),
    /// Refuse this tool for the rest of the session.
    DenySession(Option<String>),
    /// Refuse this tool in every session, stored in the user config.
    DenyGlobal(Option<String>),
}

/// What happened at an approval panel.
enum ApprovalOutcome {
    /// A decision to submit once the stream is drained.
    Decision((SessionKey, String, ApprovalChoice)),
    /// No decision: no interactive client, or the event was not an approval.
    None,
    /// The human pressed Ctrl-C while the prompt waited for input.
    Cancelled,
}

/// Prints an approval request and asks for a decision when stdin is a terminal.
///
/// Input is read by a background thread and selected against the Ctrl-C
/// channel, so a parked approval cannot swallow the interrupt. A cancelled
/// prompt leaves the decision pending and the session resumable.
async fn ask_approval(
    event: &ServerEvent,
    style: Style,
    input: Option<&mut mpsc::UnboundedReceiver<Option<String>>>,
    interrupts: &mut mpsc::Receiver<()>,
) -> Result<ApprovalOutcome, String> {
    let ServerEventKind::ApprovalRequested {
        approval,
        tool,
        call_id,
        arguments,
        ..
    } = &event.kind
    else {
        return Ok(ApprovalOutcome::None);
    };
    let Some(session) = event.session.clone() else {
        return Ok(ApprovalOutcome::None);
    };
    eprintln!();
    let rule = style.yellow("─".repeat(rule_width()));
    eprintln!("{rule}");
    eprintln!(
        "{}  {}  {}",
        style.yellow("approval required"),
        style.tool(tool),
        style.dim(format!("#{}", short(call_id)))
    );
    if let Some(arguments) = arguments {
        for line in argument_preview(tool, arguments) {
            match line {
                PreviewLine::Field { key, value } => {
                    eprintln!("  {} {}", style.dim(format!("{key:<10}")), value);
                }
                PreviewLine::Block { label, lines } => {
                    eprintln!("  {}", style.dim(label));
                    for line in lines {
                        eprintln!("  {} {}", style.cyan("│"), line);
                    }
                }
                PreviewLine::Note(note) => {
                    eprintln!("  {}", style.dim(note));
                }
            }
        }
    }
    eprintln!("{rule}");
    let Some(input) = input else {
        eprintln!(
            "  {}",
            style.dim(
                "not an interactive terminal; the session stays parked. Use --approval-mode \
                 auto-edit or yolo to run without asking."
            )
        );
        return Ok(ApprovalOutcome::None);
    };
    loop {
        eprintln!(
            "  {}  {}  {}",
            style.green("[y] allow once"),
            style.cyan("[s] allow this tool for this session"),
            style.cyan("[a] allow this tool always")
        );
        eprintln!(
            "  {}  {}  {}",
            style.red("[n] deny once"),
            style.red("[ns] deny this tool for this session"),
            style.red("[ng] deny this tool always")
        );
        match prompt_input(&format!("  {} ", style.dim("›")), &mut *input, interrupts).await? {
            PromptInput::Line(line) => {
                if let Some(choice) = parse_approval_choice(&line) {
                    return Ok(ApprovalOutcome::Decision((
                        session,
                        approval.clone(),
                        choice,
                    )));
                }
            }
            PromptInput::Eof => {
                return Ok(ApprovalOutcome::Decision((
                    session,
                    approval.clone(),
                    ApprovalChoice::DenyOnce(None),
                )));
            }
            PromptInput::Cancelled => return Ok(ApprovalOutcome::Cancelled),
        }
        eprintln!(
            "  {}",
            style.dim("answer y, s, a, n, ns or ng; add a message after n to tell the model why")
        );
    }
}

/// Parses one line of the approval prompt. `None` means ask again.
///
/// `n <message>` is a denial that reaches the model, so "no, use the existing
/// helper" is actionable feedback rather than a bare refusal. `ns` and `ng`
/// make the denial last for the session or for every session; the same suffixes
/// on `s` and `a` pick the matching allow scope.
fn parse_approval_choice(line: &str) -> Option<ApprovalChoice> {
    let line = line.trim();
    let (command, reason) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(command, reason)| (command, reason.trim()));
    let reason = (!reason.is_empty()).then(|| reason.to_owned());
    match command.to_ascii_lowercase().as_str() {
        "y" | "yes" | "1" if reason.is_none() => Some(ApprovalChoice::Once),
        "s" | "session" | "2" if reason.is_none() => Some(ApprovalChoice::ToolSession),
        "a" | "always" | "g" | "3" if reason.is_none() => Some(ApprovalChoice::ToolGlobal),
        "n" | "no" | "4" if reason.is_none() => Some(ApprovalChoice::DenyOnce(None)),
        "n" | "no" => Some(ApprovalChoice::DenyOnce(reason)),
        "ns" | "5" => Some(ApprovalChoice::DenySession(reason)),
        "ng" | "6" => Some(ApprovalChoice::DenyGlobal(reason)),
        "" => Some(ApprovalChoice::DenyOnce(None)),
        _ => None,
    }
}

/// One line in the approval panel, already split from the raw JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PreviewLine {
    /// A named scalar: `path`, `cwd`, `pattern`.
    Field { key: String, value: String },
    /// A multi-line value shown in a gutter.
    Block { label: String, lines: Vec<String> },
    /// A quiet note about a flag or a bound.
    Note(String),
}

/// Renders a tool call as something a human can read at a glance.
///
/// The approval prompt must never be a JSON dump: for the common tools the
/// useful parts are the path, the command and a short excerpt of the text that
/// will be written.
fn argument_preview(tool: &str, arguments: &serde_json::Value) -> Vec<PreviewLine> {
    let Some(object) = arguments.as_object() else {
        return vec![PreviewLine::Note(compact_json(arguments, 200))];
    };
    let lines = match tool {
        "command" => command_preview(object),
        "read" => read_preview(object),
        "write" => write_preview(object),
        "edit" => edit_preview(object),
        "glob" | "grep" | "list_dir" => path_tool_preview(tool, object),
        "ask_user" => ask_preview(object),
        _ => generic_preview(object),
    };
    if lines.is_empty() {
        vec![PreviewLine::Note(compact_json(arguments, 200))]
    } else {
        lines
    }
}

fn command_preview(object: &serde_json::Map<String, serde_json::Value>) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(command) = object.get("command").and_then(serde_json::Value::as_object) {
        match command.get("kind").and_then(serde_json::Value::as_str) {
            Some("shell") => {
                if let Some(shell) = command.get("shell").and_then(serde_json::Value::as_str) {
                    lines.push(field("shell", shell));
                }
                if let Some(script) = command.get("script").and_then(serde_json::Value::as_str) {
                    lines.push(block("script", script, 14));
                }
            }
            Some("program") => {
                if let Some(program) = command.get("program").and_then(serde_json::Value::as_str) {
                    lines.push(field("program", program));
                }
                if let Some(args) = command.get("args").and_then(serde_json::Value::as_array) {
                    let args: Vec<&str> =
                        args.iter().filter_map(serde_json::Value::as_str).collect();
                    if !args.is_empty() {
                        let joined = args.join(" ");
                        lines.push(field("args", &joined));
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(cwd) = object.get("cwd").and_then(serde_json::Value::as_str) {
        lines.push(field("cwd", cwd));
    }
    if let Some(env) = object.get("env").and_then(serde_json::Value::as_object) {
        if !env.is_empty() {
            lines.push(PreviewLine::Note(format!(
                "{} environment variable(s)",
                env.len()
            )));
        }
    }
    if let Some(stdin) = object.get("stdin").and_then(serde_json::Value::as_str) {
        lines.push(block("stdin", stdin, 6));
    }
    if let Some(timeout) = object.get("timeout_ms").and_then(serde_json::Value::as_u64) {
        lines.push(PreviewLine::Note(format!("timeout {timeout} ms")));
    }
    if object
        .get("merge_stderr")
        .and_then(serde_json::Value::as_bool)
        .is_some_and(|merge| merge)
    {
        lines.push(PreviewLine::Note("stderr is merged into stdout".to_owned()));
    }
    lines
}

fn read_preview(object: &serde_json::Map<String, serde_json::Value>) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(path) = object.get("path").and_then(serde_json::Value::as_str) {
        lines.push(field("path", path));
    }
    if let Some(range) = object.get("lines").and_then(serde_json::Value::as_object) {
        let start = range
            .get("start")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1);
        match range.get("end").and_then(serde_json::Value::as_u64) {
            Some(end) => lines.push(PreviewLine::Note(format!("lines {start}–{end}"))),
            None => lines.push(PreviewLine::Note(format!("lines {start} to the end"))),
        }
    }
    if let Some(tail) = object.get("tail").and_then(serde_json::Value::as_u64) {
        lines.push(PreviewLine::Note(format!("last {tail} line(s)")));
    }
    if let Some(pattern) = object.get("pattern").and_then(serde_json::Value::as_str) {
        lines.push(field("pattern", pattern));
    }
    if let Some(context) = object.get("context").and_then(serde_json::Value::as_u64) {
        lines.push(PreviewLine::Note(format!("{context} line(s) of context")));
    }
    for (key, label) in [
        ("ignore_case", "ignore case"),
        ("invert", "invert the match"),
        ("count_only", "count only"),
    ] {
        if object
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .is_some_and(|set| set)
        {
            lines.push(PreviewLine::Note(label.to_owned()));
        }
    }
    lines
}

fn write_preview(object: &serde_json::Map<String, serde_json::Value>) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(path) = object.get("path").and_then(serde_json::Value::as_str) {
        lines.push(field("path", path));
    }
    if object
        .get("overwrite")
        .and_then(serde_json::Value::as_bool)
        .is_some_and(|overwrite| overwrite)
    {
        lines.push(PreviewLine::Note(
            "replaces the file if it exists".to_owned(),
        ));
    }
    if let Some(content) = object.get("content").and_then(serde_json::Value::as_str) {
        lines.push(block("content", content, 12));
    }
    lines
}

fn edit_preview(object: &serde_json::Map<String, serde_json::Value>) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(path) = object.get("path").and_then(serde_json::Value::as_str) {
        lines.push(field("path", path));
    }
    if let Some(edits) = object.get("edits").and_then(serde_json::Value::as_array) {
        for (index, edit) in edits.iter().enumerate() {
            let Some(edit) = edit.as_object() else {
                continue;
            };
            let prefix = if edits.len() > 1 {
                format!("edit {} · ", index + 1)
            } else {
                String::new()
            };
            if let Some(search) = edit.get("search").and_then(serde_json::Value::as_str) {
                lines.push(block(&format!("{prefix}search"), search, 8));
            }
            if let Some(replace) = edit.get("replace").and_then(serde_json::Value::as_str) {
                lines.push(block(&format!("{prefix}replace"), replace, 8));
            }
        }
    }
    lines
}

fn path_tool_preview(
    tool: &str,
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(path) = object.get("path").and_then(serde_json::Value::as_str) {
        lines.push(field("path", path));
    }
    if let Some(pattern) = object.get("pattern").and_then(serde_json::Value::as_str) {
        lines.push(field("pattern", pattern));
    }
    if let Some(include) = object.get("include").and_then(serde_json::Value::as_str) {
        lines.push(field("include", include));
    }
    if object
        .get("ignore_case")
        .and_then(serde_json::Value::as_bool)
        .is_some_and(|set| set)
    {
        lines.push(PreviewLine::Note("ignore case".to_owned()));
    }
    if tool == "list_dir" && lines.is_empty() {
        lines.push(PreviewLine::Note("the workspace root".to_owned()));
    }
    lines
}

fn ask_preview(object: &serde_json::Map<String, serde_json::Value>) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(question) = object.get("question").and_then(serde_json::Value::as_str) {
        lines.push(block("question", question, 12));
    }
    if let Some(options) = object.get("options").and_then(serde_json::Value::as_array) {
        let choices: Vec<&str> = options
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        if !choices.is_empty() {
            let multiple = object
                .get("multiple")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            lines.push(PreviewLine::Note(format!(
                "{} choice(s), {}",
                choices.len(),
                if multiple { "several allowed" } else { "one" }
            )));
            for (index, choice) in choices.iter().enumerate() {
                lines.push(field(&format!("{}.", index + 1), choice));
            }
        }
    }
    lines
}

fn generic_preview(object: &serde_json::Map<String, serde_json::Value>) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    for (key, value) in object.iter().take(10) {
        match value {
            serde_json::Value::String(text) if text.contains('\n') || text.len() > 120 => {
                lines.push(block(key, text, 8));
            }
            serde_json::Value::String(text) => lines.push(field(key, text)),
            serde_json::Value::Null => {}
            other => lines.push(field(key, &compact_json(other, 120))),
        }
    }
    if object.len() > 10 {
        lines.push(PreviewLine::Note(format!(
            "… {} more field(s)",
            object.len() - 10
        )));
    }
    lines
}

fn field(key: &str, value: &str) -> PreviewLine {
    PreviewLine::Field {
        key: key.to_owned(),
        value: value.to_owned(),
    }
}

fn block(label: &str, text: &str, max_lines: usize) -> PreviewLine {
    let mut lines: Vec<String> = text
        .lines()
        .take(max_lines)
        .map(|line| clip_line(line, 110))
        .collect();
    if text.lines().count() > max_lines {
        lines.push(format!(
            "… {} more line(s)",
            text.lines().count() - max_lines
        ));
    }
    PreviewLine::Block {
        label: label.to_owned(),
        lines,
    }
}

fn compact_json(value: &serde_json::Value, max: usize) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    clip_line(&text, max)
}

fn clip_line(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let mut shortened: String = line.chars().take(max).collect();
    shortened.push('…');
    shortened
}

/// Shortens a multi-line block on a character boundary.
fn clip_text(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_owned(), false);
    }
    let mut shortened: String = text.chars().take(max).collect();
    shortened.push_str("\n… [truncated]");
    (shortened, true)
}

fn rule_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|columns| columns.parse::<usize>().ok())
        .map_or(72, |columns| columns.saturating_sub(2).clamp(40, 100))
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
        ApprovalChoice::ToolSession => ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: Some(GrantScope::Tool),
            remember: Some(true),
        },
        ApprovalChoice::ToolGlobal => ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: Some(GrantScope::Always),
            remember: Some(true),
        },
        ApprovalChoice::DenyOnce(reason) => ClientRequestKind::Deny {
            session,
            approval,
            reason,
            scope: Some(DenyScope::Once),
        },
        ApprovalChoice::DenySession(reason) => ClientRequestKind::Deny {
            session,
            approval,
            reason,
            scope: Some(DenyScope::ToolSession),
        },
        ApprovalChoice::DenyGlobal(reason) => ClientRequestKind::Deny {
            session,
            approval,
            reason,
            scope: Some(DenyScope::ToolGlobal),
        },
    };
    ClientRequest {
        request_id: 0,
        kind,
    }
}

/// Answers a `QuestionAsked` event, prompting on a terminal and leaving a note
/// when the client is not interactive.
///
/// With closed `options`, the prompt is a numbered list plus an automatic
/// "Something else" entry. Picking that entry (or `other`) asks for free-form
/// text, which travels through the same [`ClientRequestKind::AnswerQuestion`]
/// channel as a plain answer. A multi-select answer joins the chosen option
/// strings with `, `.
async fn answer_if_asked(
    service: &Arc<Service>,
    event: &ServerEvent,
    input: Option<&mut mpsc::UnboundedReceiver<Option<String>>>,
    interrupts: &mut mpsc::Receiver<()>,
) -> Result<bool, String> {
    let ServerEventKind::QuestionAsked {
        question,
        prompt,
        options,
        multiple,
        ..
    } = &event.kind
    else {
        return Ok(false);
    };
    let session = event
        .session
        .clone()
        .ok_or("a question arrived without a session")?;
    let question = question.clone();
    eprintln!("? {prompt}");
    let Some(input) = input else {
        eprintln!("(no interactive client; answering with a note)");
        let answer =
            "(no interactive client is attached; continue with your best judgment)".to_owned();
        return submit_answer(service, session, question, answer).await;
    };
    let answer = if options.is_empty() {
        match prompt_input("answer> ", &mut *input, interrupts).await? {
            PromptInput::Line(line) => line,
            PromptInput::Eof => {
                "(no interactive client is attached; continue with your best judgment)".to_owned()
            }
            PromptInput::Cancelled => return Ok(true),
        }
    } else {
        for (index, option) in options.iter().enumerate() {
            eprintln!("  {}. {option}", index + 1);
        }
        eprintln!("  {}. Something else (type it)", options.len() + 1);
        eprintln!(
            "  {}",
            if *multiple {
                "choose one or more numbers, separated by commas"
            } else {
                "choose a number, or type an option"
            }
        );
        loop {
            let line = match prompt_input("choice> ", &mut *input, interrupts).await? {
                PromptInput::Line(line) => line,
                PromptInput::Eof => {
                    break "(no interactive client is attached; continue with your best judgment)"
                        .to_owned();
                }
                PromptInput::Cancelled => return Ok(true),
            };
            let Some((selected, other)) = parse_selection(&line, options, *multiple) else {
                eprintln!("  choose a listed number");
                continue;
            };
            let extra = if other {
                match prompt_input("something else> ", &mut *input, interrupts).await? {
                    PromptInput::Line(text) => Some(text),
                    PromptInput::Eof => {
                        break "(no interactive client is attached; continue with your best judgment)"
                            .to_owned();
                    }
                    PromptInput::Cancelled => return Ok(true),
                }
            } else {
                None
            };
            if selected.is_empty() && extra.as_deref().is_none_or(str::is_empty) {
                eprintln!("  type an answer, or choose a listed number");
                continue;
            }
            break compose_selection(options, &selected, extra.as_deref());
        }
    };
    submit_answer(service, session, question, answer).await
}

/// Sends one answer and drains its acknowledgement.
async fn submit_answer(
    service: &Arc<Service>,
    session: SessionKey,
    question: String,
    answer: String,
) -> Result<bool, String> {
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
    Ok(false)
}

/// The three outcomes of waiting for one line of input.
enum PromptInput {
    /// A line, with its trailing newline removed.
    Line(String),
    /// End of input, or the reader thread stopped.
    Eof,
    /// Ctrl-C arrived before a line did.
    Cancelled,
}

/// Reads lines on a blocking thread.
///
/// The async prompt owns only the receiver, so it can select a line against the
/// Ctrl-C channel. A read that is already blocked stays on the thread; the
/// process exits or resumes without waiting for it.
fn stdin_lines() -> mpsc::UnboundedReceiver<Option<String>> {
    let (sender, receiver) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        loop {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = sender.send(None);
                    break;
                }
                Ok(_) => {
                    if sender.send(Some(line.trim_end().to_owned())).is_err() {
                        break;
                    }
                }
            }
        }
    });
    receiver
}

/// Prints a prompt and waits for one line or Ctrl-C.
async fn prompt_input(
    prompt: &str,
    input: &mut mpsc::UnboundedReceiver<Option<String>>,
    interrupts: &mut mpsc::Receiver<()>,
) -> Result<PromptInput, String> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let line = tokio::select! {
        biased;
        Some(()) = interrupts.recv() => return Ok(PromptInput::Cancelled),
        line = input.recv() => line,
    };
    match line {
        Some(Some(line)) => Ok(PromptInput::Line(line)),
        Some(None) | None => Ok(PromptInput::Eof),
    }
}

/// Parses a numbered selection. Returns the zero-based option indexes and
/// whether the automatic "Something else" entry was chosen.
fn parse_selection(line: &str, options: &[String], multiple: bool) -> Option<(Vec<usize>, bool)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if !multiple {
        if line.eq_ignore_ascii_case("other") || line.eq_ignore_ascii_case("o") {
            return Some((Vec::new(), true));
        }
        if let Some(index) = options
            .iter()
            .position(|option| option.eq_ignore_ascii_case(line))
        {
            return Some((vec![index], false));
        }
    }
    let tokens: Vec<&str> = line
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() || (!multiple && tokens.len() != 1) {
        return None;
    }
    if tokens.len() == 1 && tokens[0].eq_ignore_ascii_case("all") {
        return multiple.then(|| ((0..options.len()).collect(), false));
    }
    let mut selected = Vec::new();
    let mut other = false;
    for token in tokens {
        if token.eq_ignore_ascii_case("other") || token.eq_ignore_ascii_case("o") {
            if !multiple && other {
                return None;
            }
            other = true;
        } else if let Ok(number) = token.parse::<usize>() {
            if number == 0 || number > options.len() + 1 {
                return None;
            }
            if number == options.len() + 1 {
                if !multiple && other {
                    return None;
                }
                other = true;
            } else if !selected.contains(&(number - 1)) {
                selected.push(number - 1);
            }
        } else {
            return None;
        }
    }
    if selected.is_empty() && !other {
        None
    } else {
        selected.sort_unstable();
        Some((selected, other))
    }
}

/// Renders a selection as the answer the model reads.
fn compose_selection(options: &[String], selected: &[usize], other: Option<&str>) -> String {
    let mut parts: Vec<String> = selected
        .iter()
        .filter_map(|index| options.get(*index).cloned())
        .collect();
    if let Some(text) = other.filter(|text| !text.is_empty()) {
        parts.push(text.to_owned());
    }
    parts.join(", ")
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

/// Which streamed block the renderer is in, so reasoning never runs inline
/// with the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    /// Nothing is streaming.
    None,
    /// Model reasoning, shown in a dim gutter on stderr.
    Reasoning,
    /// The answer, printed plainly on stdout.
    Answer,
}

/// Renders one server event to the terminal.
#[allow(clippy::struct_excessive_bools)]
struct Renderer {
    format: Format,
    quiet: bool,
    verbose: bool,
    debug: bool,
    debug_session: bool,
    show_cost: bool,
    style: Style,
    block: Block,
    reasoning_line_start: bool,
    /// Whether `--quiet` has already printed its one redacted reasoning marker
    /// for this turn.
    reasoning_placeholder: bool,
    last_char: Option<char>,
    failed: bool,
    cancelled: bool,
    budget_stopped: bool,
    requests: u64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cache_reported: bool,
    cost: f64,
    unpriced: u64,
    tool_counts: BTreeMap<String, u64>,
}

impl Renderer {
    fn event(&mut self, event: &ServerEvent) -> Result<(), String> {
        if self.debug && !self.debug_session {
            if let Some(session) = &event.session {
                eprintln!("{}", self.style.dim(format!("debug: session {session}")));
                self.debug_session = true;
            }
        }
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

    /// Notes that a cancellation was requested, so the turn's end reads as a
    /// cancellation rather than a failure.
    fn mark_cancelled(&mut self) {
        self.cancelled = true;
    }

    /// Ends the current output line before a prompt or the final summary.
    fn ensure_newline(&mut self) {
        if matches!(self.format, Format::Text) && self.last_char.is_some_and(|last| last != '\n') {
            println!();
            self.last_char = Some('\n');
        }
    }

    /// Closes a reasoning gutter, if one is open.
    fn end_reasoning(&mut self) {
        if self.block != Block::Reasoning {
            return;
        }
        if !self.reasoning_line_start {
            eprintln!();
        }
        eprintln!("  {}", self.style.faint("╰"));
        self.block = Block::None;
        self.last_char = Some('\n');
    }

    /// Finishes any streamed block and leaves one blank line.
    fn end_blocks(&mut self) {
        self.reasoning_placeholder = false;
        self.end_reasoning();
        if self.block == Block::Answer && self.last_char != Some('\n') {
            println!();
        }
        if self.block != Block::None {
            self.block = Block::None;
            self.last_char = Some('\n');
        }
        if !self.quiet && self.last_char.is_some() {
            eprintln!();
        }
    }

    /// Prints a single blank separator line.
    fn space(&mut self) {
        if self.quiet {
            return;
        }
        if self.last_char != Some('\n') {
            println!();
        }
        eprintln!();
        self.last_char = Some('\n');
    }

    /// Writes reasoning into its own dim gutter.
    fn reasoning_delta(&mut self, text: &str) {
        let mut rest = text;
        while !rest.is_empty() {
            if self.reasoning_line_start {
                eprint!("  {} ", self.style.faint("│"));
                self.reasoning_line_start = false;
            }
            if let Some(index) = rest.find('\n') {
                let (line, tail) = rest.split_at(index);
                eprint!("{}", self.style.thought(line));
                eprintln!();
                self.last_char = Some('\n');
                self.reasoning_line_start = true;
                rest = &tail[1..];
            } else {
                eprint!("{}", self.style.thought(rest));
                self.last_char = rest.chars().last();
                rest = "";
            }
        }
        let _ = std::io::stderr().flush();
    }

    /// Writes answer text, closing reasoning and separating it first.
    fn answer_delta(&mut self, text: &str) {
        if self.quiet {
            print!("{text}");
            if let Some(last) = text.chars().last() {
                self.last_char = Some(last);
            }
            let _ = std::io::stdout().flush();
            return;
        }
        if self.block == Block::Reasoning {
            self.end_reasoning();
            self.space();
        } else if self.block != Block::Answer && !text.is_empty() {
            if self.last_char.is_some() {
                self.space();
            }
            self.block = Block::Answer;
        }
        self.block = Block::Answer;
        print!("{text}");
        if let Some(last) = text.chars().last() {
            self.last_char = Some(last);
        }
        let _ = std::io::stdout().flush();
    }

    /// Renders a fenced block, so verbose tool activity stays readable and can
    /// be copied into a markdown report.
    fn code_block(&self, language: &str, text: &str) {
        eprintln!("  {}", self.style.faint(format!("```{language}")));
        if text.is_empty() {
            eprintln!("  {}", self.style.faint("(empty)"));
        } else {
            for line in text.lines() {
                eprintln!("  {line}");
            }
        }
        eprintln!("  {}", self.style.faint("```"));
    }

    /// Renders tool activity: what started, and how it ended.
    #[allow(clippy::collapsible_match)]
    fn tool_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::ToolStarted {
                tool,
                call_id,
                arguments,
                ..
            } => {
                *self.tool_counts.entry(tool.clone()).or_default() += 1;
                if !self.quiet {
                    self.end_blocks();
                    eprintln!(
                        "{} {} {}",
                        self.style.cyan("→"),
                        self.style.tool(tool),
                        self.style.faint(format!("#{}", short(call_id)))
                    );
                    if self.verbose && tool.as_str() != "ask_user" {
                        if let Some(arguments) = arguments {
                            let text = serde_json::to_string_pretty(arguments)
                                .unwrap_or_else(|_| compact_json(arguments, 4_000));
                            let (text, truncated) = clip_text(&text, 4_000);
                            self.code_block("json", &text);
                            if truncated {
                                eprintln!(
                                    "  {}",
                                    self.style.yellow("arguments shortened for display")
                                );
                            }
                        }
                    }
                }
            }
            ServerEventKind::ToolFinished {
                tool,
                is_error,
                error,
                output,
                truncated,
                duration_ms,
                ..
            } => {
                if !self.quiet {
                    if *is_error {
                        eprintln!(
                            "{} {} {}",
                            self.style.red("✗"),
                            self.style.bold(tool),
                            self.style.dim(format!("failed in {duration_ms} ms"))
                        );
                    } else {
                        eprintln!(
                            "{} {} {}",
                            self.style.green("✓"),
                            self.style.bold(tool),
                            self.style.dim(format!("in {duration_ms} ms"))
                        );
                    }
                    if self.verbose {
                        if *is_error {
                            if let Some(error) = error {
                                self.code_block("text", error);
                            }
                        } else if let Some(output) = output {
                            self.code_block("text", output);
                            if *truncated {
                                eprintln!(
                                    "  {}",
                                    self.style.yellow("output shortened for display")
                                );
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Renders usage for one reconstructed request and keeps the run totals.
    #[allow(clippy::too_many_lines)]
    fn usage_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::RequestReconstructed {
                usage,
                cost_usd,
                truncated,
                duration_ms,
                first_token_ms,
                tokens_per_second,
                cache_hit,
                tools,
                ..
            } => {
                self.requests += 1;
                self.input += usage.input_tokens.unwrap_or(0);
                self.output += usage.output_tokens.unwrap_or(0);
                if let Some(cache) = usage.cache_read_tokens {
                    self.cache_read += cache;
                    self.cache_reported = true;
                }
                if let Some(cache) = usage.cache_write_tokens {
                    self.cache_write += cache;
                }
                if let Some(cost) = cost_usd {
                    self.cost += cost;
                } else {
                    self.unpriced += 1;
                }
                if self.quiet {
                    return;
                }
                if self.block != Block::None {
                    self.end_blocks();
                }
                let mut parts = Vec::new();
                if let Some(input) = usage.input_tokens {
                    parts.push(self.style.dim(format!("{} in", human_tokens(input))));
                }
                if let Some(output) = usage.output_tokens {
                    parts.push(self.style.dim(format!("{} out", human_tokens(output))));
                }
                if let Some(rate) = tokens_per_second {
                    parts.push(self.style.dim(format!("{rate:.1} tok/s")));
                }
                if *duration_ms > 0 {
                    parts.push(
                        self.style
                            .dim(format!("took {}", duration_label(*duration_ms))),
                    );
                }
                if let Some(first) = first_token_ms {
                    parts.push(
                        self.style
                            .dim(format!("latency {}", duration_label(*first))),
                    );
                }
                if let (Some(cache), Some(input)) = (usage.cache_read_tokens, usage.input_tokens) {
                    parts.push(
                        self.style
                            .dim(format!("cache {}", percent_label(cache, input))),
                    );
                } else if *cache_hit {
                    parts.push(self.style.dim("cache hit".to_owned()));
                }
                if let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) {
                    parts.push(
                        self.style
                            .dim(format!("in:out {}", token_ratio(input, output))),
                    );
                }
                if !tools.is_empty() {
                    parts.push(self.style.dim(format!("tools {}", Self::tool_list(tools))));
                }
                if self.show_cost {
                    parts.push(match cost_usd {
                        Some(cost) if *cost > 0.0 => self.style.green(format!("${cost:.6}")),
                        Some(cost) => self.style.faint(format!("${cost:.6}")),
                        None => self.style.yellow("cost not reported".to_owned()),
                    });
                }
                eprintln!(
                    "  {} {}",
                    self.style.blue("↳"),
                    parts.join(&self.style.faint(" · "))
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
    #[allow(clippy::too_many_lines)]
    fn status_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::CompactionStarted { reason } => {
                if !self.quiet {
                    self.end_blocks();
                    eprintln!(
                        "{} {}",
                        self.style.dim("compacting context…"),
                        self.style.faint(format!("({reason})"))
                    );
                }
            }
            ServerEventKind::CompactionFinished {
                compacted,
                north_session,
                error,
            } => {
                self.end_blocks();
                if *compacted {
                    let detail = if self.quiet {
                        String::new()
                    } else {
                        north_session
                            .as_ref()
                            .map_or_else(String::new, |id| format!(" · new session {}", short(id)))
                    };
                    eprintln!(
                        "{}{}",
                        self.style.green("✓ context compacted"),
                        self.style.dim(detail)
                    );
                } else {
                    let detail = error
                        .as_ref()
                        .map_or_else(String::new, |reason| format!(": {reason}"));
                    eprintln!(
                        "{}{}",
                        self.style
                            .yellow("compaction did not happen; history is unchanged"),
                        self.style.dim(detail)
                    );
                }
            }
            ServerEventKind::TurnStarted { .. } => {
                self.cancelled = false;
                self.reasoning_placeholder = false;
            }
            ServerEventKind::Error { code, message } => {
                self.end_blocks();
                eprintln!(
                    "{} {} {}",
                    self.style.red("error:"),
                    self.style.bold(message),
                    self.style.dim(format!("({})", code_name(*code)))
                );
                if !self.cancelled {
                    self.failed = true;
                }
            }
            ServerEventKind::ApprovalResolved { approved, .. } => {
                if !self.quiet {
                    self.end_blocks();
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
                self.end_blocks();
                if !*committed && self.cancelled {
                    if !self.quiet {
                        eprintln!(
                            "{}",
                            self.style
                                .yellow("turn cancelled · the session is saved and can be resumed")
                        );
                    }
                } else if !*committed && self.budget_stopped {
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
            Delta::Text { text, .. } => self.answer_delta(text),
            Delta::Reasoning { text, .. } => {
                if self.quiet {
                    if !self.reasoning_placeholder {
                        self.reasoning_placeholder = true;
                        eprintln!("  {}", self.style.dim("thinking…"));
                    }
                } else {
                    if self.block != Block::Reasoning {
                        if self.block == Block::Answer {
                            self.end_blocks();
                        } else if self.block == Block::None {
                            self.space();
                        }
                        eprintln!("  {} {}", self.style.faint("┌"), self.style.dim("thinking"));
                        self.block = Block::Reasoning;
                        self.reasoning_line_start = true;
                    }
                    self.reasoning_delta(text);
                }
            }
            Delta::Refusal { text, .. } => {
                self.end_blocks();
                eprintln!("{}", self.style.red(format!("refusal: {text}")));
                self.failed = true;
            }
            Delta::ToolArguments { .. } | Delta::Media { .. } => {}
        }
    }

    /// Formats the tools of one request, with counts, in first-seen order.
    fn tool_list(tools: &[String]) -> String {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for tool in tools {
            if let Some((_, count)) = counts.iter_mut().find(|(name, _)| *name == tool) {
                *count += 1;
            } else {
                counts.push((tool, 1));
            }
        }
        counts
            .into_iter()
            .map(|(tool, count)| format!("{tool} ×{count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Formats how many times each tool ran, in name order.
    fn tool_summary(&self) -> String {
        self.tool_counts
            .iter()
            .map(|(tool, count)| format!("{} ×{count}", self.style.tool(tool.clone())))
            .collect::<Vec<_>>()
            .join(&self.style.faint(" · "))
    }

    /// Writes the run's cost and tool summary, if the run made any requests.
    fn summary(&self) {
        if matches!(self.format, Format::Jsonl) || self.quiet || self.requests == 0 {
            return;
        }
        let mut parts = vec![
            self.style.cyan(format!("{} request(s)", self.requests)),
            self.style.dim(format!("{} in", human_tokens(self.input))),
            self.style.dim(format!("{} out", human_tokens(self.output))),
        ];
        if self.cache_reported && self.input > 0 {
            parts.push(self.style.blue(format!(
                "cache {}",
                percent_label(self.cache_read, self.input)
            )));
        }
        parts.push(
            self.style
                .dim(format!("in:out {}", token_ratio(self.input, self.output))),
        );
        if self.show_cost {
            if self.unpriced == self.requests {
                parts.push(self.style.yellow("cost not reported"));
            } else {
                parts.push(if self.cost > 0.0 {
                    self.style.green(format!("${:.6}", self.cost))
                } else {
                    self.style.faint(format!("${:.6}", self.cost))
                });
                if self.unpriced > 0 {
                    parts.push(self.style.yellow(format!("{} not reported", self.unpriced)));
                }
            }
        }
        eprintln!();
        eprintln!(
            "  {} {}",
            self.style.faint("──"),
            parts.join(&self.style.faint(" · "))
        );
        if !self.tool_counts.is_empty() {
            eprintln!("  {} {}", self.style.faint("tools"), self.tool_summary());
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

/// A share of a total, rounded so `0.4%` does not become a bare `0%`.
#[allow(clippy::cast_precision_loss)]
fn percent_label(part: u64, total: u64) -> String {
    if total == 0 {
        return "0%".to_owned();
    }
    let percent = part as f64 / total as f64 * 100.0;
    if percent >= 10.0 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

/// The input-to-output ratio, the number a cost conversation usually asks
/// about. A turn with no output is unbounded rather than zero.
#[allow(clippy::cast_precision_loss)]
fn token_ratio(input: u64, output: u64) -> String {
    if input == 0 && output == 0 {
        return "–".to_owned();
    }
    if output == 0 {
        return "∞".to_owned();
    }
    let ratio = input as f64 / output as f64;
    if ratio >= 10.0 {
        format!("{ratio:.0}:1")
    } else {
        format!("{ratio:.1}:1")
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

/// Prints the startup facts that make a run or resume reproducible.
#[allow(clippy::too_many_arguments)]
fn debug_start(
    style: Style,
    spec: &ModelSpec,
    configuration: &Config,
    source: Source,
    database: &Path,
    config_path: Option<&Path>,
    session: Option<&str>,
) {
    let workspace =
        std::env::current_dir().map_or_else(|_| "?".to_owned(), |path| path.display().to_string());
    let config_label =
        config_path.map_or_else(|| "(default)".to_owned(), |path| path.display().to_string());
    let iterations = configuration
        .max_iterations
        .map_or_else(|| "unlimited".to_owned(), |limit| limit.to_string());
    eprintln!("{}", style.dim("debug: startup"));
    eprintln!(
        "{}",
        style.dim(format!(
            "  session    {}",
            session.unwrap_or("(new; shown on first event)")
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  model      {} · provider {} · wire {}",
            spec.label(),
            spec.provider,
            spec.wire.family()
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  approval   {}",
            approval_label(configuration.approval_mode)
        ))
    );
    eprintln!("{}", style.dim(format!("  iterations {iterations}")));
    eprintln!(
        "{}",
        style.dim(format!(
            "  compaction {}{}",
            configuration.compaction.threshold,
            if !configuration.compaction.enabled {
                " (disabled)"
            } else if configuration.compaction.trigger == "eager" {
                " (eager)"
            } else {
                " (idle)"
            }
        ))
    );
    eprintln!("{}", style.dim(format!("  workspace  {workspace}")));
    eprintln!("{}", style.dim(format!("  config     {config_label}")));
    eprintln!(
        "{}",
        style.dim(format!("  database   {}", database.display()))
    );
    eprintln!(
        "{}",
        style.dim(format!("  catalogue  {}", source_label(source)))
    );
    if !configuration.extensions.is_empty() {
        eprintln!(
            "{}",
            style.dim(format!(
                "  extensions {}",
                configuration
                    .extensions
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        );
    }
}

/// Prints the static prompt size in characters. Tokens are provider-reported,
/// so no estimate is made here.
fn debug_prompt_size(style: Style, service: &Service, source: &str) {
    match service.prompt_size() {
        Ok(size) => {
            eprintln!("{}", style.dim("debug: prompt material"));
            eprintln!("{}", style.dim(format!("  source     {source}")));
            eprintln!(
                "{}",
                style.dim(format!("  system     {} chars", size.system_chars))
            );
            eprintln!(
                "{}",
                style.dim(format!("  tools      {} chars", size.tool_chars))
            );
            eprintln!(
                "{}",
                style.dim(format!("  total      {} chars", size.total_chars()))
            );
        }
        Err(error) => eprintln!(
            "{}",
            style.yellow(format!("debug: could not measure the prompt: {error}"))
        ),
    }
}

/// Prints the selected model's capabilities and catalogue metadata.
#[allow(clippy::too_many_lines)]
fn debug_model(style: Style, spec: &ModelSpec, model: Option<&norte_code_catalog::Model>) {
    let capabilities = &spec.capabilities;
    eprintln!("{}", style.dim("debug: model"));
    eprintln!("{}", style.dim(format!("  reference  {}", spec.label())));
    if let Some(model) = model {
        if !model.name.is_empty() {
            eprintln!("{}", style.dim(format!("  name       {}", model.name)));
        }
        if let Some(family) = &model.family {
            eprintln!("{}", style.dim(format!("  family     {family}")));
        }
        if let Some(description) = &model.description {
            eprintln!("{}", style.dim(format!("  about      {description}")));
        }
    }
    eprintln!(
        "{}",
        style.dim(format!(
            "  wire       {} · provider {}",
            spec.wire.family(),
            spec.provider
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  context    {}",
            token_limit_label(capabilities.max_context)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  output     {}",
            token_limit_label(capabilities.max_output)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  modalities input {} · output {}",
            modality_set(capabilities.modalities.input.keys()),
            modality_set(capabilities.modalities.output.iter())
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  tools      {}",
            tool_kind_set(&capabilities.tool_kinds)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  reasoning  {}",
            reasoning_capabilities(&capabilities.reasoning)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  streaming  {}",
            capabilities
                .streaming
                .map_or_else(|| "-".to_owned(), |value| streaming_name(value).to_owned())
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!("  cache      {}", cache_name(capabilities.cache)))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  features   {}",
            feature_set(&capabilities.features)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  structured {}",
            structured_set(&capabilities.structured_output)
        ))
    );
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
            eprintln!(
                "{}",
                style.dim(format!("  catalogue  {}", metadata.join(" · ")))
            );
        }
    }
}

/// Prints the reasoning setting the session is actually using.
fn debug_reasoning(style: Style, reasoning: &Reasoning) {
    eprintln!("{}", style.dim("debug: reasoning"));
    eprintln!(
        "{}",
        style.dim(format!(
            "  effective  {}",
            reasoning_setting_label(reasoning)
        ))
    );
}

fn reasoning_setting_label(reasoning: &Reasoning) -> String {
    match reasoning {
        Reasoning::Automatic => "auto".to_owned(),
        Reasoning::Disabled => "off".to_owned(),
        Reasoning::Effort { effort } => effort.clone(),
        Reasoning::Budget { tokens } => format!("budget {tokens} tokens"),
    }
}

/// Formats a token limit without guessing an unknown value.
fn token_limit_label(limit: Option<u64>) -> String {
    limit.map_or_else(|| "unknown".to_owned(), |value| format!("{value} tokens"))
}

/// Formats a set of modalities.
fn modality_set<'a>(values: impl Iterator<Item = &'a Modality>) -> String {
    let names: Vec<_> = values.map(|value| modality_name(*value)).collect();
    if names.is_empty() {
        "-".to_owned()
    } else {
        names.join(", ")
    }
}

/// Formats the tool kinds a model accepts.
fn tool_kind_set(values: &std::collections::BTreeSet<ToolKind>) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values
        .iter()
        .map(|kind| tool_kind_name(*kind))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats the independent features a model declares.
fn feature_set(values: &std::collections::BTreeSet<Feature>) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values
        .iter()
        .map(|feature| feature_name(*feature))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats the structured-output modes a model declares.
fn structured_set(values: &std::collections::BTreeSet<StructuredOutputMode>) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values
        .iter()
        .map(|mode| structured_name(*mode))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats a model's reasoning controls.
fn reasoning_capabilities(reasoning: &norte::Reasoning) -> String {
    if !reasoning.supported {
        return "not supported".to_owned();
    }
    let mut parts = Vec::new();
    if reasoning.can_disable {
        parts.push("off allowed".to_owned());
    }
    if reasoning.efforts.is_empty() {
        parts.push("auto only".to_owned());
    } else {
        parts.push(format!(
            "efforts {}",
            reasoning
                .efforts
                .iter()
                .map(|effort| effort_label(*effort))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if reasoning.supports_budget {
        parts.push(reasoning.max_budget.map_or_else(
            || "budget yes".to_owned(),
            |max| format!("budget up to {max}"),
        ));
    }
    parts.join(" · ")
}

/// Lower-case labels for the capability enums. They serialize as snake case;
/// these keep the debug output readable without a serialization dependency.
fn modality_name(modality: Modality) -> &'static str {
    match modality {
        Modality::Text => "text",
        Modality::Image => "image",
        Modality::Audio => "audio",
        Modality::Video => "video",
        Modality::Document => "document",
        _ => "other",
    }
}

fn tool_kind_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Function => "function",
        ToolKind::ProviderExecuted => "provider-executed",
        ToolKind::ProviderSchema => "provider-schema",
        _ => "other",
    }
}

fn feature_name(feature: Feature) -> &'static str {
    match feature {
        Feature::Citations => "citations",
        Feature::ParallelToolCalls => "parallel-tool-calls",
        Feature::ForcedToolChoice => "forced-tool-choice",
        Feature::Batch => "batch",
        Feature::Logprobs => "logprobs",
        _ => "other",
    }
}

fn structured_name(mode: StructuredOutputMode) -> &'static str {
    match mode {
        StructuredOutputMode::JsonObject => "json-object",
        StructuredOutputMode::JsonSchema => "json-schema",
        StructuredOutputMode::StrictSchema => "strict-schema",
        _ => "other",
    }
}

fn streaming_name(mode: StreamingGranularity) -> &'static str {
    match mode {
        StreamingGranularity::Delta => "delta",
        StreamingGranularity::Part => "part",
        StreamingGranularity::Disabled => "disabled",
        _ => "other",
    }
}

fn cache_name(mode: CacheMode) -> &'static str {
    match mode {
        CacheMode::None => "none",
        CacheMode::Automatic => "automatic",
        CacheMode::Explicit => "explicit",
        _ => "other",
    }
}

fn effort_label(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        _ => "other",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
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

fn parse_budget_unit(unit: &str) -> Result<BudgetUnit, String> {
    match unit {
        "usd" => Ok(BudgetUnit::Usd),
        "tokens" => Ok(BudgetUnit::Tokens),
        "requests" => Ok(BudgetUnit::Requests),
        other => Err(format!("unknown budget unit `{other}`")),
    }
}

fn parse_budget_action(action: &str) -> Result<BudgetAction, String> {
    match action {
        "warn" => Ok(BudgetAction::Warn),
        "ask" => Ok(BudgetAction::Ask),
        "stop" => Ok(BudgetAction::Stop),
        other => Err(format!("unknown budget action `{other}`")),
    }
}

/// Selects the system prompt: flag, then config, then the built-in default.
fn select_system_prompt(
    flag: Option<String>,
    configured: Option<String>,
) -> (&'static str, String) {
    if let Some(prompt) = flag {
        ("flag", prompt)
    } else if let Some(prompt) = configured {
        ("config", prompt)
    } else {
        ("default", DEFAULT_SYSTEM_PROMPT.to_owned())
    }
}

/// Parses `--reasoning`. The service still checks the setting against the
/// selected model, so this only catches names no model could accept.
fn parse_reasoning_setting(value: &str) -> Result<Reasoning, String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "auto" | "automatic" => Ok(Reasoning::Automatic),
        "off" | "none" | "disabled" => Ok(Reasoning::Disabled),
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
            let effort = match normalized.as_str() {
                "minimal" => "minimal",
                "low" => "low",
                "medium" => "medium",
                _ => "high",
            };
            Ok(Reasoning::Effort {
                effort: effort.to_owned(),
            })
        }
        _ => Err(format!(
            "`{value}` is not a reasoning setting; use auto, off, minimal, low, medium or high"
        )),
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
        ApprovalChoice, Cli, Command, DenyScope, GrantScope, PreviewLine, apply_extensions,
        approval_request, argument_preview, budget_label, compose_selection, cost_summary,
        parse_approval_choice, parse_reasoning_setting, parse_selection, select_system_prompt,
    };
    use clap::Parser;
    use norte_code_cost::CostSummary;
    use norte_code_protocol::{Budget, BudgetAction, BudgetUnit, ClientRequestKind, SessionKey};

    #[test]
    fn the_system_prompt_prefers_the_flag_then_the_config() {
        assert_eq!(
            select_system_prompt(Some("flag".to_owned()), Some("config".to_owned())),
            ("flag", "flag".to_owned())
        );
        assert_eq!(
            select_system_prompt(None, Some("config".to_owned())),
            ("config", "config".to_owned())
        );
        let (source, prompt) = select_system_prompt(None, None);
        assert_eq!(source, "default");
        assert!(prompt.contains("Norte Code"));
    }

    #[test]
    fn reasoning_names_are_parsed_and_unknown_ones_are_rejected() {
        assert_eq!(
            parse_reasoning_setting("off").expect("off"),
            norte_code_protocol::Reasoning::Disabled
        );
        assert_eq!(
            parse_reasoning_setting("xhigh").expect("alias"),
            norte_code_protocol::Reasoning::Effort {
                effort: "high".to_owned()
            }
        );
        assert!(parse_reasoning_setting("x").is_err());
    }

    #[test]
    fn vendor_extensions_are_parsed_into_the_config() {
        let mut config = norte_code_config::Config::default();
        apply_extensions(
            &mut config,
            &[
                r#"openai.chat:provider.order=["deepinfra"]"#.to_owned(),
                "openai.chat:temperature=0.2".to_owned(),
                "openai.chat:metadata=review".to_owned(),
            ],
        )
        .expect("extensions");
        assert_eq!(
            config.extensions["openai.chat:provider.order"],
            serde_json::json!(["deepinfra"])
        );
        assert_eq!(
            config.extensions["openai.chat:temperature"],
            serde_json::json!(0.2)
        );
        assert_eq!(
            config.extensions["openai.chat:metadata"],
            serde_json::json!("review")
        );
        assert!(apply_extensions(&mut config, &["provider".to_owned()]).is_err());
        assert!(apply_extensions(&mut config, &["openai.chat:provider".to_owned()]).is_err());
    }

    #[test]
    fn closed_question_selections_are_parsed() {
        let options = ["Read".to_owned(), "Write".to_owned()];
        assert_eq!(
            parse_selection("1", &options, false),
            Some((vec![0], false))
        );
        assert_eq!(
            parse_selection("write", &options, false),
            Some((vec![1], false))
        );
        assert_eq!(
            parse_selection("3", &options, false),
            Some((Vec::new(), true))
        );
        assert_eq!(
            parse_selection("1, 2", &options, true),
            Some((vec![0, 1], false))
        );
        assert_eq!(
            parse_selection("1, 3", &options, true),
            Some((vec![0], true))
        );
        assert_eq!(
            parse_selection("all", &options, true),
            Some((vec![0, 1], false))
        );
        assert_eq!(parse_selection("1, 2", &options, false), None);
        assert_eq!(parse_selection("0", &options, false), None);
        assert_eq!(
            compose_selection(&options, &[0, 1], Some("something custom")),
            "Read, Write, something custom"
        );
    }

    #[test]
    fn approval_answers_are_parsed_leniently() {
        assert_eq!(parse_approval_choice("y"), Some(ApprovalChoice::Once));
        assert_eq!(parse_approval_choice(" YES "), Some(ApprovalChoice::Once));
        assert_eq!(
            parse_approval_choice("s"),
            Some(ApprovalChoice::ToolSession)
        );
        assert_eq!(parse_approval_choice("A"), Some(ApprovalChoice::ToolGlobal));
        assert_eq!(
            parse_approval_choice("always"),
            Some(ApprovalChoice::ToolGlobal)
        );
        assert_eq!(
            parse_approval_choice("n"),
            Some(ApprovalChoice::DenyOnce(None))
        );
        assert_eq!(
            parse_approval_choice(""),
            Some(ApprovalChoice::DenyOnce(None))
        );
        assert_eq!(
            parse_approval_choice("ns"),
            Some(ApprovalChoice::DenySession(None))
        );
        assert_eq!(
            parse_approval_choice("ng"),
            Some(ApprovalChoice::DenyGlobal(None))
        );
        assert_eq!(parse_approval_choice("maybe"), None);
    }

    #[test]
    fn a_denial_can_carry_feedback_for_the_model() {
        assert_eq!(
            parse_approval_choice("n use the existing helper"),
            Some(ApprovalChoice::DenyOnce(Some(
                "use the existing helper".to_owned()
            )))
        );
        assert_eq!(
            parse_approval_choice("no this touches production"),
            Some(ApprovalChoice::DenyOnce(Some(
                "this touches production".to_owned()
            )))
        );
        assert_eq!(
            parse_approval_choice("ns use the helper"),
            Some(ApprovalChoice::DenySession(Some(
                "use the helper".to_owned()
            )))
        );
        assert_eq!(
            parse_approval_choice("ng not in this repo"),
            Some(ApprovalChoice::DenyGlobal(Some(
                "not in this repo".to_owned()
            )))
        );
        assert_eq!(parse_approval_choice("f"), None);
    }

    #[test]
    fn a_read_preview_names_the_path_instead_of_dumping_json() {
        let arguments = serde_json::json!({"path": "src/main.rs"});
        assert_eq!(
            argument_preview("read", &arguments),
            vec![PreviewLine::Field {
                key: "path".to_owned(),
                value: "src/main.rs".to_owned(),
            }]
        );
    }

    #[test]
    fn a_command_preview_shows_the_script_and_its_shell() {
        let arguments = serde_json::json!({
            "command": {"kind": "shell", "shell": "sh", "script": "cargo test\ncargo clippy"},
            "cwd": "/tmp/project",
        });
        let lines = argument_preview("command", &arguments);
        assert!(lines.contains(&PreviewLine::Field {
            key: "shell".to_owned(),
            value: "sh".to_owned(),
        }));
        assert!(lines.contains(&PreviewLine::Field {
            key: "cwd".to_owned(),
            value: "/tmp/project".to_owned(),
        }));
        assert!(lines.contains(&PreviewLine::Block {
            label: "script".to_owned(),
            lines: vec!["cargo test".to_owned(), "cargo clippy".to_owned()],
        }));
    }

    #[test]
    fn write_content_is_bounded() {
        let content = (0..20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let arguments = serde_json::json!({"path": "src/main.rs", "content": content});
        let lines = argument_preview("write", &arguments);
        let block = lines
            .iter()
            .find_map(|line| match line {
                PreviewLine::Block { label, lines } if label == "content" => Some(lines),
                _ => None,
            })
            .expect("content block");
        assert_eq!(block.len(), 13);
        assert!(block.last().is_some_and(|line| line.contains("more line")));
    }

    #[test]
    fn cost_totals_show_cache_and_the_input_output_ratio() {
        let mixed = CostSummary {
            requests: 2,
            input_tokens: 1_000,
            output_tokens: 100,
            cache_read_tokens: 900,
            reported_cost_usd: 0.5,
            not_reported: 1,
            ..CostSummary::default()
        };
        assert_eq!(
            cost_summary(&mixed),
            "2 request(s) · 1k in · 100 out · cache 90% · in:out 10:1 · $0.500000 · 1 not reported"
        );
        let unpriced = CostSummary {
            requests: 2,
            not_reported: 2,
            ..CostSummary::default()
        };
        assert_eq!(
            cost_summary(&unpriced),
            "2 request(s) · 0 in · 0 out · in:out – · cost not reported"
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
    fn an_interactive_run_can_start_without_a_prompt() {
        let cli = Cli::try_parse_from(["norte-code", "run", "--interactive"])
            .expect("interactive run should parse");
        match cli.command {
            Command::Run(args) => {
                assert!(args.interactive);
                assert!(args.prompt.is_none());
            }
            _ => panic!("expected run"),
        }
    }

    #[test]
    fn a_headless_run_still_requires_a_prompt() {
        assert!(Cli::try_parse_from(["norte-code", "run"]).is_err());
    }

    #[test]
    fn approval_scopes_are_explicit_in_the_request() {
        let session = SessionKey::new("s");
        let session_allow = approval_request(
            session.clone(),
            "approval".to_owned(),
            ApprovalChoice::ToolSession,
        );
        match session_allow.kind {
            ClientRequestKind::Approve {
                scope, remember, ..
            } => {
                assert!(matches!(scope, Some(GrantScope::Tool)));
                assert_eq!(remember, Some(true));
            }
            other => panic!("expected an approval, got {other:?}"),
        }
        let global_allow = approval_request(
            session.clone(),
            "approval".to_owned(),
            ApprovalChoice::ToolGlobal,
        );
        match global_allow.kind {
            ClientRequestKind::Approve { scope, .. } => {
                assert!(matches!(scope, Some(GrantScope::Always)));
            }
            other => panic!("expected an approval, got {other:?}"),
        }
        let deny = approval_request(
            session,
            "approval".to_owned(),
            ApprovalChoice::DenySession(Some("use the helper".to_owned())),
        );
        match deny.kind {
            ClientRequestKind::Deny { reason, scope, .. } => {
                assert_eq!(reason.as_deref(), Some("use the helper"));
                assert_eq!(scope, Some(DenyScope::ToolSession));
            }
            other => panic!("expected a denial, got {other:?}"),
        }
    }
}
