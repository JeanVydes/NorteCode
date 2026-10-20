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
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only config schema revision this build understands.
pub const CONFIG_VERSION: u32 = 1;
/// Largest config file accepted.
const MAX_CONFIG_BYTES: usize = 1024 * 1024;
/// Largest API key file accepted.
const MAX_SECRET_BYTES: usize = 64 * 1024;

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
    /// Automatic retries for transient provider failures.
    pub retry: RetryConfig,
    /// The session's default reasoning setting.
    pub reasoning: ReasoningConfig,
    /// A system prompt override; `None` uses the built-in default.
    pub system_prompt: Option<String>,
    /// How tool calls are approved.
    pub approval_mode: ApprovalMode,
    /// Tools always allowed or denied across sessions.
    pub approvals: ApprovalsConfig,
    /// How many model iterations one turn may run, or `None` for no limit.
    pub max_iterations: Option<usize>,
    /// Vendor request extras, keyed `<wire-family>:<dotted.path>`.
    ///
    /// For example, `OpenRouter`'s upstream routing is
    /// `openai.chat:provider` with `{"order": ["deepinfra"]}`. Norte ignores a
    /// key whose family does not match the active wire.
    pub extensions: BTreeMap<String, Value>,
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
            retry: RetryConfig::default(),
            reasoning: ReasoningConfig::default(),
            system_prompt: None,
            approval_mode: ApprovalMode::default(),
            approvals: ApprovalsConfig::default(),
            max_iterations: None,
            extensions: BTreeMap::new(),
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
    /// When an automatic compaction may run: `idle` (the default) only at an
    /// idle turn boundary, or `eager` at the first safe model-request boundary
    /// inside a turn. Eager still never splits a tool call from its results.
    pub trigger: String,
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
            trigger: "idle".to_owned(),
            keep_recent: 6,
            model: None,
            reasoning: "lowest".to_owned(),
        }
    }
}

/// Automatic retries for transient provider failures.
///
/// The host retries by resuming the turn on the same session, so the user's
/// message is never sent twice. Only failures that can clear on their own are
/// retried: rate limits, overload, server errors and transport errors. A quota
/// or authentication failure is permanent and fails the turn immediately.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// How many times to retry after the first failure. `0` disables retries.
    pub max_attempts: u32,
    /// The longest wait the host will accept, in milliseconds.
    pub max_delay_ms: u64,
    /// The first fallback wait when the provider gives no `retry_after`, in
    /// milliseconds. It doubles on each attempt, up to `max_delay_ms`.
    pub backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 2,
            max_delay_ms: 30_000,
            backoff_ms: 1_000,
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

/// Approval rules that outlive one session.
///
/// A tool named in `deny` is refused before the approval mode is consulted; a
/// tool named in `allow` runs without asking. Denial wins when a tool is in
/// both lists. These are deliberately tool-wide: use the session prompt for a
/// narrower decision.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalsConfig {
    /// Tool names always allowed across sessions.
    pub allow: Vec<String>,
    /// Tool names always denied across sessions.
    pub deny: Vec<String>,
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

/// Default cap on the bytes of each command stream that reach the model.
pub const DEFAULT_COMMAND_MAX_MODEL_BYTES: usize = 128 * 1024;

/// Tool switches.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Whether the `web_fetch` tool is registered.
    pub web_fetch: bool,
    /// Bytes of each `command` stream and summary that reach the model.
    ///
    /// The tool still captures up to its own larger ceiling; this bounds only
    /// the text the model sees, eliding the middle with a marker.
    pub command_max_model_bytes: usize,
    /// How `command` is sandboxed.
    pub sandbox: SandboxConfig,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            web_fetch: false,
            command_max_model_bytes: DEFAULT_COMMAND_MAX_MODEL_BYTES,
            sandbox: SandboxConfig::default(),
        }
    }
}

