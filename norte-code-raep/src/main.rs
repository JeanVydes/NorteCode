//! Norte Code's server process.

use std::{net::SocketAddr, sync::Arc};

use norte_code_agent::{DEFAULT_SYSTEM_PROMPT, Service, VendorModels};
use norte_code_protocol::{
    catalog::load_catalog_files,
    config::{Config, user_config_path},
    sqlite::{SessionDirectory, SqliteStore},
};
use norte_code_raep::{HostConfig, NorteCodeRaepHost, router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let loaded = Config::load(user_config_path().as_deref(), None)?;
    let catalog = Arc::new(load_catalog_files()?);
    let database = std::env::var_os("NORTE_CODE_DATABASE").map_or_else(
        || std::path::PathBuf::from("norte-code.db"),
        std::path::PathBuf::from,
    );
    let store = Arc::new(SqliteStore::open(&database)?);
    let session_store: Arc<dyn norte_code_protocol::Store> = store.clone();
    let blobs: Arc<dyn norte::BlobStore> = store.clone();
    let directory: Arc<dyn SessionDirectory> = store;
    let service = Arc::new(
        Service::with_directory(
            loaded.config,
            catalog,
            None,
            Arc::new(VendorModels),
            session_store,
            blobs,
            directory,
            DEFAULT_SYSTEM_PROMPT,
        )
        .await?,
    );
    service.install_subagent_runner();
    let host = Arc::new(NorteCodeRaepHost::new(service, HostConfig::default())?);
    let app = router(host);
    let bind = std::env::var("NORTE_CODE_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8787".to_owned())
        .parse::<SocketAddr>()?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!("norte-code RAEP server listening on {bind}");
    axum::serve(listener, app).await?;
    Ok(())
}
