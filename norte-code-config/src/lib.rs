//! Norte Code configuration.
//!
//! Loads, validates and merges `config.json`: a user file plus an optional
//! per-project overlay, where the project wins. Every key has a documented
//! default, and a validation error names the field that is wrong.
//!
//! Secrets are not stored here by default. A provider names an environment
//! variable (`api_key_env`) or a file (`api_key_file`); the key is resolved on
//! demand into a [`ApiKey`], whose `Debug` output is redacted.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only config schema revision this build understands.
pub const CONFIG_VERSION: u32 = 1;

/// Where the catalogue is fetched from unless configured otherwise.
pub const DEFAULT_CATALOG_URL: &str = "https://models.dev/catalog.json";

/// The whole configuration file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema revision. Must be [`CONFIG_VERSION`].
    pub version: u32,
    /// The `provider/model` a new session uses when none is named.
    pub default_model: Option<String>,
    /// Per-provider settings.
    pub providers: BTreeMap<String, ProviderConfig>,
    /// The model catalogue.
    pub catalog: CatalogConfig,
    /// Automatic compaction.
    pub compaction: CompactionConfig,
    /// The session's default reasoning setting.
    pub reasoning: ReasoningConfig,
    /// How tool calls are approved.
    pub approval_mode: ApprovalMode,
    /// How many requests one turn may spend.
    pub max_iterations: usize,
    /// Display settings for cost.
    pub cost: CostConfig,
    /// Optional spending limits.
    pub budgets: BudgetsConfig,
    /// Tool switches.
    pub tools: ToolsConfig,
    /// Agent Skills settings.
    pub skills: SkillsConfig,
    /// Per-model overrides, keyed by `provider/model`.
    pub model_overrides: BTreeMap<String, ModelOverride>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            default_model: None,
            providers: BTreeMap::new(),
            catalog: CatalogConfig::default(),
            compaction: CompactionConfig::default(),
            reasoning: ReasoningConfig::default(),
            approval_mode: ApprovalMode::default(),
            max_iterations: 32,
            cost: CostConfig::default(),
            budgets: BudgetsConfig::default(),
            tools: ToolsConfig::default(),
            skills: SkillsConfig::default(),
            model_overrides: BTreeMap::new(),
        }
    }
}

/// One provider's settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// The environment variable holding the API key.
    pub api_key_env: Option<String>,
    /// The file holding the API key.
    pub api_key_file: Option<PathBuf>,
    /// A base URL override, for a proxy or a compatible vendor.
    pub endpoint: Option<String>,
}

/// Catalogue settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogConfig {
    /// Where the catalogue is fetched from.
    pub url: String,
    /// How long a cached catalogue stays fresh.
    pub ttl_hours: u64,
    /// Never touch the network.
    pub offline: bool,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_CATALOG_URL.to_owned(),
            ttl_hours: 24,
            offline: false,
        }
    }
}

/// Compaction settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    /// Whether compaction may run.
    pub enabled: bool,
    /// The used-context fraction that triggers it.
    pub threshold: f64,
    /// How many recent messages to keep verbatim.
    pub keep_recent: usize,
    /// A model to summarise with, or the session's own.
    pub model: Option<String>,
    /// The reasoning setting for the summary request.
    pub reasoning: String,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.90,
            keep_recent: 6,
            model: None,
            reasoning: "lowest".to_owned(),
        }
    }
}

/// The default reasoning setting.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReasoningConfig {
    /// `auto`, `off`, an effort name, or a `{budget}` object.
    pub default: Value,
}

impl Default for ReasoningConfig {
    fn default() -> Self {
        Self {
            default: Value::String("auto".to_owned()),
        }
    }
}

/// How tool calls are approved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    /// Ask before every write and command.
    #[default]
    Ask,
    /// Allow edits without asking; commands still ask.
    AutoEdit,
    /// Allow everything.
    Yolo,
}

/// Cost display settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CostConfig {
    /// Whether to show cost in the UI and the CLI.
    pub show: bool,
}

impl Default for CostConfig {
    fn default() -> Self {
        Self { show: true }
    }
}

/// The three budget scopes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetsConfig {
    /// One message.
    pub message: Option<BudgetConfig>,
    /// One session.
    pub session: Option<BudgetConfig>,
    /// The day, across sessions.
    pub global_daily: Option<BudgetConfig>,
}

/// One budget.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    /// `usd`, `tokens` or `requests`.
    pub unit: String,
    /// The limit.
    pub limit: f64,
    /// `warn`, `ask` or `stop`.
    pub action: String,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            unit: "usd".to_owned(),
            limit: 0.0,
            action: "warn".to_owned(),
        }
    }
}

/// Tool switches.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Whether the `web_fetch` tool is registered.
    pub web_fetch: bool,
}

