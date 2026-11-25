//! The client configuration: the `norte-code` cli.json, reduced to what the
//! interface needs to attach to the raep and paint itself.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use norte_code_client::config::{CliRuntimeConfig, ReconnectConfig, RetentionConfig};
use serde::Deserialize;
use terminalui::components::presentation::DotSize;
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

    /// The client runtime limits are outside the supported range.
    #[error("invalid client runtime configuration at {path}: {source}")]
    InvalidRuntime {
        /// Where the file lives.
        path: PathBuf,
        /// What failed validation.
        #[source]
        source: norte_code_client::config::CliConfigError,
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
    /// Show the floating frame and memory metrics box.
    #[serde(default)]
    pub(crate) debug: bool,
    /// Expand tool results by default.
    #[serde(default)]
    pub(crate) show_tool_output: bool,
    /// Group reasoning and tool activity in expandable transcript blocks.
    #[serde(default)]
    pub(crate) tool_verbosity: bool,
    /// Chart glyph density, clamped to 2..=6 by the renderer.
    #[serde(default = "default_chart_density")]
    pub(crate) chart_density: u8,
    /// Chart contrast from 0.25 through 4.0.
    #[serde(default = "default_chart_contrast")]
    pub(crate) chart_contrast: f32,
    /// Halftone dot glyph size.
    #[serde(default)]
    pub(crate) chart_dot_size: DotSize,
}

fn default_chart_density() -> u8 {
    4
}
fn default_chart_contrast() -> f32 {
    1.0
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
    /// Reconnect backoff for a dropped event stream.
    #[serde(default)]
    pub(crate) reconnect: ReconnectConfig,
    /// Transcript and console history limits.
    #[serde(default)]
    pub(crate) retention: RetentionConfig,
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
    /// Returns [`ConfigError::Json`], [`ConfigError::Io`] or
    /// [`ConfigError::InvalidRuntime`] when the file exists but cannot be used;
    /// a missing file falls back to the default local service, never to an
    /// error.
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
                reconnect: ReconnectConfig::default(),
                retention: RetentionConfig::default(),
            });
        }
        let raw = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        let config: Self = serde_json::from_str(&raw).map_err(|source| ConfigError::Json {
            path: path.clone(),
            source,
        })?;
        parse_runtime(&path, &raw)?;
        Ok(config)
    }
}

fn parse_runtime(path: &Path, raw: &str) -> Result<CliRuntimeConfig, ConfigError> {
    let config: CliRuntimeConfig =
        serde_json::from_str(raw).map_err(|source| ConfigError::Json {
            path: path.to_path_buf(),
            source,
        })?;
    config
        .validate()
        .map_err(|source| ConfigError::InvalidRuntime {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(config)
}

fn default_path() -> PathBuf {
    Path::new(CONFIG_FILE).to_path_buf()
}

fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".config").join("norte-code").join(CONFIG_FILE))
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
