#![allow(clippy::all, clippy::pedantic)]

//! Plugin management subcommands: `north service plugins …`.

use clap::{Parser, Subcommand};
use clap::error::ErrorKind;

use norte_code_raep::plugin_manager;

#[derive(Parser)]
#[command(name = "plugins", about = "Manage installed plugins")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show installed plugins.
    List,
    /// Install a plugin from a git repository.
    Add {
        /// Repository URL.
        url: String,
        /// Install a single workspace package.
        #[arg(long)]
        package: Option<String>,
        /// Install every plugin package in the workspace.
        #[arg(long)]
        all: bool,
    },
    /// Remove an installed plugin.
    Remove {
        /// Plugin name.
        name: String,
    },
    /// Disable a plugin.
    Disable {
        /// Plugin name.
        name: String,
    },
    /// Enable a plugin.
    Enable {
        /// Plugin name.
        name: String,
    },
}

pub fn dispatch(rest: &[String]) -> Result<(), String> {
    let cli = match Cli::try_parse_from(std::iter::once("plugins").chain(rest.iter().map(String::as_str))) {
        Ok(cli) => cli,
        Err(e) if e.kind() == ErrorKind::DisplayHelp || e.kind() == ErrorKind::DisplayVersion => {
            print!("{}", e);
            return Ok(());
        }
        Err(e) => return Err(e.to_string()),
    };
    match cli.command.unwrap_or(Command::List) {
        Command::List => list(),
        Command::Add { url, package, all: _ } => add(&url, package.as_deref()),
        Command::Remove { name } => remove(&name),
        Command::Disable { name } => set_enabled(&name, false),
        Command::Enable { name } => set_enabled(&name, true),
    }
}

fn list() -> Result<(), String> {
    let info = plugin_manager::plugin_info();
    let sources = plugin_manager::recorded_sources();
    let dir = plugin_manager::plugin_dir();

    println!(
        "  {}  {}",
        crate::output::bold("plugins"),
        crate::output::muted(&dir.display().to_string())
    );
    if info.is_empty() {
        println!("  {} no plugins installed", crate::output::muted("○"));
        return Ok(());
    }
    for plugin in &info {
        let marker = if plugin.enabled {
            crate::output::ok("●")
        } else {
            crate::output::muted("○")
        };
        let state = if plugin.enabled {
            crate::output::ok("enabled")
        } else {
            crate::output::muted("disabled")
        };
        println!(
            "  {}  {}  {}",
            marker,
            crate::output::accent(&plugin.name),
            state
        );
        if let Some((url, package)) = sources.get(&plugin.name) {
            let extra = match package {
                Some(package) => format!("{url}  (package {package})"),
                None => url.clone(),
            };
            println!("       {}", crate::output::muted(&extra));
        }
    }
    Ok(())
}

fn add(url: &str, package: Option<&str>) -> Result<(), String> {
    let progress = |index: usize, total: usize, label: &str| {
        println!("  [{} / {}] {}", index, total, crate::output::muted(label));
    };

    let stems = if let Some(package) = package {
        vec![plugin_manager::install_plugin(url, Some(package), &progress)?]
    } else {
        plugin_manager::install_all_plugins(url, &progress)?
    };

    println!(
        "  {} installed: {}",
        crate::output::ok("✓"),
        stems.join(", ")
    );
    println!(
        "  {} restart the service to load them: `norte service restart`",
        crate::output::muted("→")
    );
    Ok(())
}

fn remove(name: &str) -> Result<(), String> {
    plugin_manager::remove_plugin(name)?;
    println!(
        "  {} removed {}",
        crate::output::ok("✓"),
        crate::output::accent(name)
    );
    println!(
        "  {} restart the service: `norte service restart`",
        crate::output::muted("→")
    );
    Ok(())
}

fn set_enabled(name: &str, enabled: bool) -> Result<(), String> {
    plugin_manager::set_plugin_enabled(name, enabled)?;
    let verb = if enabled { "enabled" } else { "disabled" };
    println!(
        "  {} {} {}",
        crate::output::ok("✓"),
        verb,
        crate::output::accent(name)
    );
    println!(
        "  {} restart the service: `norte service restart`",
        crate::output::muted("→")
    );
    Ok(())
}