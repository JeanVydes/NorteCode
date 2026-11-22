//! RAEP server integration for Norte Code.
//!
//! This crate is the server-side migration boundary. It owns no model,
//! tool, session or persistence policy: those remain in
//! [`norte_code_agent::Service`]. It translates RAEP frames into the existing
//! Norte Code service protocol and translates service events back into RAEP
//! envelopes for any client transport.

#![forbid(unsafe_code)]

pub mod bundle;
mod commands;
mod host;
mod http;
mod payloads;
mod plugin_display;
pub mod plugin_manager;

pub use commands::AgentCommand;
pub use host::replay::ReplayLog;
pub use host::stores::{ArtifactRef, ArtifactStore, BlobArtifactStore};
pub use host::uploads::{
    MemoryUploadIndex, SqliteUploadIndex, UploadIndex, UploadReceipt, UploadRecord,
};
pub use host::{HostConfig, HostError, NorteCodeRaepHost};
pub use http::router;

/// Extensions owned by the `NorteCode` server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NorteCodeExtension {
    /// Carries a complete `NorteCode` [`norte_code_wire::ClientRequest`].
    Command,
    /// Carries service telemetry.
    Telemetry,
    /// Carries service tool and rule state.
    ToolList,
    /// Carries plugin lifecycle data.
    Plugin,
}

impl NorteCodeExtension {
    /// Returns the namespaced wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Command => "norte_code.command",
            Self::Telemetry => "norte_code.telemetry",
            Self::ToolList => "norte_code.tool_list",
            Self::Plugin => "norte_code.plugin",
        }
    }
}

use std::{
    collections::{BTreeMap, HashMap},
    net::SocketAddr,
    sync::Arc,
};

use norte_code_agent::{DEFAULT_SYSTEM_PROMPT, ModelSpec, Service, VendorModels};
use norte_code_plugin_host::load_plugins;
use norte_code_plugin_host::plugins::{Catalogue, Plugins};
use norte_code_protocol::{
    catalog::{Catalog, CatalogError, ensure_catalog_files, load_catalog_files},
    config::{
        Agent, ServiceConfig, ensure_user_service_config, project_config_path, user_config_path,
    },
    sqlite::{SessionDirectory, SqliteStore},
};

/// Wires the 0600 `secrets.json` into every agent's config so a key the
/// client writes there is usable on the host.
fn install_file_key_store(agents: Vec<Agent>) -> Vec<Agent> {
    agents
        .into_iter()
        .map(|mut agent| {
            agent.keys = Some(std::sync::Arc::new(norte_code_protocol::keys::FileKeyStore));
            agent
        })
        .collect()
}
use norte_code_wire::catalog::{Models, Providers};

/// Moves pins written by the file-based pin store into the session stats.
///
/// The file store was replaced by pins inside the session records, which is
/// what the compaction reads; this rewrites their files once, then renames
/// the directory so it never runs again. Failures are reported and skipped;
/// nothing here may stop the server from starting.
async fn migrate_pins_to_session_stats(
    services: &HashMap<String, Arc<Service>>,
    dir: &std::path::Path,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut migrated = true;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(session) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned)
        else {
            migrated = false;
            continue;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            migrated = false;
            continue;
        };
        let Ok(turn_ids) = serde_json::from_str::<Vec<String>>(&content) else {
            migrated = false;
            continue;
        };
        if turn_ids.is_empty() {
            continue;
        }
        let mut applied = false;
        for service in services.values() {
            if service
                .pin_turns(
                    &norte_code_wire::SessionKey::new(session.clone()),
                    &turn_ids,
                    true,
                )
                .await
                .is_ok()
            {
                applied = true;
                break;
            }
        }
        if !applied {
            eprintln!("warning: could not migrate pins for session `{session}`; skipping it");
            migrated = false;
        }
    }
    if migrated {
        let renamed = dir.with_file_name("pins.migrated");
        if let Err(error) = std::fs::rename(dir, &renamed) {
            eprintln!(
                "warning: could not rename {} after pin migration: {error}",
                dir.display()
            );
        }
    }
}