/// Command sandbox settings.
///
/// Disabled by default. When enabled, `command` runs through bubblewrap on
/// Linux or `sandbox-exec` on macOS: the workspace is writable, the rest of the
/// filesystem is read-only, and the network is unavailable unless explicitly
/// allowed. If the platform sandbox binary is missing, the command fails rather
/// than running unsandboxed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Whether `command` runs inside the platform sandbox.
    pub enabled: bool,
    /// Whether the sandboxed command may use the network.
    pub network: bool,
    /// Extra directories writable inside the sandbox, in addition to the
    /// workspace. Relative paths are resolved against the workspace; paths that
    /// do not exist are ignored.
    pub writable: Vec<PathBuf>,
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
    /// A file could not be written.
    #[error("could not write {path}: {source}")]
    Write {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
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
        if self.max_iterations == Some(0) {
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
        if !matches!(self.compaction.trigger.as_str(), "idle" | "eager") {
            return Err(invalid("compaction.trigger", "must be `idle` or `eager`"));
        }
        if self.retry.max_attempts > 10 {
            return Err(invalid(
                "retry.max_attempts",
                "must be at most 10; retries are bounded on purpose",
            ));
        }
        if self.retry.max_delay_ms > 600_000 {
            return Err(invalid(
                "retry.max_delay_ms",
                "must be at most 600000 (ten minutes)",
            ));
        }
        if self.retry.backoff_ms > 600_000 {
            return Err(invalid(
                "retry.backoff_ms",
                "must be at most 600000 (ten minutes)",
            ));
        }
        if self
            .tools
            .sandbox
            .writable
            .iter()
            .any(|path| path.as_os_str().is_empty())
        {
            return Err(invalid("tools.sandbox.writable", "paths must not be empty"));
        }
        if !(1024..=1024 * 1024).contains(&self.tools.command_max_model_bytes) {
            return Err(invalid(
                "tools.command_max_model_bytes",
                "must be between 1024 and 1048576 bytes",
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
            let text =
                read_limited(path, MAX_SECRET_BYTES).map_err(|source| ConfigError::Secret {
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

/// Reads a regular file, refusing a special file and a size over `limit`.
fn read_limited(path: &Path, limit: usize) -> std::io::Result<String> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let file = fs::File::open(path)?;
    let mut text = String::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_string(&mut text)?;
    if text.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file is larger than {limit} bytes"),
        ));
    }
    Ok(text)
}

/// Reads a JSON file into a value.
fn read_value(path: &Path) -> Result<Value, ConfigError> {
    let text = read_limited(path, MAX_CONFIG_BYTES).map_err(|source| ConfigError::Read {
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

/// Adds one global approval rule to the user config, preserving the rest of
/// the file.
///
/// A tool moves between `approvals.allow` and `approvals.deny`; the opposite
/// list is cleared for that tool so the file never says both. Unknown keys and
/// the existing formatting-independent JSON content are kept.
///
/// # Errors
///
/// Returns [`ConfigError`] when the file cannot be read or written, is not a
/// JSON object, or has an `approvals` field of the wrong shape.
pub fn update_user_approval(path: &Path, tool: &str, allow: bool) -> Result<(), ConfigError> {
    let tool = tool.trim();
    if tool.is_empty() {
        return Err(invalid("approvals", "the tool name is empty"));
    }
    let mut root = if path.exists() {
        read_value(path)?
    } else {
        Value::Object(serde_json::Map::new())
    };
    let root = root
        .as_object_mut()
        .ok_or_else(|| invalid("approvals", "the config root must be a JSON object"))?;
    let approvals = root
        .entry("approvals")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let approvals = approvals
        .as_object_mut()
        .ok_or_else(|| invalid("approvals", "must be a JSON object"))?;
    let (keep, drop) = if allow {
        ("allow", "deny")
    } else {
        ("deny", "allow")
    };
    let list = approvals
        .entry(keep)
        .or_insert_with(|| Value::Array(Vec::new()));
    let list = list.as_array_mut().ok_or_else(|| {
        invalid(
            format!("approvals.{keep}"),
            "must be an array of tool names",
        )
    })?;
    if !list.iter().any(|entry| entry.as_str() == Some(tool)) {
        list.push(Value::String(tool.to_owned()));
    }
    if let Some(other) = approvals.get_mut(drop).and_then(Value::as_array_mut) {
        other.retain(|entry| entry.as_str() != Some(tool));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let mut body = serde_json::to_string_pretty(&root).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    body.push('\n');
    fs::write(path, body).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
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
        assert_eq!(loaded.config.max_iterations, None);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn a_file_over_the_limit_is_refused_before_parsing() {
        let path = temp_file("0123456789");
        let error = read_limited(&path, 4).expect_err("file must be bounded");
        assert!(error.to_string().contains("larger than 4 bytes"));
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_directory_is_not_a_regular_file() {
        let path = temp_file("{}");
        let error = read_limited(path.parent().expect("parent"), 64).expect_err("directory");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_system_prompt_can_come_from_the_config() {
        let loaded = Config::from_json(r#"{"system_prompt":"Answer tersely."}"#).expect("config");
        assert_eq!(
            loaded.config.system_prompt.as_deref(),
            Some("Answer tersely.")
        );
        let defaults = Config::from_json("{}").expect("config");
        assert_eq!(defaults.config.system_prompt, None);
    }

    #[test]
    fn vendor_extensions_are_typed_json() {
        let loaded =
            Config::from_json(r#"{"extensions":{"openai.chat:provider":{"order":["deepinfra"]}}}"#)
                .expect("config");
        assert_eq!(
            loaded.config.extensions["openai.chat:provider"],
            serde_json::json!({"order": ["deepinfra"]})
        );
    }

    #[test]
    fn the_project_overlay_wins() {
        let user = temp_file(r#"{"default_model":"openai/gpt-5","max_iterations":4}"#);
        let project = temp_file(r#"{"max_iterations":8}"#);
        let loaded = Config::load(Some(&user), Some(&project)).expect("config");
        assert_eq!(loaded.config.max_iterations, Some(8));
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
    fn compaction_trigger_defaults_to_idle_and_rejects_unknown_modes() {
        let loaded = Config::from_json("{}").expect("config");
        assert_eq!(loaded.config.compaction.trigger, "idle");
        let eager = Config::from_json(r#"{"compaction":{"trigger":"eager"}}"#).expect("config");
        assert_eq!(eager.config.compaction.trigger, "eager");
        let error =
            Config::from_json(r#"{"compaction":{"trigger":"always"}}"#).expect_err("trigger");
        assert!(error.to_string().contains("compaction.trigger"));
    }

    #[test]
    fn retry_defaults_are_bounded_and_capped_by_validation() {
        let loaded = Config::from_json("{}").expect("config");
        assert_eq!(loaded.config.retry.max_attempts, 2);
        assert_eq!(loaded.config.retry.max_delay_ms, 30_000);
        assert_eq!(loaded.config.retry.backoff_ms, 1_000);
        let zero = Config::from_json(r#"{"retry":{"max_attempts":0}}"#).expect("config");
        assert_eq!(zero.config.retry.max_attempts, 0);
        let error =
            Config::from_json(r#"{"retry":{"max_attempts":11}}"#).expect_err("max_attempts");
        assert!(error.to_string().contains("retry.max_attempts"));
        let error =
            Config::from_json(r#"{"retry":{"max_delay_ms":600001}}"#).expect_err("max_delay_ms");
        assert!(error.to_string().contains("retry.max_delay_ms"));
    }

    #[test]
    fn command_model_bytes_default_and_validation() {
        let loaded = Config::from_json("{}").expect("config");
        assert_eq!(
            loaded.config.tools.command_max_model_bytes,
            DEFAULT_COMMAND_MAX_MODEL_BYTES
        );
        let custom = Config::from_json(r#"{"tools":{"command_max_model_bytes":65536}}"#)
            .expect("custom limit");
        assert_eq!(custom.config.tools.command_max_model_bytes, 65_536);
        for value in ["0", "1023", "1048577"] {
            let error = Config::from_json(&format!(
                r#"{{"tools":{{"command_max_model_bytes":{value}}}}}"#
            ))
            .expect_err("out of range");
            assert!(error.to_string().contains("tools.command_max_model_bytes"));
        }
    }

    #[test]
    fn sandbox_is_off_without_network_and_accepts_extra_writable_paths() {
        let loaded = Config::from_json("{}").expect("config");
        assert!(!loaded.config.tools.sandbox.enabled);
        assert!(!loaded.config.tools.sandbox.network);
        assert!(loaded.config.tools.sandbox.writable.is_empty());
        let enabled = Config::from_json(
            r#"{"tools":{"sandbox":{"enabled":true,"network":true,"writable":["/tmp/cache"]}}}"#,
        )
        .expect("config");
        assert!(enabled.config.tools.sandbox.enabled);
        assert!(enabled.config.tools.sandbox.network);
        assert_eq!(
            enabled.config.tools.sandbox.writable,
            vec![PathBuf::from("/tmp/cache")]
        );
        let error = Config::from_json(r#"{"tools":{"sandbox":{"writable":[""]}}}"#)
            .expect_err("empty path");
        assert!(error.to_string().contains("tools.sandbox.writable"));
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
    fn a_global_approval_rule_moves_between_lists_and_keeps_unknown_keys() {
        let path = temp_file(r#"{"mystery":true}"#);
        update_user_approval(&path, "command", true).expect("allow");
        update_user_approval(&path, "command", true).expect("allow again");
        let body = fs::read_to_string(&path).expect("read");
        let root: Value = serde_json::from_str(&body).expect("json");
        assert_eq!(root["approvals"]["allow"], serde_json::json!(["command"]));
        assert_eq!(root["mystery"], true);

        update_user_approval(&path, "command", false).expect("deny");
        let loaded = Config::load(Some(&path), None).expect("config");
        assert!(loaded.config.approvals.allow.is_empty());
        assert_eq!(loaded.config.approvals.deny, ["command"]);
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
