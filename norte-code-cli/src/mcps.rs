#![allow(clippy::all, clippy::pedantic)]

//! MCP server subcommands: `north service mcps …`.

use std::collections::BTreeMap;
use std::fs;

use clap::{Parser, Subcommand};
use clap::error::ErrorKind;

use norte_code_protocol::config::{ensure_user_service_config, user_config_path};

#[derive(Parser)]
#[command(name = "mcps", about = "Edit MCP servers in service.json")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show configured MCP servers.
    List {
        /// Filter to one agent.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Add an MCP server.
    Add {
        /// Server name.
        name: String,
        /// Local command to run.
        #[arg(long)]
        command: Option<String>,
        /// Argument for the command (repeatable).
        #[arg(long = "arg")]
        args: Vec<String>,
        /// Environment variable K=V (repeatable).
        #[arg(long = "env")]
        env: Vec<String>,
        /// Remote URL.
        #[arg(long)]
        url: Option<String>,
        /// Header K=V (repeatable).
        #[arg(long = "header")]
        headers: Vec<String>,
        /// Allowed tool (repeatable).
        #[arg(long = "allow")]
        allow: Vec<String>,
        /// Denied tool (repeatable).
        #[arg(long = "deny")]
        deny: Vec<String>,
        /// Agent to add it to.
        #[arg(long)]
        agent: Option<String>,
        /// Add it disabled.
        #[arg(long)]
        disabled: bool,
    },
    /// Remove an MCP server.
    Remove {
        /// Server name.
        name: String,
        /// Agent it belongs to.
        #[arg(long)]
        agent: Option<String>,
    },
}

pub fn dispatch(rest: &[String]) -> Result<(), String> {
    let cli = match Cli::try_parse_from(std::iter::once("mcps").chain(rest.iter().map(String::as_str))) {
        Ok(cli) => cli,
        Err(e) if e.kind() == ErrorKind::DisplayHelp || e.kind() == ErrorKind::DisplayVersion => {
            print!("{}", e);
            return Ok(());
        }
        Err(e) => return Err(e.to_string()),
    };
    match cli.command.unwrap_or(Command::List { agent: None }) {
        Command::List { agent } => list(agent.as_deref()),
        Command::Add {
            name,
            command,
            args,
            env,
            url,
            headers,
            allow,
            deny,
            agent,
            disabled,
        } => add(AddArgs {
            name,
            command,
            args,
            env,
            url,
            headers,
            allow,
            deny,
            agent,
            disabled,
        }),
        Command::Remove { name, agent } => remove(&name, agent.as_deref()),
    }
}

struct AddArgs {
    name: String,
    command: Option<String>,
    args: Vec<String>,
    env: Vec<String>,
    url: Option<String>,
    headers: Vec<String>,
    allow: Vec<String>,
    deny: Vec<String>,
    agent: Option<String>,
    disabled: bool,
}

fn sane_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("__")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn default_agent_name(root: &serde_json::Value) -> Option<String> {
    if let Some(name) = root.get("default_agent").and_then(|v| v.as_str()) {
        return Some(name.to_owned());
    }
    let agents = root.get("agents")?.as_array()?;
    if agents.len() == 1 {
        return agents
            .first()
            .and_then(|a| a.get("name"))
            .and_then(|v| v.as_str())
            .map(str::to_owned);
    }
    None
}