async fn build_services(
    agents: Vec<Agent>,
    catalog: Arc<Catalog>,
    session_store: Arc<dyn norte_code_protocol::Store>,
    blobs: Arc<dyn norte::BlobStore>,
    directory: Arc<dyn SessionDirectory>,
    offered: &Providers,
    offered_models: &Models,
    plugin_tools: &[Arc<dyn norte::AnyFunction>],
) -> Result<HashMap<String, Arc<Service>>, Box<dyn std::error::Error>> {
    let mut services = HashMap::new();
    let profiles: BTreeMap<String, Agent> = agents
        .iter()
        .cloned()
        .map(|agent| (agent.name.clone(), agent))
        .collect();
    for agent in agents {
        let default_spec = match (
            agent.default_provider.as_deref(),
            agent.default_model.as_deref(),
        ) {
            (Some(provider), Some(model)) => {
                match ModelSpec::from_catalog(&agent, &catalog, provider, model, None) {
                    Ok(spec) => Some(spec),
                    // A configured default that the catalogue no longer carries must
                    // not abort startup; config reload already treats it as a warning.
                    Err(error) => {
                        eprintln!(
                            "norte-code: default provider/model not in catalogue for agent `{}`: {error}",
                            agent.name
                        );
                        None
                    }
                }
            }
            (None, None) => None,
            _ => {
                return Err(format!(
                    "agent `{}` must set both default_provider and default_model",
                    agent.name
                )
                .into());
            }
        };
        let service = Arc::new(
            Service::with_directory(
                agent.clone(),
                Arc::clone(&catalog),
                default_spec,
                Arc::new(VendorModels),
                Arc::clone(&session_store),
                Arc::clone(&blobs),
                Arc::clone(&directory),
                DEFAULT_SYSTEM_PROMPT,
            )
            .await?,
        )
        .with_agent_profiles(profiles.clone());
        service.set_plugin_catalogue(offered.clone(), offered_models.clone());
        service.set_plugin_tools(plugin_tools.to_vec());
        service.install_subagent_runner();
        services.insert(agent.name, service);
    }
    Ok(services)
}

fn load_catalog(offered: Catalogue) -> Result<Catalog, Box<dyn std::error::Error>> {
    match load_catalog_files() {
        Ok(catalog) => Ok(catalog),
        Err(_error) if !offered.providers.is_empty() && !offered.models.is_empty() => {
            Ok(Catalog::new(
                Providers {
                    providers: offered.providers,
                },
                Models {
                    models: offered.models,
                },
            )?)
        }
        Err(CatalogError::NoProviders { .. } | CatalogError::NoModels { .. })
            if offered.providers.is_empty() && offered.models.is_empty() =>
        {
            eprintln!("catalogue is empty; install a catalogue plugin before starting a session");
            Ok(Catalog::default())
        }
        Err(error) => Err(error.into()),
    }
}

fn allowed_origins() -> Vec<String> {
    std::env::var("NORTE_CODE_ALLOWED_ORIGINS").map_or_else(
        |_| HostConfig::default().allowed_origins,
        |origins| {
            origins
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(str::to_owned)
                .collect()
        },
    )
}