/// Agent Skills settings.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    /// Whether skills are discovered and their tools registered.
    pub enabled: bool,
    /// Extra discovery roots, in order, after the user root.
    pub paths: Vec<PathBuf>,
    /// Also read the `.agents/skills` interoperability roots.
    pub interop: bool,
    /// Whether `skill_script` is registered. Off by default.
    pub allow_scripts: bool,
    /// Whether a skill's `allowed-tools` may pre-approve calls. Off by default.
    pub honor_allowed_tools: bool,
    /// The largest number of valid skills kept.
    pub max_skills: usize,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            paths: Vec::new(),
            interop: true,
            allow_scripts: false,
            honor_allowed_tools: false,
            max_skills: 128,
        }
    }
}

/// A per-model override of what the catalogue says.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelOverride {
    /// Context window in tokens.
    pub max_context: Option<u64>,
    /// Output limit in tokens.
    pub max_output: Option<u64>,
    /// A reasoning override.
    pub reasoning: Option<ReasoningOverride>,
}

/// A per-model override of the catalogue's reasoning declaration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReasoningOverride {
    /// Whether the model reasons at all.
    pub supported: Option<bool>,
    /// Whether reasoning may be turned off.
    pub can_disable: Option<bool>,
    /// The effort names to expose.
    pub efforts: Option<Vec<String>>,
    /// Whether a token budget is accepted.
    pub supports_budget: Option<bool>,
    /// The largest accepted budget.
    pub max_budget: Option<u32>,
}

/// A resolved API key. Its `Debug` output is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wraps a key.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key itself.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// The result of a load: the config plus the keys that were ignored.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// The merged configuration.
    pub config: Config,
    /// Unknown keys, as dotted paths, to report to the user.
    pub warnings: Vec<String>,
}

/// A configuration failure.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A file was not valid JSON, or did not match the schema.
    #[error("{path} is not valid config: {source}")]
    Parse {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: serde_json::Error,
    },
    /// A value was present but wrong.
    #[error("config is invalid: {path}: {message}")]
    Invalid {
        /// The offending field.
        path: String,
        /// What is wrong with it.
        message: String,
    },
    /// An API key could not be resolved.
    #[error("could not resolve the API key for {provider}: {message}")]
    Secret {
        /// The provider.
        provider: String,
        /// What went wrong.
        message: String,
    },
}

impl Config {
    /// Loads and validates a configuration from a user file and an optional
    /// project overlay. The project wins for every key it sets.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when a file cannot be read, is not valid JSON or
    /// fails validation.
    pub fn load(user: Option<&Path>, project: Option<&Path>) -> Result<Loaded, ConfigError> {
        let mut value = Value::Object(serde_json::Map::new());
        if let Some(path) = user {
            if path.exists() {
                merge(&mut value, read_value(path)?);
            }
        }
        if let Some(path) = project {
            if path.exists() {
                merge(&mut value, read_value(path)?);
            }
        }
        Self::from_value(value)
    }

    /// Parses and validates a configuration from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the string is not valid JSON or fails
    /// validation.
    pub fn from_json(text: &str) -> Result<Loaded, ConfigError> {
        Self::from_value(read_text(text)?)
    }

    /// Validates an already-parsed [`Value`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the value does not match the schema.
    pub fn from_value(value: Value) -> Result<Loaded, ConfigError> {
        let mut warnings = Vec::new();
        let config: Config = serde_ignored::deserialize(value, |path| {
            warnings.push(path.to_string());
        })
        .map_err(|source| ConfigError::Parse {
            path: PathBuf::from("<memory>"),
            source,
        })?;
        config.validate()?;
        Ok(Loaded { config, warnings })
    }