fn find_agent_index(root: &mut serde_json::Value, agent: Option<&str>) -> Result<usize, String> {
    let target = match agent {
        Some(name) => name.to_owned(),
        None => default_agent_name(root).ok_or_else(|| {
            let names: Vec<String> = root
                .get("agents")
                .and_then(|v| v.as_array())
                .map(|agents| {
                    agents
                        .iter()
                        .filter_map(|a| a.get("name").and_then(|v| v.as_str()))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            format!(
                "could not pick an agent; name one with --agent. Available: {}",
                names.join(", ")
            )
        })?,
    };
    let agents = root
        .get_mut("agents")
        .and_then(|v| v.as_array_mut())
        .ok_or("service.json has no `agents` array".to_owned())?;
    let index = agents
        .iter()
        .position(|a| a.get("name").and_then(|v| v.as_str()) == Some(target.as_str()));
    index.ok_or_else(|| {
        let names: Vec<String> = agents
            .iter()
            .filter_map(|a| a.get("name").and_then(|v| v.as_str()))
            .map(str::to_owned)
            .collect();
        format!("agent `{target}` not found; available: {}", names.join(", "))
    })
}

fn load_service_json() -> Result<(std::path::PathBuf, serde_json::Value), String> {
    ensure_user_service_config().map_err(|e| format!("{e}"))?;
    let path = user_config_path().ok_or("could not resolve service.json path".to_owned())?;
    let raw = fs::read_to_string(&path).map_err(|e| format!("{e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("could not parse service.json: {e}"))?;
    Ok((path, value))
}

fn atomic_write(path: &std::path::Path, body: &str) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("{e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("could not replace {}: {e}", path.display()))?;
    Ok(())
}

fn parse_kv_list(items: &[String], what: &str) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    for item in items {
        let (key, value) = item
            .split_once('=')
            .ok_or_else(|| format!("{what} must be `KEY=VALUE`, got `{item}`"))?;
        if key.is_empty() {
            return Err(format!("{what} key must not be empty"));
        }
        map.insert(key.to_owned(), value.to_owned());
    }
    Ok(map)
}

fn add(args: AddArgs) -> Result<(), String> {
    if !sane_name(&args.name) {
        return Err(format!(
            "a server name may hold letters, digits, `-` and `_`, and not `__`"
        ));
    }
    let is_remote = args.url.is_some();
    let is_local = args.command.is_some();
    if is_remote && is_local {
        return Err("`command` and `url` cannot both be set".to_owned());
    }
    if !is_remote && !is_local {
        return Err("either `command` or `url` is required".to_owned());
    }

    let env = parse_kv_list(&args.env, "--env")?;
    let headers = parse_kv_list(&args.headers, "--header")?;

    let mut entry = serde_json::Map::new();
    if let Some(command) = args.command {
        entry.insert("command".to_owned(), serde_json::json!(command));
        entry.insert("args".to_owned(), serde_json::json!(args.args));
        if !env.is_empty() {
            entry.insert("env".to_owned(), serde_json::json!(env));
        }
    }
    if let Some(url) = args.url {
        entry.insert("url".to_owned(), serde_json::json!(url));
        if !headers.is_empty() {
            entry.insert("headers".to_owned(), serde_json::json!(headers));
        }
    }
    entry.insert("enabled".to_owned(), serde_json::json!(!args.disabled));
    entry.insert("allow".to_owned(), serde_json::json!(args.allow));
    entry.insert("deny".to_owned(), serde_json::json!(args.deny));

    let (path, mut root) = load_service_json()?;
    let index = find_agent_index(&mut root, args.agent.as_deref())?;
    let agent_name = root["agents"][index]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let servers = root["agents"][index]
        .as_object_mut()
        .ok_or("agent entry is not an object".to_owned())?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("mcpServers is not an object".to_owned())?;
    servers.insert(args.name.clone(), serde_json::Value::Object(entry));

    let body = serde_json::to_string_pretty(&root).map_err(|e| format!("{e}"))?;
    atomic_write(&path, &body)?;

    println!(
        "  {} added MCP server {} to agent {}",
        crate::output::ok("✓"),
        crate::output::accent(&args.name),
        crate::output::accent(&agent_name)
    );
    println!(
        "  {} restart the service to load it: `norte service restart`",
        crate::output::muted("→")
    );
    Ok(())
}

fn remove(name: &str, agent: Option<&str>) -> Result<(), String> {
    let (path, mut root) = load_service_json()?;
    let index = find_agent_index(&mut root, agent)?;
    let agent_name = root["agents"][index]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let servers = root["agents"][index]
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut());
    let Some(servers) = servers else {
        return Err(format!("agent `{agent_name}` has no MCP servers"));
    };
    if servers.remove(name).is_none() {
        return Err(format!(
            "MCP server `{name}` not found under agent `{agent_name}`"
        ));
    }
    let body = serde_json::to_string_pretty(&root).map_err(|e| format!("{e}"))?;
    atomic_write(&path, &body)?;
    println!(
        "  {} removed MCP server {} from agent {}",
        crate::output::ok("✓"),
        crate::output::accent(name),
        crate::output::accent(&agent_name)
    );
    println!(
        "  {} restart the service: `norte service restart`",
        crate::output::muted("→")
    );
    Ok(())
}

fn list(agent: Option<&str>) -> Result<(), String> {
    let (_path, root) = load_service_json()?;
    let agents = root
        .get("agents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for agent_entry in &agents {
        let agent_name = agent_entry
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("(unnamed)")
            .to_owned();
        if let Some(filter) = agent
            && filter != &agent_name
        {
            continue;
        }
        let servers = agent_entry
            .get("mcpServers")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        println!(
            "  {}  {}",
            crate::output::bold(&agent_name),
            crate::output::muted(&format!("({} servers)", servers.len()))
        );
        if servers.is_empty() {
            println!("  {}", crate::output::muted("  (none)"));
            continue;
        }
        for (name, cfg) in &servers {
            let target = if let Some(url) = cfg.get("url").and_then(|v| v.as_str()) {
                url.to_owned()
            } else {
                cfg.get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no command)")
                    .to_owned()
            };
            let enabled = cfg
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            let marker = if enabled {
                crate::output::ok("●")
            } else {
                crate::output::muted("○")
            };
            let state = if enabled {
                crate::output::ok("enabled")
            } else {
                crate::output::muted("disabled")
            };
            println!(
                "  {}  {}  {}  {}",
                marker,
                crate::output::accent(name),
                state,
                crate::output::muted(&target)
            );
        }
    }

    let url = crate::service::service_url();
    if crate::service::probe(&url) {
        let token = cli_token();
        if let Ok(client) = norte_code_client::RestClient::new(&url, &token) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            if let Ok(rt) = rt
                && let Ok(catalog) = rt.block_on(client.mcp_catalog())
            {
                println!();
                println!(
                    "  {}  {}",
                    crate::output::bold("live"),
                    crate::output::muted("(running service)")
                );
                for server in &catalog.servers {
                    let state = if server.active {
                        crate::output::ok("active")
                    } else {
                        crate::output::muted("disabled")
                    };
                    println!(
                        "  {}  {}  {}",
                        crate::output::accent(&server.name),
                        state,
                        crate::output::muted(&server.command)
                    );
                }
                if !catalog.failures.is_empty() {
                    println!("  {}", crate::output::warn("failures:"));
                    for failure in &catalog.failures {
                        println!(
                            "    {}  {}",
                            crate::output::red(&failure.name),
                            crate::output::muted(&failure.reason)
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

fn cli_token() -> String {
    let path = norte_code_protocol::config::user_cli_path();
    let Some(path) = path else {
        return String::new();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return String::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return String::new();
    };
    value
        .get("service_auth")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned()
}