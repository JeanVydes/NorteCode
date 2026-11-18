//! The client configuration: the same `norte-code` cli.json the previous
//! client read, reduced to what the interface needs to attach to the raep.

use std::{
    collections::BTreeMap,
    fs,
    io,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

const CONFIG_FILE: &str = "cli.json";

/// What failed while reading or locating the configuration.
#[derive(Debug, Error)]
pub(crate) enum ConfigError {
    /// `HOME` is not set, so the configuration path is unknown.
    #[error("HOME is not set; cannot locate {CONFIG_FILE}")]
    HomeMissing,

    /// The file exists but cannot be read or parsed.
    #[error("invalid configuration at {path}: {source}")]
    Json {
        /// Where the file lives.
        path: PathBuf,
        /// What the parser said.
        #[source]
        source: serde_json::Error,
    },

    /// The file exists but is not readable.
    #[error("could not access configuration at {path}: {source}")]
    Io {
        /// Where the file lives.
        path: PathBuf,
        /// What the operating system said.
        #[source]
        source: io::Error,
    },
}

/// The service the client attaches to.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ServiceConfig {
    /// Base URL of the raep service.
    pub(crate) url: String,
    /// `local` when the service is spawned alongside the client.
    #[serde(default)]
    pub(crate) kind: ServiceType,
    /// Optional bearer token; `null` for a local service.
    #[serde(default)]
    pub(crate) token: Option<String>,
    /// Everything unknown is kept around so a merge never drops user data.
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

/// Whether the service is local or remote; informational only.
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServiceType {
    /// Spawned alongside the client.
    #[default]
    Local,
    /// Reached over the network.
    Remote,
}

/// The client configuration.
#[derive(Deserialize, Clone)]
pub(crate) struct ClientConfig {
    /// The raep service the UI attaches to.
    pub(crate) service: ServiceConfig,
    /// The agent id to attach to; `norte-code` by default.
    #[serde(default = "default_agent")]
    pub(crate) agent_id: String,
    /// Everything else is preserved untouched.
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, Value>,
}

fn default_agent() -> String {
    "norte-code".to_owned()
}

impl ClientConfig {
    /// The configuration for one attachment attempt.
    ///
    /// Reads the `cli.json` under `$HOME/.config/norte-code/`; when it is
    /// missing the default local service is used.
    ///
    /// # Errors
    /// Returns [`ConfigError::Json`] or [`ConfigError::Io`] when the file exists
    /// but cannot be read; a missing file falls back to the default local
    /// service, never to an error.
    pub(crate) fn load() -> Result<Self, ConfigError> {
        let path = config_path().unwrap_or_else(default_path);
        if !path.exists() {
            return Ok(Self {
                service: ServiceConfig {
                    url: "http://127.0.0.1:8787".to_owned(),
                    kind: ServiceType::Local,
                    token: None,
                    extra: BTreeMap::new(),
                },
                agent_id: default_agent(),
                extra: BTreeMap::new(),
            });
        }
        let raw = fs::read_to_string(&path).map_err(|source| ConfigError::Io { path: path.clone(), source })?;
        serde_json::from_str(&raw).map_err(|source| ConfigError::Json { path, source })
    }

    /// Use a fresh local service configuration for a test.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            service: ServiceConfig {
                url: "http://127.0.0.1:8787".to_owned(),
                kind: ServiceType::Local,
                token: None,
                extra: BTreeMap::new(),
            },
            agent_id: default_agent(),
            extra: BTreeMap::new(),
        }
    }
}

fn default_path() -> PathBuf {
    Path::new("cli.json").to_path_buf()
}

fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".config").join("norte-code").join(CONFIG_FILE))
}
