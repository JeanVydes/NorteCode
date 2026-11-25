//! The client configuration: the `norte-code` cli.json, reduced to what the
//! interface needs to attach to the raep and paint itself.

use std::{
    fs, io,
    io::Write,
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
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
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
    pub(crate) reasoning: Option<crate::ui_model::ReasoningLevel>,
    pub(crate) send_mode: Option<crate::ui_model::SendMode>,
    pub(crate) diff_mode: Option<crate::diff::DiffMode>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: None,
            themes: Vec::new(),
            rail_position: None,
            debug: false,
            show_tool_output: false,
            tool_verbosity: false,
            chart_density: 4,
            chart_contrast: 1.0,
            chart_dot_size: DotSize::Medium,
            reasoning: None,
            send_mode: None,
            diff_mode: None,
        }
    }
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
    #[serde(default)]
    pub(crate) service: ServiceConfig,
    /// CLI-compatible flat service address.
    #[serde(default)]
    service_url: Option<String>,
    /// CLI-compatible flat bearer token.
    #[serde(default)]
    service_auth: Option<String>,
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

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8787".to_owned(),
            token: None,
        }
    }
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
            create_default(&path)?;
        }
        let raw = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        let mut config: Self = serde_json::from_str(&raw).map_err(|source| ConfigError::Json {
            path: path.clone(),
            source,
        })?;
        let root: serde_json::Value =
            serde_json::from_str(&raw).map_err(|source| ConfigError::Json {
                path: path.clone(),
                source,
            })?;
        if root.get("service").is_none() {
            if let Some(url) = config.service_url.take() {
                config.service.url = url;
            }
            config.service.token = config.service_auth.take();
        }
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
    norte_code_protocol::config::user_cli_path()
}

fn create_default(path: &Path) -> Result<(), ConfigError> {
    let create = || -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => file.write_all(include_bytes!("../../default/cli.json")),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    };
    create().map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Persist the user preferences while preserving settings owned by other clients.
pub(crate) fn save_preferences(
    preferences: &crate::ui_model::Preferences,
) -> Result<(), ConfigError> {
    let path = config_path().unwrap_or_else(default_path);
    save_preferences_at(&path, preferences)
}

fn save_preferences_at(
    path: &Path,
    preferences: &crate::ui_model::Preferences,
) -> Result<(), ConfigError> {
    if !path.exists() {
        create_default(path)?;
    }
    let raw = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut document: serde_json::Value =
        serde_json::from_str(&raw).map_err(|source| ConfigError::Json {
            path: path.to_path_buf(),
            source,
        })?;
    let object = document.as_object_mut().ok_or_else(|| ConfigError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration must be an object",
        ),
    })?;
    let ui = object
        .entry("ui")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let ui = ui.as_object_mut().ok_or_else(|| ConfigError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidData, "ui must be an object"),
    })?;
    let saved = SavedPreferences::from(preferences);
    let serialized = serde_json::to_value(saved).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    if let serde_json::Value::Object(fields) = serialized {
        ui.extend(fields);
    }
    let output = serde_json::to_vec_pretty(&document).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    atomic_write(path, &output).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(serde::Serialize)]
struct SavedPreferences<'a> {
    theme: &'a str,
    rail_position: RailPosition,
    debug: bool,
    show_tool_output: bool,
    tool_verbosity: bool,
    chart_density: u8,
    chart_contrast: f32,
    chart_dot_size: DotSize,
    reasoning: crate::ui_model::ReasoningLevel,
    send_mode: crate::ui_model::SendMode,
    diff_mode: crate::diff::DiffMode,
}

impl<'a> From<&'a crate::ui_model::Preferences> for SavedPreferences<'a> {
    fn from(p: &'a crate::ui_model::Preferences) -> Self {
        Self {
            theme: crate::theme::theme_name(&p.theme),
            rail_position: p.rail_position,
            debug: p.debug.enabled(),
            show_tool_output: p.show_tool_output,
            tool_verbosity: p.tool_verbosity,
            chart_density: p.chart_density,
            chart_contrast: f32::from(p.chart_contrast) / 4.0,
            chart_dot_size: match p.chart_dot_size {
                0 => DotSize::Small,
                2 => DotSize::Large,
                _ => DotSize::Medium,
            },
            reasoning: p.reasoning,
            send_mode: p.send_mode,
            diff_mode: p.diff_mode,
        }
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut attempt = 0u32;
    loop {
        let temporary = parent.join(format!(".cli.json.{}.{}.tmp", std::process::id(), attempt));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(mut file) => {
                let result = (|| {
                    file.set_permissions(fs::metadata(path)?.permissions())?;
                    file.write_all(bytes)?;
                    file.write_all(b"\n")?;
                    file.sync_all()?;
                    fs::rename(&temporary, path)
                })();
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                return result;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
