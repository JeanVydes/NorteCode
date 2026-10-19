//! The headless `norte-code` command.
//!
//! `norte-code run "prompt"` builds the service in memory, sends one message
//! and streams the answer. Text goes to stdout; diagnostics, reasoning and
//! tool activity go to stderr. `--jsonl` writes one protocol event per line on
//! stdout instead, for scripts and tests.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use futures_util::StreamExt;
use norte_code_agent::{DEFAULT_SYSTEM_PROMPT, ModelSpec, Service, VendorModels};
use norte_code_config::{Config, user_config_path};
use norte_code_protocol::{
    ClientRequest, ClientRequestKind, Delta, ErrorCode, ServerEvent, ServerEventKind,
};

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
    /// What to do.
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Send one prompt and stream the answer.
    Run(RunArgs),
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
    /// Write one JSON event per line on stdout.
    #[arg(long)]
    jsonl: bool,
    /// Do not print reasoning, tool activity or usage.
    #[arg(long)]
    quiet: bool,
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
    let Command::Run(args) = cli.command;
    let user = cli.config.or_else(user_config_path);
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let project = find_project_config(&cwd);
    let loaded = Config::load(user.as_deref(), project.as_deref()).map_err(|e| e.to_string())?;
    for warning in &loaded.warnings {
        eprintln!("warning: unknown config key `{warning}`");
    }
    let config = loaded.config;

    let reference = args
        .model
        .or_else(|| config.default_model.clone())
        .ok_or_else(|| {
            "no model selected: pass `--model provider/model` or set `default_model` in the config"
                .to_owned()
        })?;
    let (provider, _) = reference
        .split_once('/')
        .ok_or_else(|| format!("`{reference}` is not a `provider/model` reference"))?;
    // The catalogue supplies these exactly; this is the pre-catalogue fallback.
    let env = fallback_env(provider);
    let spec =
        ModelSpec::resolve(&config, &reference, args.endpoint, &env).map_err(|e| e.to_string())?;

    let service = Arc::new(
        Service::in_memory(config, &spec, &VendorModels, DEFAULT_SYSTEM_PROMPT)
            .map_err(|e| e.to_string())?,
    );

    let request = ClientRequest {
        request_id: 1,
        kind: ClientRequestKind::SendMessage {
            session: None,
            text: args.prompt,
            budget: None,
            reasoning: None,
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
        last_char: None,
        failed: false,
    };
    while let Some(event) = stream.next().await {
        renderer.event(&event)?;
    }
    if matches!(renderer.format, Format::Text)
        && renderer.last_char.is_some_and(|last| last != '\n')
    {
        println!();
    }
    Ok(u8::from(renderer.failed))
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
                duration_ms,
                ..
            } => {
                if !self.quiet {
                    let outcome = if *is_error { "failed" } else { "ok" };
                    eprintln!("· {tool} {outcome} in {duration_ms} ms");
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
/// The catalogue's `env` list replaces this in milestone 2; until then, derive
/// `PROVIDER_API_KEY` and add Google's two aliases.
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
