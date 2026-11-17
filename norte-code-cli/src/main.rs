//! The single entry point for Norte Code.
//!
//! `norte` runs the service if it is not already running and then opens the
//! terminal interface. `norte service …` manages the background server, and
//! `norte tui` only runs the terminal interface. Everything else is a thin
//! wrapper over the host's HTTP API.

#![forbid(unsafe_code)]
#![allow(clippy::all, clippy::pedantic)]

mod mcps;
mod output;
mod plugins;
mod providers;
mod service;
mod tui;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "norte",
    version,
    about = "The single entry point for Norte Code"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run only the terminal interface.
    Tui,
    /// Manage the background service.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Start the service detached.
    Up,
    /// Stop the running service.
    Down,
    /// Restart the service.
    Restart,
    /// Print service status.
    Status,
    /// Print the catalogue.
    Catalog,
    /// Print the service configuration.
    Config,
    /// Print the service log.
    Logs {
        /// Follow new lines.
        #[arg(short = 'f', long = "follow", alias = "live")]
        follow: bool,
    },
    /// Check paths, files and the service.
    Doctor,
    /// Manage plugins.
    Plugins {
        #[command(subcommand)]
        command: Option<plugins::Command>,
    },
    /// Manage providers.
    Providers {
        #[command(subcommand)]
        command: Option<providers::Command>,
    },
    /// Manage MCP servers.
    Mcps {
        #[command(subcommand)]
        command: Option<mcps::Command>,
    },
    /// Run the RAEP server in this process (internal).
    #[command(hide = true)]
    Serve,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e)
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                || e.kind() == clap::error::ErrorKind::DisplayVersion =>
        {
            print!("{e}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{} {}", output::red("✗"), e);
            return ExitCode::FAILURE;
        }
    };

    match cli.command {
        None => {
            if let Err(error) = service::ensure_up() {
                eprintln!("{} {}", output::red("✗"), error);
                return ExitCode::FAILURE;
            }
            if let Err(error) = tui::run() {
                eprintln!("{} {}", output::red("✗"), error);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Some(Command::Tui) => match tui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{} {}", output::red("✗"), error);
                ExitCode::FAILURE
            }
        },
        Some(Command::Service { command }) => match command {
            ServiceCommand::Up => match service::up() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{} {}", output::red("✗"), error);
                    ExitCode::FAILURE
                }
            },
            ServiceCommand::Down => match service::down() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{} {}", output::red("✗"), error);
                    ExitCode::FAILURE
                }
            },
            ServiceCommand::Restart => {
                service::down().ok();
                match service::up() {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("{} {}", output::red("✗"), error);
                        ExitCode::FAILURE
                    }
                }
            }
            ServiceCommand::Status => {
                service::status();
                ExitCode::SUCCESS
            }
            ServiceCommand::Catalog => match service::catalog() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{} {}", output::red("✗"), error);
                    ExitCode::FAILURE
                }
            },
            ServiceCommand::Config => match service::config() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{} {}", output::red("✗"), error);
                    ExitCode::FAILURE
                }
            },
            ServiceCommand::Logs { follow } => {
                let args: Vec<String> = if follow {
                    vec!["--follow".to_owned()]
                } else {
                    vec![]
                };
                match service::logs(&args) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("{} {}", output::red("✗"), error);
                        ExitCode::FAILURE
                    }
                }
            }
            ServiceCommand::Doctor => match service::doctor() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{} {}", output::red("✗"), error);
                    ExitCode::FAILURE
                }
            },
            ServiceCommand::Plugins { command } => match plugins::run(command) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{} {}", output::red("✗"), error);
                    ExitCode::FAILURE
                }
            },
            ServiceCommand::Providers { command } => match providers::run(command) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{} {}", output::red("✗"), error);
                    ExitCode::FAILURE
                }
            },
            ServiceCommand::Mcps { command } => match mcps::run(command) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{} {}", output::red("✗"), error);
                    ExitCode::FAILURE
                }
            },
            ServiceCommand::Serve => match service::serve() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{} {}", output::red("✗"), error);
                    ExitCode::FAILURE
                }
            },
        },
    }
}
