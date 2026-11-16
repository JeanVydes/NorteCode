#![allow(clippy::all, clippy::pedantic)]

//! Provider catalogue subcommands: `north service providers …`.

use std::fs;

use clap::Subcommand;

use norte_code_protocol::catalog::{load_catalog_raw, save_catalog_files};
use norte_code_wire::catalog::{Provider, Wire, WireSchema};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Show providers (catalogue files and plugin-injected).
    List,
    /// Add or replace a provider.
    Add {
        /// Provider id.
        id: String,
        /// Display name.
        #[arg(long)]
        name: Option<String>,
        /// Comma-separated environment variables holding the key(s).
        #[arg(long, value_delimiter = ',')]
        keys: Vec<String>,
    },
    /// Add a wire to a provider.
    AddWire {
        /// Provider id.
        id: String,
        /// Wire schema(s): openai, anthropic, google (comma-separated).
        #[arg(long, value_delimiter = ',')]
        schema: Vec<String>,
        /// Base URL for the wire(s).
        #[arg(long)]
        base_url: Option<String>,
        /// Credential header: bearer, x_api_key, x_goog_api_key or disabled.
        #[arg(long)]
        auth: Option<String>,
    },
    /// Remove wire(s) from a provider.
    RemoveWire {
        /// Provider id.
        id: String,
        /// Wire schema(s) to remove (comma-separated).
        #[arg(long, value_delimiter = ',')]
        schema: Vec<String>,
    },
    /// Show the wires of a provider.
    GetWires {
        /// Provider id.
        id: String,
    },
    /// Remove a provider and its models.
    Remove {
        /// Provider id.
        id: String,
    },
}

pub(crate) fn run(command: Option<Command>) -> Result<(), String> {
    match command.unwrap_or(Command::List) {
        Command::List => list(),
        Command::Add { id, name, keys } => add(&id, name.as_deref(), &keys),
        Command::AddWire {
            id,
            schema,
            base_url,
            auth,
        } => add_wire(&id, &schema, base_url.as_deref(), auth.as_deref()),
        Command::RemoveWire { id, schema } => remove_wires(&id, &schema),
        Command::GetWires { id } => get_wires(&id),
        Command::Remove { id } => remove(&id),
    }
}

fn parse_auth(value: &str) -> Result<norte_code_wire::catalog::AuthKind, String> {
    match value {
        "bearer" => Ok(norte_code_wire::catalog::AuthKind::Bearer),
        "x_api_key" | "x-api-key" => Ok(norte_code_wire::catalog::AuthKind::XApiKey),
        "x_goog_api_key" | "x-goog-api-key" => Ok(norte_code_wire::catalog::AuthKind::XGoogApiKey),
        "disabled" => Ok(norte_code_wire::catalog::AuthKind::Disabled),
        other => Err(format!(
            "unknown auth `{other}`; expected bearer, x_api_key, x_goog_api_key or disabled"
        )),
    }
}

fn default_base_url(schema: WireSchema) -> &'static str {
    match schema {
        WireSchema::OpenAI => "https://api.openai.com/v1",
        WireSchema::Anthropic => "https://api.anthropic.com",
        WireSchema::Google => "https://generativelanguage.googleapis.com",
    }
}

fn parse_schema(value: &str) -> Result<WireSchema, String> {
    match value {
        "openai" => Ok(WireSchema::OpenAI),
        "anthropic" => Ok(WireSchema::Anthropic),
        "google" => Ok(WireSchema::Google),
        other => Err(format!(
            "unknown wire schema `{other}`; expected openai, anthropic or google"
        )),
    }
}

fn add(id: &str, name: Option<&str>, keys: &[String]) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err("provider id must not be empty".to_owned());
    }
    norte_code_protocol::catalog::ensure_catalog_files()
        .map_err(|e| format!("{e}"))?;
    let (mut providers, models) = load_catalog_raw().map_err(|e| format!("{e}"))?;
    let wires = providers
        .providers
        .get(id)
        .map(|p| p.wires.clone())
        .unwrap_or_default();
    providers.providers.insert(
        id.to_owned(),
        Provider {
            name: name.unwrap_or(id).to_owned(),
            doc: None,
            environment_keys: keys.to_vec(),
            wires,
        },
    );
    save_catalog_files(&providers, &models).map_err(|e| format!("{e}"))?;

    println!(
        "  {} added provider {}",
        crate::output::ok("✓"),
        crate::output::accent(id)
    );
    if !keys.is_empty() {
        println!(
            "  {} key from: {}",
            crate::output::muted("→"),
            keys.join(", ")
        );
    }
    if let Ok(dir) = norte_code_protocol::catalog::catalog_dir() {
        println!(
            "  {} {}",
            crate::output::muted("→"),
            dir.join("providers.json").display()
        );
    }

    notify_catalog_if_up();
    Ok(())
}

