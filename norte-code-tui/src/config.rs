//! The client configuration: the `norte-code` cli.json, reduced to what the
//! interface needs to attach to the raep and paint itself.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

use crate::theme::CustomThemeConfig;
use crate::ui::RailPosition;

const CONFIG_FILE: &str = "cli.json";

/// What failed while reading or locating the configuration.
#[derive(Debug, Error)]
pub(crate) enum ConfigError {
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
    /// Optional bearer token; `null` for a local service.
    #[serde(default)]
    pub(crate) token: Option<String>,
}

/// The interface configuration, under the `ui` key.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct UiConfig {
    /// Name of the active theme, built-in or configured.
    #[serde(default)]
    pub(crate) theme: Option<String>,
    /// Extra themes, in cycle order.
    #[serde(default)]
    pub(crate) themes: Vec<CustomThemeConfig>,
    /// Where the session rail sits; `left` when absent.
    #[serde(default)]
    pub(crate) rail_position: Option<RailPosition>,
}

/// The client configuration.
#[derive(Clone, Deserialize)]
pub(crate) struct ClientConfig {
    /// The raep service the UI attaches to.
    pub(crate) service: ServiceConfig,
    /// The agent id to attach to; `norte-code` by default.
    #[serde(default = "default_agent")]
    pub(crate) agent_id: String,
    /// Theme selection and custom themes.
    #[serde(default)]
    pub(crate) ui: UiConfig,
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
                    token: None,
                },
                agent_id: default_agent(),
                ui: UiConfig::default(),
            });
        }
        let raw = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        serde_json::from_str(&raw).map_err(|source| ConfigError::Json { path, source })
    }
}

fn default_path() -> PathBuf {
    Path::new("cli.json").to_path_buf()
}

fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".config").join("norte-code").join(CONFIG_FILE))
}