/// Runs the RAEP server process until it is asked to stop.
///
/// # Errors
///
/// Returns an error when the server cannot start.
pub async fn run_server() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = std::env::current_dir()?;
    let project = project_config_path(&workspace);
    ensure_user_service_config()?;
    let service_config = ServiceConfig::load(user_config_path().as_deref(), Some(&project))?;
    let agent = service_config.default_agent()?;
    let agent_id = agent.name.clone();
    let mut plugins = Plugins::new();
    load_plugins(&mut plugins, None);
    let plugin_tools = plugins.tools();
    crate::plugin_display::capture(plugins.status_named(), plugins.banner_named());
    let offered = plugins.catalogue();
    let created = ensure_catalog_files()?;
    if created.providers || created.models {
        eprintln!(
            "created the default server catalogue; edit providers.json and models.json if needed"
        );
    }
    let mut catalog = load_catalog(offered.clone())?;
    for skipped in catalog.extend(offered.providers.clone(), offered.models.clone()) {
        eprintln!("warning: {skipped}");
    }
    let catalog = Arc::new(catalog);
    let database = database_path()?;
    let store = Arc::new(SqliteStore::open(&database)?);
    let session_store: Arc<dyn norte_code_protocol::Store> = store.clone();
    let blobs: Arc<dyn norte::BlobStore> = store.clone();
    let directory: Arc<dyn SessionDirectory> = store;
    let offered_providers = Providers {
        providers: offered.providers.clone(),
    };
    let offered_models = Models {
        models: offered.models.clone(),
    };
    let services = build_services(
        install_file_key_store(service_config.agents),
        catalog,
        session_store,
        Arc::clone(&blobs),
        directory,
        &offered_providers,
        &offered_models,
        &plugin_tools,
    )
    .await?;
    migrate_pins_to_session_stats(&services, &database.with_file_name("pins")).await;
    let auth_principals = std::env::var("NORTE_CODE_AUTH_USERS")
        .ok()
        .map(|users| {
            users
                .split(',')
                .filter_map(|entry| {
                    let (token, principal) = entry.split_once('=')?;
                    if token.is_empty() || principal.is_empty() {
                        None
                    } else {
                        Some((token.to_owned(), principal.to_owned()))
                    }
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let host_config = HostConfig {
        agent_id,
        auth_token: std::env::var("NORTE_CODE_AUTH_TOKEN").ok(),
        auth_principals,
        allowed_origins: allowed_origins(),
        ..HostConfig::default()
    };
    let replay_log = ReplayLog::new(database.with_extension("replay"), host_config.replay_bytes);
    let upload_index = Arc::new(SqliteUploadIndex::open(
        database.with_extension("uploads.db"),
    )?);
    let artifact_store = Arc::new(BlobArtifactStore::new(blobs));
    let host = Arc::new(
        NorteCodeRaepHost::with_agents(services, host_config)?
            .with_replay_log(replay_log)
            .with_artifact_store(artifact_store)
            .with_upload_index(upload_index),
    );
    // The service's delegation events are emitted on their own broadcast;
    // this drain is what turns them into the standard RAEP frames on the
    // parent's stream (RFC 8.2.10).
    host.spawn_delegation_drain()?;
    let app = router(host);
    let bind = std::env::var("NORTE_CODE_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8787".to_owned())
        .parse::<SocketAddr>()?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let actual_bind = listener.local_addr()?;
    if let Some(path) = std::env::var_os("NORTE_CODE_READY_FILE") {
        std::fs::write(&path, actual_bind.to_string())?;
    }
    eprintln!("norte-code RAEP server listening on {actual_bind}");
    let server = axum::serve(listener, app);
    tokio::select! {
        result = server => result?,
        () = shutdown_signal() => {}
    }
    Ok(())
}

/// Stops the server when the process is asked to stop.
///
/// The terminal asks with `Ctrl+C` and `kill` asks with `SIGTERM`, and both
/// are handled here. Handling them matters for a server that was started in
/// the background: the shell leaves `SIGINT` ignored in a background job, so
/// without a handler of its own the server would ignore the signal that
/// usually stops it.
async fn shutdown_signal() {
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("norte-code: could not listen for Ctrl+C: {error}");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                eprintln!("norte-code: could not listen for SIGTERM: {error}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    eprintln!("norte-code: shutting down");
}
/// The path the service stores its state in: the sessions database and the
/// sibling files that hang off it (replay log, upload index, pin migration
/// output).
///
/// It defaults to `$XDG_DATA_HOME/norte-code/sessions.db` (or the platform
/// equivalent), not `./norte-code.db`: a service whose state files land in
/// whatever directory it happens to be launched from spreads its files
/// across the filesystem and, on the project root, pollutes the repository.
/// `NORTE_CODE_DATABASE` overrides it.
fn database_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    use std::path::PathBuf;

    if let Some(path) = std::env::var_os("NORTE_CODE_DATABASE") {
        let path = PathBuf::from(path);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let _ = std::fs::create_dir_all(parent);
        }
        return Ok(path);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .ok_or("neither `XDG_DATA_HOME`, `HOME` nor `APPDATA` is set; set NORTE_CODE_DATABASE")?;
    let default = base.join("norte-code").join("sessions.db");
    if let Some(parent) = default.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    Ok(default)
}
