use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{
    tools::DiffMode,
    ui_model::{ThemeMode, ThinkingVisibility, Toggle, TuiPreferences, UiModel},
};
use norte_code_wire::BudgetAction;

const CONFIG_FILE: &str = "client.json";
const LEGACY_CONFIG_FILE: &str = "cli.json";
const CONFIG_VERSION: u16 = 1;

#[derive(Debug, Error)]
pub(crate) enum ConfigError {
    #[error("HOME is not set; cannot locate {CONFIG_FILE}")]
    HomeMissing,
    #[error("could not access configuration at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid configuration at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("configuration path has no parent directory: {0}")]
    InvalidPath(PathBuf),
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ClientConfig {
    version: u16,
    pub(crate) service: ServiceConfig,
    pub(crate) agent_id: String,
    pub(crate) budget: BudgetConfig,
    pub(crate) preferences: PreferencesConfig,
    pub(crate) cli: CliConfig,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
    #[serde(skip)]
    path: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ServiceConfig {
    pub(crate) url: String,
    #[serde(rename = "type")]
    pub(crate) service_type: ServiceType,
    pub(crate) token: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServiceType {
    #[default]
    Local,
    Remote,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PreferencesConfig {
    pub(crate) developer_mode: Toggle,
    pub(crate) theme: ThemeMode,
    pub(crate) thinking_visibility: ThinkingVisibility,
    pub(crate) show_timestamps: Toggle,
    pub(crate) show_tool_output: Toggle,
    pub(crate) auto_refresh_sessions: Toggle,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct BudgetConfig {
    pub(crate) action: BudgetAction,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CliConfig {
    pub(crate) diff_style: DiffMode,
    pub(crate) charts: ChartsConfig,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ChartsConfig {
    pub(crate) contrast: u8,
    pub(crate) density: u8,
    pub(crate) dot_size: DotSize,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DotSize {
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct LegacyConfig {
    service_url: Option<String>,
    service_type: Option<ServiceType>,
    service_token: Option<String>,
    agent_id: Option<String>,
    developer_mode: Option<bool>,
    diff_style: Option<DiffMode>,
    charts: Option<ChartsConfig>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            service: ServiceConfig::default(),
            agent_id: "norte-code".to_owned(),
            budget: BudgetConfig::default(),
            preferences: PreferencesConfig::default(),
            cli: CliConfig::default(),
            extra: BTreeMap::new(),
            path: PathBuf::new(),
        }
    }
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8787".to_owned(),
            service_type: ServiceType::Local,
            token: None,
            extra: BTreeMap::new(),
        }
    }
}

impl Default for PreferencesConfig {
    fn default() -> Self {
        Self {
            developer_mode: Toggle::Off,
            theme: ThemeMode::Dark,
            thinking_visibility: ThinkingVisibility::Collapsed,
            show_timestamps: Toggle::On,
            show_tool_output: Toggle::On,
            auto_refresh_sessions: Toggle::Off,
            extra: BTreeMap::new(),
        }
    }
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            action: BudgetAction::Ask,
            extra: BTreeMap::new(),
        }
    }
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            diff_style: DiffMode::Unified,
            charts: ChartsConfig::default(),
            extra: BTreeMap::new(),
        }
    }
}

impl Default for ChartsConfig {
    fn default() -> Self {
        Self {
            contrast: 75,
            density: 82,
            dot_size: DotSize::Medium,
            extra: BTreeMap::new(),
        }
    }
}

impl ClientConfig {
    fn at_path(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            ..Self::default()
        }
    }

    pub(crate) fn load() -> Result<Self, ConfigError> {
        let path = default_path()?;
        let legacy_path = path.with_file_name(LEGACY_CONFIG_FILE);
        Self::load_from_paths(&path, &legacy_path)
    }

    fn load_from_paths(path: &Path, legacy_path: &Path) -> Result<Self, ConfigError> {
        if path.exists() {
            return Self::read(path);
        }
        if legacy_path.exists() {
            let legacy = Self::read_legacy(legacy_path)?;
            let config = Self::from_legacy(legacy, path.to_owned());
            config.save()?;
            fs::remove_file(legacy_path).map_err(|source| ConfigError::Io {
                path: legacy_path.to_owned(),
                source,
            })?;
            return Ok(config);
        }
        let config = Self::at_path(path);
        config.save()?;
        Ok(config)
    }