    /// Checks the invariants the loader cannot express.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] naming the first field that is wrong.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(invalid(
                "version",
                format!(
                    "expected {CONFIG_VERSION}, found {}; migrate the file",
                    self.version
                ),
            ));
        }
        if self.max_iterations == 0 {
            return Err(invalid("max_iterations", "must be at least 1"));
        }
        if let Some(model) = &self.default_model {
            if model.split_once('/').is_none() {
                return Err(invalid(
                    "default_model",
                    format!("`{model}` is not a `provider/model` reference"),
                ));
            }
        }
        if self.skills.max_skills == 0 {
            return Err(invalid("skills.max_skills", "must be at least 1"));
        }
        if !(self.compaction.threshold > 0.0 && self.compaction.threshold <= 1.0) {
            return Err(invalid(
                "compaction.threshold",
                "must be a fraction in (0, 1]",
            ));
        }
        for (scope, budget) in [
            ("message", &self.budgets.message),
            ("session", &self.budgets.session),
            ("global_daily", &self.budgets.global_daily),
        ] {
            if let Some(budget) = budget {
                if budget.limit <= 0.0 {
                    return Err(invalid(
                        format!("budgets.{scope}.limit"),
                        "must be greater than zero",
                    ));
                }
                if !matches!(budget.unit.as_str(), "usd" | "tokens" | "requests") {
                    return Err(invalid(
                        format!("budgets.{scope}.unit"),
                        "must be `usd`, `tokens` or `requests`",
                    ));
                }
                if !matches!(budget.action.as_str(), "warn" | "ask" | "stop") {
                    return Err(invalid(
                        format!("budgets.{scope}.action"),
                        "must be `warn`, `ask` or `stop`",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Resolves a provider's API key.
    ///
    /// The config's `api_key_env` wins, then its `api_key_file`, then the
    /// catalogue's `env` list (`default_env`). The catalogue is what makes a
    /// new provider work with nothing but its usual environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Secret`] when no source yields a non-empty key.
    pub fn resolve_api_key(
        &self,
        provider: &str,
        default_env: &[String],
    ) -> Result<ApiKey, ConfigError> {
        let entry = self.providers.get(provider);
        let explicit_env = entry.and_then(|entry| entry.api_key_env.as_deref());
        let file = entry.and_then(|entry| entry.api_key_file.as_deref());

        let mut tried = Vec::new();
        for name in explicit_env
            .into_iter()
            .chain(default_env.iter().map(String::as_str))
        {
            tried.push(name.to_owned());
            if let Ok(value) = std::env::var(name) {
                if !value.trim().is_empty() {
                    return Ok(ApiKey::new(value.trim().to_owned()));
                }
            }
        }
        if let Some(path) = file {
            let text = fs::read_to_string(path).map_err(|source| ConfigError::Secret {
                provider: provider.to_owned(),
                message: format!("could not read {}: {source}", path.display()),
            })?;
            let key = text.trim();
            if !key.is_empty() {
                return Ok(ApiKey::new(key.to_owned()));
            }
        }

        let message = if tried.is_empty() {
            "no `api_key_env`, `api_key_file` or catalogue `env` names to try".to_owned()
        } else {
            format!("no value in any of: {}", tried.join(", "))
        };
        Err(ConfigError::Secret {
            provider: provider.to_owned(),
            message,
        })
    }
}

/// Reads a JSON file into a value.
fn read_value(path: &Path) -> Result<Value, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    read_text(&text).map_err(|error| match error {
        ConfigError::Parse { source, .. } => ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        },
        other => other,
    })
}

/// Parses JSON text.
fn read_text(text: &str) -> Result<Value, ConfigError> {
    serde_json::from_str(text).map_err(|source| ConfigError::Parse {
        path: PathBuf::from("<memory>"),
        source,
    })
}

/// Deep-merges `overlay` into `base`; `overlay` wins. Objects merge, everything
/// else replaces.
fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn invalid(path: impl Into<String>, message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        path: path.into(),
        message: message.into(),
    }
}

/// The user config path: `$XDG_CONFIG_HOME/norte-code/config.json`, or the
/// platform equivalent.
#[must_use]
pub fn user_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("norte-code").join("config.json"))
}

/// The per-project overlay path under `root`.
#[must_use]
pub fn project_config_path(root: &Path) -> PathBuf {
    root.join(".norte-code").join("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn temp_file(body: &str) -> PathBuf {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("norte-code-config-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.json");
        fs::write(&path, body).expect("write");
        path
    }

    #[test]
    fn defaults_are_valid() {
        let loaded = Config::from_json("{}").expect("config");
        assert_eq!(loaded.config.version, CONFIG_VERSION);
        assert_eq!(loaded.config.max_iterations, 32);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn the_project_overlay_wins() {
        let user = temp_file(r#"{"default_model":"openai/gpt-5","max_iterations":4}"#);
        let project = temp_file(r#"{"max_iterations":8}"#);
        let loaded = Config::load(Some(&user), Some(&project)).expect("config");
        assert_eq!(loaded.config.max_iterations, 8);
        assert_eq!(loaded.config.default_model.as_deref(), Some("openai/gpt-5"));
    }

    #[test]
    fn unknown_keys_warn_instead_of_failing() {
        let loaded = Config::from_json(r#"{"mystery":true}"#).expect("config");
        assert_eq!(loaded.warnings, vec!["mystery".to_owned()]);
    }

    #[test]
    fn a_future_version_is_rejected() {
        let error = Config::from_json(r#"{"version":2}"#).expect_err("version");
        assert!(error.to_string().contains("version"));
    }

    #[test]
    fn a_bad_budget_is_rejected() {
        let error = Config::from_json(
            r#"{"budgets":{"session":{"unit":"usd","limit":0,"action":"stop"}}}"#,
        )
        .expect_err("budget");
        assert!(error.to_string().contains("budgets.session.limit"));
    }

    #[test]
    fn a_key_can_come_from_a_file() {
        let key_file = temp_file("  sk-test\n");
        let loaded = Config::from_json(&format!(
            r#"{{"providers":{{"test":{{"api_key_file":"{}"}}}}}}"#,
            key_file.display()
        ))
        .expect("config");
        let key = loaded.config.resolve_api_key("test", &[]).expect("key");
        assert_eq!(key.expose(), "sk-test");
    }

    #[test]
    fn a_missing_key_names_what_was_tried() {
        let loaded = Config::from_json("{}").expect("config");
        let error = loaded
            .config
            .resolve_api_key("nobody", &["NORTE_CODE_TEST_NOBODY_API_KEY".to_owned()])
            .expect_err("key");
        assert!(error.to_string().contains("NORTE_CODE_TEST_NOBODY_API_KEY"));
    }
}