fn add_wire(
    id: &str,
    schemas: &[String],
    base_url: Option<&str>,
    auth: Option<&str>,
) -> Result<(), String> {
    if schemas.is_empty() {
        return Err("at least one --schema is required (openai, anthropic or google)".to_owned());
    }
    norte_code_protocol::catalog::ensure_catalog_files()
        .map_err(|e| format!("{e}"))?;
    let (mut providers, models) = load_catalog_raw().map_err(|e| format!("{e}"))?;
    let provider = providers
        .providers
        .get_mut(id)
        .ok_or_else(|| format!("`{id}` is not in providers.json; add it first with `providers add`"))?;
    let auth = auth.map(parse_auth).transpose()?;
    for schema in schemas {
        let schema = parse_schema(schema)?;
        let api = base_url
            .map(str::to_owned)
            .unwrap_or_else(|| default_base_url(schema).to_owned());
        provider.wires.retain(|wire| wire.schema != schema);
        provider.wires.push(Wire {
            api,
            schema,
            auth,
        });
    }
    save_catalog_files(&providers, &models).map_err(|e| format!("{e}"))?;

    println!(
        "  {} added wires to {}",
        crate::output::ok("✓"),
        crate::output::accent(id)
    );
    notify_catalog_if_up();
    Ok(())
}

fn remove_wires(id: &str, schemas: &[String]) -> Result<(), String> {
    if schemas.is_empty() {
        return Err("at least one --schema is required".to_owned());
    }
    norte_code_protocol::catalog::ensure_catalog_files()
        .map_err(|e| format!("{e}"))?;
    let (mut providers, models) = load_catalog_raw().map_err(|e| format!("{e}"))?;
    let provider = providers
        .providers
        .get_mut(id)
        .ok_or_else(|| format!("`{id}` is not in providers.json"))?;
    let targets: Vec<WireSchema> = schemas.iter().map(|s| parse_schema(s)).collect::<Result<_, _>>()?;
    let before = provider.wires.len();
    provider.wires.retain(|wire| !targets.contains(&wire.schema));
    if before == provider.wires.len() {
        return Err(format!(
            "no matching wires found in `{id}`; see them with `providers get-wires {id}`"
        ));
    }
    save_catalog_files(&providers, &models).map_err(|e| format!("{e}"))?;

    println!(
        "  {} removed wires from {}",
        crate::output::ok("✓"),
        crate::output::accent(id)
    );
    notify_catalog_if_up();
    Ok(())
}

fn auth_label(auth: norte_code_wire::catalog::AuthKind) -> String {
    match auth {
        norte_code_wire::catalog::AuthKind::Bearer => "bearer".to_owned(),
        norte_code_wire::catalog::AuthKind::XApiKey => "x_api_key".to_owned(),
        norte_code_wire::catalog::AuthKind::XGoogApiKey => "x_goog_api_key".to_owned(),
        norte_code_wire::catalog::AuthKind::Disabled => "disabled".to_owned(),
    }
}

fn get_wires(id: &str) -> Result<(), String> {
    norte_code_protocol::catalog::ensure_catalog_files()
        .map_err(|e| format!("{e}"))?;
    let (providers, _models) = load_catalog_raw().map_err(|e| format!("{e}"))?;
    let provider = providers
        .providers
        .get(id)
        .ok_or_else(|| format!("`{id}` is not in providers.json"))?;
    println!(
        "  {}  {}",
        crate::output::accent(id),
        crate::output::muted(&format!("({} wires)", provider.wires.len()))
    );
    if provider.wires.is_empty() {
        println!("  {}", crate::output::muted("(none; add one with `providers add-wire`)"));
        return Ok(());
    }
    for wire in &provider.wires {
        let auth = wire.auth.map(|a| auth_label(a)).unwrap_or_else(|| {
            format!(
                "(schema default: {})",
                auth_label(norte_code_wire::catalog::AuthKind::for_schema(wire.schema))
            )
        });
        println!(
            "  {}  {}",
            crate::output::dim(&format!("{:<10}", wire.schema.label())),
            crate::output::muted(&wire.api)
        );
        println!(
            "       {}  {}",
            crate::output::muted("auth:"),
            crate::output::dim(&auth)
        );
    }
    Ok(())
}

