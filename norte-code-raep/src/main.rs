//! Norte Code's server process.

use std::{
    collections::{BTreeMap, HashMap},
    net::SocketAddr,
    sync::Arc,
};

use norte_code_agent::{DEFAULT_SYSTEM_PROMPT, ModelSpec, Service, VendorModels};
use norte_code_plugin_host::load_plugins;
use norte_code_protocol::{
    catalog::{Catalog, Models, Providers, ensure_catalog_files, load_catalog_files},
    config::{
        Agent, ServiceConfig, ensure_user_service_config, project_config_path, user_config_path,
    },
    plugins::Plugins,
    sqlite::{SessionDirectory, SqliteStore},
};
use norte_code_raep::{
    BlobArtifactStore, HostConfig, NorteCodeRaepHost, SqliteReplayStore, router,
};

async fn build_services(
    agents: Vec<Agent>,
    catalog: Arc<Catalog>,
    session_store: Arc<dyn norte_code_protocol::Store>,
    blobs: Arc<dyn norte::BlobStore>,
    directory: Arc<dyn SessionDirectory>,
    offered: &Providers,
    offered_models: &Models,
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
            (Some(provider), Some(model)) => Some(ModelSpec::from_catalog(
                &agent, &catalog, provider, model, None,
            )?),
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
        service.install_subagent_runner();
        services.insert(agent.name, service);
    }
    Ok(services)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = std::env::current_dir()?;
    let project = project_config_path(&workspace);
    ensure_user_service_config()?;
    let service_config = ServiceConfig::load(user_config_path().as_deref(), Some(&project))?;
    let agent = service_config.default_agent()?;
    let agent_id = agent.name.clone();
    let mut plugins = Plugins::new();
    load_plugins(&mut plugins, None);
    let offered = plugins.catalogue();
    let created = ensure_catalog_files()?;
    if created.providers || created.models {
        eprintln!(
            "created the default server catalogue; edit providers.json and models.json if needed"
        );
    }
    let mut catalog = match load_catalog_files() {
        Ok(catalog) => catalog,
        Err(_error) if !offered.providers.is_empty() && !offered.models.is_empty() => Catalog::new(
            Providers {
                providers: offered.providers.clone(),
            },
            Models {
                models: offered.models.clone(),
            },
        )?,
        Err(error) => return Err(error.into()),
    };
    for skipped in catalog.extend(offered.providers.clone(), offered.models.clone()) {
        eprintln!("warning: {skipped}");
    }
    let catalog = Arc::new(catalog);
    let database = std::env::var_os("NORTE_CODE_DATABASE").map_or_else(
        || std::path::PathBuf::from("norte-code.db"),
        std::path::PathBuf::from,
    );
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
        service_config.agents,
        catalog,
        session_store,
        Arc::clone(&blobs),
        directory,
        &offered_providers,
        &offered_models,
    )
    .await?;
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
        ..HostConfig::default()
    };
    let replay_database = database.with_extension("replay.db");
    let replay_store = Arc::new(SqliteReplayStore::open(
        replay_database,
        host_config.replay_capacity,
    )?);
    let artifact_store = Arc::new(BlobArtifactStore::new(blobs));
    let host = Arc::new(
        NorteCodeRaepHost::with_agents(services, host_config)?
            .with_replay_store_instance(replay_store)
            .with_artifact_store(artifact_store),
    );
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
    axum::serve(listener, app).await?;
    Ok(())
}