    fn read(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        let mut config: Self =
            serde_json::from_slice(&contents).map_err(|source| ConfigError::Json {
                path: path.to_owned(),
                source,
            })?;
        path.clone_into(&mut config.path);
        Ok(config)
    }

    fn read_legacy(path: &Path) -> Result<LegacyConfig, ConfigError> {
        let contents = fs::read(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        serde_json::from_slice(&contents).map_err(|source| ConfigError::Json {
            path: path.to_owned(),
            source,
        })
    }

    fn from_legacy(legacy: LegacyConfig, path: PathBuf) -> Self {
        let mut config = Self {
            path,
            ..Self::default()
        };
        if let Some(url) = legacy.service_url {
            config.service.url = url;
        }
        if let Some(service_type) = legacy.service_type {
            config.service.service_type = service_type;
        }
        config.service.token = legacy.service_token;
        if let Some(agent_id) = legacy.agent_id {
            config.agent_id = agent_id;
        }
        if let Some(developer_mode) = legacy.developer_mode {
            config.preferences.developer_mode = toggle(developer_mode);
        }
        if let Some(diff_style) = legacy.diff_style {
            config.cli.diff_style = diff_style;
        }
        if let Some(charts) = legacy.charts {
            config.cli.charts = charts;
        }
        config
    }

    pub(crate) fn save(&self) -> Result<(), ConfigError> {
        let path = if self.path.as_os_str().is_empty() {
            default_path()?
        } else {
            self.path.clone()
        };
        let parent = path
            .parent()
            .ok_or_else(|| ConfigError::InvalidPath(path.clone()))?;
        fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_owned(),
            source,
        })?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|source| ConfigError::Json {
            path: path.clone(),
            source,
        })?;
        write_atomic(&path, &bytes)?;
        Ok(())
    }

    pub(crate) fn apply_to(&self, model: &mut UiModel) {
        model.preferences = TuiPreferences {
            theme: self.preferences.theme,
            thinking: self.preferences.thinking_visibility,
            show_timestamps: self.preferences.show_timestamps,
            show_tool_output: self.preferences.show_tool_output,
            auto_refresh_sessions: self.preferences.auto_refresh_sessions,
            developer_mode: self.preferences.developer_mode,
        };
        model.cli.diff_style = self.cli.diff_style;
        model.budget_action = self.budget.action;
    }

    pub(crate) fn update_preferences(&mut self, model: &UiModel) {
        self.preferences.theme = model.preferences.theme;
        self.preferences.thinking_visibility = model.preferences.thinking;
        self.preferences.show_timestamps = model.preferences.show_timestamps;
        self.preferences.show_tool_output = model.preferences.show_tool_output;
        self.preferences.auto_refresh_sessions = model.preferences.auto_refresh_sessions;
        self.preferences.developer_mode = model.preferences.developer_mode;
        self.cli.diff_style = model.cli.diff_style;
        self.budget.action = model.budget_action;
    }
}

fn toggle(enabled: bool) -> Toggle {
    Toggle::from(enabled)
}

fn default_path() -> Result<PathBuf, ConfigError> {
    let home = env::var_os("HOME").ok_or(ConfigError::HomeMissing)?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("norte-code")
        .join(CONFIG_FILE))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let parent = path
        .parent()
        .ok_or_else(|| ConfigError::InvalidPath(path.to_owned()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let file_name = path
        .file_name()
        .ok_or_else(|| ConfigError::InvalidPath(path.to_owned()))?;
    let temporary_path = parent.join(format!(".{}.{}.tmp", file_name.to_string_lossy(), nonce));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary_path)
        .map_err(|source| ConfigError::Io {
            path: temporary_path.clone(),
            source,
        })?;
    if let Err(source) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temporary_path);
        return Err(ConfigError::Io {
            path: temporary_path,
            source,
        });
    }
    fs::rename(&temporary_path, path).map_err(|source| ConfigError::Io {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