fn remove(id: &str) -> Result<(), String> {
    norte_code_protocol::catalog::ensure_catalog_files()
        .map_err(|e| format!("{e}"))?;
    let (mut providers, mut models) = load_catalog_raw().map_err(|e| format!("{e}"))?;
    if providers.providers.remove(id).is_none() {
        return Err(format!(
            "`{id}` is not in providers.json; a plugin's providers are the plugin's to remove"
        ));
    }
    models.models.retain(|model| model.provider_id != *id);
    save_catalog_files(&providers, &models).map_err(|e| format!("{e}"))?;
    println!(
        "  {} removed provider {}",
        crate::output::ok("✓"),
        crate::output::accent(id)
    );
    notify_catalog_if_up();
    Ok(())
}

fn list() -> Result<(), String> {
    let url = crate::service::service_url();
    if crate::service::probe(&url) {
        if let Ok(client) = build_client(&url) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("could not start a runtime: {e}"))?;
            match rt.block_on(client.catalog_detail()) {
                Ok(detail) => {
                    if detail.providers.is_empty() {
                        let has_file_providers = load_catalog_raw()
                            .map(|(p, _)| !p.providers.is_empty())
                            .unwrap_or(false);
                        if has_file_providers {
                            drop(rt);
                            return print_file_catalog();
                        }
                    }
                    print_live_catalog(&detail);
                    return Ok(());
                }
                Err(_) => {}
            }
        }
    }
    print_file_catalog()
}

fn print_live_catalog(detail: &norte_code_client::api::CatalogDetail) {
    let mut files: Vec<&norte_code_wire::CatalogProviderInfo> = Vec::new();
    let mut plugins: Vec<&norte_code_wire::CatalogProviderInfo> = Vec::new();
    for provider in &detail.providers {
        if provider.from_file {
            files.push(provider);
        } else {
            plugins.push(provider);
        }
    }
    print_provider_group("catalogue (files)", &files);
    print_provider_group("plugins", &plugins);
}

fn print_provider_group(
    title: &str,
    providers: &[&norte_code_wire::CatalogProviderInfo],
) {
    println!(
        "  {}  {}",
        crate::output::bold(title),
        crate::output::muted(&format!("({} providers)", providers.len()))
    );
    if providers.is_empty() {
        println!("  {}", crate::output::muted("  (none)"));
        return;
    }
    for provider in providers {
        let api = provider.api.clone().unwrap_or_else(|| "(no wire)".to_owned());
        println!(
            "  {}  {}  {}",
            crate::output::accent(&provider.provider_id),
            crate::output::muted(&format!("— {}", provider.name)),
            crate::output::muted(&format!("· {} models", provider.model_count))
        );
        println!("       {}", crate::output::dim(&api));
    }
}

fn print_file_catalog() -> Result<(), String> {
    norte_code_protocol::catalog::ensure_catalog_files()
        .map_err(|e| format!("{e}"))?;
    let (providers, models) = load_catalog_raw().map_err(|e| format!("{e}"))?;
    println!(
        "  {}  {}",
        crate::output::bold("providers (files)"),
        crate::output::muted(&format!("({} providers)", providers.providers.len()))
    );
    if providers.providers.is_empty() {
        println!("  {}", crate::output::muted("(none)"));
        return Ok(());
    }
    let mut by_provider: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for model in &models.models {
        *by_provider.entry(&model.provider_id).or_insert(0) += 1;
    }
    for (id, provider) in &providers.providers {
        let api = provider
            .wires
            .first()
            .map(|w| w.api.clone())
            .unwrap_or_else(|| "(no wire)".to_owned());
        let count = by_provider.get(id.as_str()).copied().unwrap_or(0);
        println!(
            "  {}  {}  {}",
            crate::output::accent(id),
            crate::output::muted(&format!("— {}", provider.name)),
            crate::output::muted(&format!("· {count} models"))
        );
        println!("       {}", crate::output::dim(&api));
    }
    println!();
    println!(
        "  {} showing the catalogue files (start the service for plugin-injected providers)",
        crate::output::muted("→")
    );
    Ok(())
}

fn build_client(url: &str) -> Result<norte_code_client::RestClient, String> {
    let token = cli_token();
    norte_code_client::RestClient::new(url, &token).map_err(|e| format!("{e}"))
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

fn notify_catalog_if_up() {
    let url = crate::service::service_url();
    if !crate::service::probe(&url) {
        println!(
            "  {} service is offline — it will pick this up on start",
            crate::output::muted("→")
        );
        return;
    }
    let Ok(client) = build_client(&url) else {
        return;
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return,
    };
    match rt.block_on(client.command(None, norte_code_wire::ClientRequestKind::RefreshCatalog)) {
        Ok(_) => println!(
            "  {} refreshed the running service's catalogue",
            crate::output::muted("→")
        ),
        Err(error) => println!(
            "  {} could not refresh the running service: {error}",
            crate::output::muted("→")
        ),
    }
}