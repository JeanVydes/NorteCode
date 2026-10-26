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
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// MCP servers whose tools the agent may call.
    ///
    /// The block keeps the name and the shape the rest of the ecosystem uses
    /// (`mcpServers`, with `command`, `args` and `env` per entry), so a
    /// configuration written for another client works here unchanged.
    #[serde(
        default,
        rename = "mcpServers",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

/// One MCP server the agent may reach.
///
/// An entry names either a program to run, which speaks the protocol over its
/// standard input and output and is what a server published on `npm` or `PyPI`
/// expects, or a `url` to talk to over Streamable HTTP. Exactly one of the two
/// is required.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    /// The program to run, for a local server.
    pub command: String,
    /// The endpoint, for a remote server.
    ///
    /// A server that has one is reached over Streamable HTTP instead of being
    /// started, so `command` is ignored.
    pub url: Option<String>,
    /// The headers every request to a remote server carries.
    ///
    /// A token goes here as `Authorization`, by value or read from the
    /// environment; the values are registered for redaction like any other
    /// secret.
    pub headers: BTreeMap<String, String>,
    /// Its arguments, in order.
    pub args: Vec<String>,
    /// Extra environment variables for the child.
    ///
    /// A server that needs a token gets it here, by name, and the value is
    /// registered for redaction like any other secret.
    pub env: BTreeMap<String, String>,
    /// Whether the server is started. Off lets an entry be kept without
    /// paying for its process on every run.
    pub enabled: bool,
    /// The tools of this server that may reach the model.
    ///
    /// Empty means every tool the server publishes, minus the ones in `deny`.
    /// Names are written as the server names them, without the
    /// `mcp__<server>__` prefix the model sees.
    pub allow: Vec<String>,
    /// The tools of this server that never reach the model.
    ///
    /// `deny` wins over `allow`, so naming a tool in both is a way to keep it
    /// out while leaving it in a copied `allow` list.
    pub deny: Vec<String>,
}

impl McpServerConfig {
    /// How the startup line names this server.
    #[must_use]
    pub fn target(&self) -> String {
        self.url.clone().unwrap_or_else(|| self.command.clone())
    }

    /// Whether this entry reaches a remote endpoint.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        self.url.as_ref().is_some_and(|url| !url.trim().is_empty())
    }

    /// The tools of `published` this entry lets through.
    ///
    /// A tool is kept when it is not denied and either nothing is allowed or it
    /// is named in `allow`.
    #[must_use]
    pub fn select<'a, T>(&self, published: impl Iterator<Item = &'a T>) -> Vec<&'a T>
    where
        T: AsRef<str> + 'a,
    {
        published
            .filter(|tool| {
                let name = tool.as_ref();
                !self.deny.iter().any(|denied| denied == name)
                    && (self.allow.is_empty() || self.allow.iter().any(|allowed| allowed == name))
            })
            .collect()
    }
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            url: None,
            headers: BTreeMap::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: true,
            allow: Vec::new(),
            deny: Vec::new(),
        }
    }
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
            mcp_servers: BTreeMap::new(),
        }
    }
}

/// The wire a provider speaks, when the configuration overrides the
/// catalogue's `npm`-derived choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireKind {
    /// `OpenAI` Responses.
    Responses,
    /// `OpenAI` chat completions.
    Chat,
    /// Anthropic Messages.
    Messages,
    /// Google `generateContent`.
    GenerateContent,
}

/// The credential a provider's requests carry, when it is not the wire's
/// default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthKind {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `x-api-key: <key>`.
    XApiKey,
    /// `x-goog-api-key: <key>`.
    XGoogApiKey,
    /// Send no credential, for a local or otherwise open endpoint.
    Disabled,
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
    /// The wire to speak, replacing the one the catalogue's `npm` selects.
    pub wire: Option<WireKind>,
    /// The credential scheme to use, replacing the wire's default.
    pub auth: Option<AuthKind>,
    /// Whether `OpenAI` wires may send the session id as `prompt_cache_key`.
    ///
    /// `None` keeps the wire default, which sends it; `Some(false)` omits it for
    /// a gateway that rejects the field.
    pub prompt_cache_key: Option<bool>,
    /// Extra headers sent with every request to this provider.
    ///
    /// They are added to the ones the provider ships with, and a header named
    /// here replaces the built-in one of the same name.
    pub headers: BTreeMap<String, String>,
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
            max_attempts: 4,
            max_delay_ms: 60_000,
            backoff_ms: 5_000,
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
    /// How far a tool may reach.
    pub confinement: ConfinementConfig,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            web_fetch: false,
            command_max_model_bytes: DEFAULT_COMMAND_MAX_MODEL_BYTES,
            confinement: ConfinementConfig::default(),
        }
    }
}

/// How far a tool may reach.
///
/// The two halves answer the same question at different levels. The workspace
/// gate is argument-level: it re-roots a tool's path under the workspace and
/// refuses an escape, a symlink out of it, or a special file that would block
/// the tool. The sandbox is process-level and applies only to `command`: it
/// wraps the child in the platform's isolation (bubblewrap on Linux,
/// `sandbox-exec` on macOS).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfinementConfig {
    /// Whether tool paths are confined to the workspace.
    ///
    /// On by default. Turning it off lets `read`, `write`, `edit`, `glob`,
    /// `list_dir`, `grep` and `command`'s `cwd` name any path, which is what a
    /// host working across repositories wants; a special file is still refused,
    /// because opening a FIFO with no writer would block a tool forever.
    pub workspace_only: bool,
    /// How `command` is sandboxed.
    pub sandbox: SandboxConfig,
}

impl Default for ConfinementConfig {
    /// Confine paths, do not sandbox commands.
    ///
    /// The derived default would leave `workspace_only` false, which is the one
    /// value this type must not pick by accident.
    fn default() -> Self {
        Self {
            workspace_only: true,
            sandbox: SandboxConfig::default(),
        }
    }
}

impl Config {
    /// Checks every `mcpServers` entry.
    ///
    /// A server that is on must name a program, and its name becomes a segment
    /// of every tool it exposes (`mcp__<server>__<tool>`), so it may not carry
    /// the separator the namespace uses.
    fn validate_mcp_servers(&self) -> Result<(), ConfigError> {
        for (name, server) in &self.mcp_servers {
            let remote = server.is_remote();
            let local = !server.command.trim().is_empty();
            if !remote && !local {
                // A disabled entry is not started, so it may be a placeholder.
                if server.enabled {
                    return Err(invalid(
                        format!("mcpServers.{name}"),
                        "either `command` or `url` is required",
                    ));
                }
                continue;
            }
            if remote && local {
                return Err(invalid(
                    format!("mcpServers.{name}"),
                    "`command` and `url` cannot both be set: one server is local or remote",
                ));
            }
            let sane = !name.is_empty()
                && !name.contains("__")
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !sane {
                return Err(invalid(
                    format!("mcpServers.{name}"),
                    "a server name may hold letters, digits, `-` and `_`, and not `__`",
                ));
            }
        }
        Ok(())
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
    /// An input/output modality override.
    pub modalities: Option<ModalitiesOverride>,
}

/// A per-model override of the modalities the model accepts and produces.
///
/// A model the catalogue lists carries its own modalities, and a model it does
/// not — a private endpoint named only under `providers` — has nothing to read
/// and falls back to text in, text out. This is where a host that knows better
/// says so: `input` and `output` **replace** what the catalogue or the fallback
/// declared, so `{"input": ["text", "image"]}` is what lets a model the
/// catalogue does not describe take an image.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModalitiesOverride {
    /// The modalities the model accepts on input.
    pub input: Option<Vec<ModalityKind>>,
    /// The modalities the model produces on output.
    pub output: Option<Vec<ModalityKind>>,
}

/// A kind of content, as the configuration spells it.
///
/// Typed and not a `String` for the same reason [`WireKind`] is: a name the
/// build does not know is refused where it is written, by the parser, instead
/// of being dropped later and leaving a model declaring less than its host
/// asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModalityKind {
    /// Text.
    Text,
    /// Images.
    Image,
    /// Audio.
    Audio,
    /// Video.
    Video,
    /// Documents, such as PDFs. The catalogue spells this `pdf`.
    #[serde(rename = "pdf", alias = "document")]
    Pdf,
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
        self.validate_mcp_servers()?;
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
            .confinement
            .sandbox
            .writable
            .iter()
            .any(|path| path.as_os_str().is_empty())
        {
            return Err(invalid(
                "tools.confinement.sandbox.writable",
                "paths must not be empty",
            ));
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

/// Writes a config file atomically, preserving an existing symlink and mode.
///
/// A fixed temporary name would race with another Norte Code process, and a
/// truncating write would lose the file if the process died halfway. Write a
/// unique sibling with `create_new`, then rename it over the target.
fn atomic_write(path: &Path, body: &[u8]) -> Result<(), ConfigError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let target = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::canonicalize(path).map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source,
            })?
        }
        Ok(_) => path.to_path_buf(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(source) => {
            return Err(ConfigError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let existing = match fs::metadata(&target) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(ConfigError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if existing
        .as_ref()
        .is_some_and(|metadata| !metadata.is_file())
    {
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        });
    }
    let permissions = existing.map(|metadata| metadata.permissions());
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for _ in 0..16 {
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config.json");
        let tmp = parent.join(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = match options.open(&tmp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(ConfigError::Write {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let result = (|| -> std::io::Result<()> {
            let mut file = file;
            file.write_all(body)?;
            file.sync_all()?;
            if let Some(permissions) = &permissions {
                file.set_permissions(permissions.clone())?;
            }
            drop(file);
            fs::rename(&tmp, &target)
        })();
        if let Err(source) = result {
            let _ = fs::remove_file(&tmp);
            return Err(ConfigError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
        return Ok(());
    }
    Err(ConfigError::Write {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a unique temporary file",
        ),
    })
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
    atomic_write(path, body.as_bytes())
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
    fn a_provider_can_override_the_wire_and_the_credential_scheme() {
        let loaded = Config::from_json(
            r#"{
                "providers": {
                    "openrouter": {
                        "wire": "messages",
                        "auth": "bearer"
                    }
                }
            }"#,
        )
        .expect("config");
        let provider = loaded.config.providers.get("openrouter").expect("provider");
        assert_eq!(provider.wire, Some(WireKind::Messages));
        assert_eq!(provider.auth, Some(AuthKind::Bearer));
    }

    #[test]
    fn the_wire_and_auth_names_are_the_documented_ones() {
        let loaded = Config::from_json(
            r#"{
                "providers": {
                    "google": {"wire": "generate-content", "auth": "x-goog-api-key"},
                    "anthropic": {"wire": "responses", "auth": "x-api-key"},
                    "local": {"wire": "chat", "auth": "disabled"}
                }
            }"#,
        )
        .expect("config");
        assert_eq!(
            loaded.config.providers["google"].wire,
            Some(WireKind::GenerateContent)
        );
        assert_eq!(
            loaded.config.providers["google"].auth,
            Some(AuthKind::XGoogApiKey)
        );
        assert_eq!(
            loaded.config.providers["anthropic"].auth,
            Some(AuthKind::XApiKey)
        );
        assert_eq!(
            loaded.config.providers["local"].auth,
            Some(AuthKind::Disabled)
        );
    }

    #[test]
    fn an_unknown_wire_is_rejected_by_name() {
        let error = Config::from_json(r#"{"providers":{"openrouter":{"wire":"telepathy"}}}"#)
            .expect_err("unknown wire");
        assert!(error.to_string().contains("telepathy"), "{error}");
        assert!(error.to_string().contains("messages"), "{error}");
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
    fn a_model_override_declares_the_modalities_a_model_takes() {
        // The block a model the catalogue does not describe is given the
        // capabilities it has. `pdf` and `document` are the same modality, and
        // both spellings are accepted because the catalogue uses the first and
        // a person writes the second.
        let loaded = Config::from_json(
            r#"{"model_overrides":{"river/m":{"modalities":{"input":["text","image","pdf"],"output":["text"]}}}}"#,
        )
        .expect("config");
        let modalities = loaded.config.model_overrides["river/m"]
            .modalities
            .as_ref()
            .expect("modalities");
        assert_eq!(
            modalities.input.as_deref(),
            Some(&[ModalityKind::Text, ModalityKind::Image, ModalityKind::Pdf][..])
        );
        assert_eq!(
            modalities.output.as_deref(),
            Some(&[ModalityKind::Text][..])
        );

        let document = Config::from_json(
            r#"{"model_overrides":{"river/m":{"modalities":{"input":["document"]}}}}"#,
        )
        .expect("config");
        assert_eq!(
            document.config.model_overrides["river/m"]
                .modalities
                .as_ref()
                .and_then(|modalities| modalities.input.as_deref()),
            Some(&[ModalityKind::Pdf][..]),
            "`document` is the same modality as `pdf`"
        );
    }

    #[test]
    fn an_unknown_modality_is_rejected_by_name() {
        // Refused where it is written, by the parser, and not silently dropped:
        // a dropped name would leave the model declaring less than its host
        // asked for, and the image would be refused with no clue why.
        let error = Config::from_json(
            r#"{"model_overrides":{"river/m":{"modalities":{"input":["telepathy"]}}}}"#,
        )
        .expect_err("unknown modality");
        assert!(error.to_string().contains("telepathy"), "{error}");
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
        assert_eq!(loaded.config.retry.max_attempts, 4);
        assert_eq!(loaded.config.retry.max_delay_ms, 60_000);
        assert_eq!(loaded.config.retry.backoff_ms, 5_000);
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
    fn confinement_confines_paths_and_leaves_the_sandbox_off() {
        let loaded = Config::from_json("{}").expect("config");
        assert!(
            loaded.config.tools.confinement.workspace_only,
            "tool paths are confined by default"
        );
        assert!(!loaded.config.tools.confinement.sandbox.enabled);
        assert!(!loaded.config.tools.confinement.sandbox.network);
        assert!(loaded.config.tools.confinement.sandbox.writable.is_empty());

        let enabled = Config::from_json(
            r#"{"tools":{"confinement":{"sandbox":{"enabled":true,"network":true,"writable":["/tmp/cache"]}}}}"#,
        )
        .expect("config");
        assert!(enabled.config.tools.confinement.sandbox.enabled);
        assert!(enabled.config.tools.confinement.sandbox.network);
        assert_eq!(
            enabled.config.tools.confinement.sandbox.writable,
            vec![PathBuf::from("/tmp/cache")]
        );

        let open = Config::from_json(r#"{"tools":{"confinement":{"workspace_only":false}}}"#)
            .expect("config");
        assert!(
            !open.config.tools.confinement.workspace_only,
            "the gate can be turned off without touching the sandbox"
        );
        assert!(!open.config.tools.confinement.sandbox.enabled);

        let error = Config::from_json(r#"{"tools":{"confinement":{"sandbox":{"writable":[""]}}}}"#)
            .expect_err("empty path");
        assert!(
            error
                .to_string()
                .contains("tools.confinement.sandbox.writable")
        );
    }

    #[test]
    fn an_mcp_block_written_for_another_client_loads_unchanged() {
        // The shape Claude Desktop and Cursor write, copied verbatim.
        let loaded = Config::from_json(
            r#"{"mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                    "env": { "TOKEN": "secret" }
                },
                "off": { "command": "slow", "enabled": false }
            }}"#,
        )
        .expect("the ecosystem format loads");

        let filesystem = loaded.config.mcp_servers.get("filesystem").expect("entry");
        assert_eq!(filesystem.command, "npx");
        assert_eq!(filesystem.args.len(), 3);
        assert_eq!(
            filesystem.env.get("TOKEN").map(String::as_str),
            Some("secret")
        );
        assert!(filesystem.enabled, "a server without `enabled` is on");
        assert!(
            !loaded.config.mcp_servers.get("off").expect("entry").enabled,
            "a disabled entry keeps its place"
        );
    }

    #[test]
    fn a_remote_entry_is_a_url_and_not_a_command() {
        // The shape another client writes for a hosted server.
        let loaded = Config::from_json(
            r#"{"mcpServers":{"remote":{"url":"https://mcp.example.com/mcp","headers":{"Authorization":"Bearer x"}}}}"#,
        )
        .expect("a remote entry loads");
        let server = loaded.config.mcp_servers.get("remote").expect("entry");
        assert!(server.is_remote());
        assert_eq!(server.target(), "https://mcp.example.com/mcp");
        assert_eq!(
            server.headers.get("Authorization").map(String::as_str),
            Some("Bearer x")
        );

        // A local entry is labelled by its program.
        let local = Config::from_json(r#"{"mcpServers":{"local":{"command":"npx"}}}"#)
            .expect("a local entry loads")
            .config;
        let server = local.mcp_servers.get("local").expect("entry");
        assert!(!server.is_remote());
        assert_eq!(server.target(), "npx");

        // A url on its own is a remote server, which is the point of the form.
        Config::from_json(r#"{"mcpServers":{"x":{"url":"https://a"}}}"#)
            .expect("a url alone is a remote entry");

        // Nothing at all is a server that cannot be reached; both is ambiguous.
        let error =
            Config::from_json(r#"{"mcpServers":{"x":{}}}"#).expect_err("an empty entry is refused");
        assert!(error.to_string().contains("either"), "{error}");
        let error =
            Config::from_json(r#"{"mcpServers":{"x":{"command":"npx","url":"https://a"}}}"#)
                .expect_err("both is ambiguous");
        assert!(error.to_string().contains("cannot both"), "{error}");
    }

    #[test]
    fn the_tool_lists_decide_what_reaches_the_model() {
        let published = ["echo", "get-sum", "get-env"];

        // Nothing named: every tool the server publishes.
        let open = McpServerConfig::default();
        assert_eq!(open.select(published.iter()).len(), 3);

        // An allow list is the whole of it.
        let only = McpServerConfig {
            allow: vec!["echo".to_owned(), "get-sum".to_owned()],
            ..McpServerConfig::default()
        };
        let kept = only.select(published.iter());
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept.iter().any(|tool| **tool == "echo"), "{kept:?}");
        assert!(
            !kept.iter().any(|tool| **tool == "get-env"),
            "a tool outside the allow list is absent: {kept:?}"
        );

        // A deny list takes away from everything else.
        let without = McpServerConfig {
            deny: vec!["get-env".to_owned()],
            ..McpServerConfig::default()
        };
        let kept = without.select(published.iter());
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(!kept.iter().any(|tool| **tool == "get-env"), "{kept:?}");

        // Deny wins over allow, so naming a tool in both keeps it out.
        let both = McpServerConfig {
            allow: vec!["echo".to_owned()],
            deny: vec!["echo".to_owned()],
            ..McpServerConfig::default()
        };
        assert!(both.select(published.iter()).is_empty());
    }

    #[test]
    fn mcp_entries_are_validated() {
        // A server that is on must name a program to run.
        let error = Config::from_json(r#"{"mcpServers":{"x":{"command":"  "}}}"#)
            .expect_err("an empty command is refused");
        assert!(error.to_string().contains("mcpServers.x"), "{error}");
        assert!(error.to_string().contains("command"), "{error}");

        // A disabled entry may be incomplete: it is off, so it is not started.
        Config::from_json(r#"{"mcpServers":{"x":{"command":"","enabled":false}}}"#)
            .expect("a disabled placeholder is fine");

        // The name becomes part of every tool name it exposes.
        let error = Config::from_json(r#"{"mcpServers":{"a__b":{"command":"npx"}}}"#)
            .expect_err("the namespace separator is refused");
        assert!(error.to_string().contains("a__b"), "{error}");
        let error = Config::from_json(r#"{"mcpServers":{"a b":{"command":"npx"}}}"#)
            .expect_err("a space is refused");
        assert!(error.to_string().contains("a b"), "{error}");
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
        let leftovers = fs::read_dir(path.parent().expect("parent"))
            .expect("directory")
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_stays_a_symlink() {
        use std::os::unix::fs::symlink;

        let target = temp_file("{}");
        let directory = target.parent().expect("parent").join("link");
        fs::create_dir_all(&directory).expect("directory");
        let link = directory.join("config.json");
        symlink(&target, &link).expect("symlink");
        update_user_approval(&link, "command", true).expect("update");
        assert!(
            fs::symlink_metadata(&link)
                .expect("metadata")
                .file_type()
                .is_symlink()
        );
        let body = fs::read_to_string(&target).expect("target");
        assert!(body.contains("\"command\""));
        let _ = fs::remove_dir_all(target.parent().expect("parent"));
    }

    #[cfg(unix)]
    #[test]
    fn a_new_config_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_file("{}");
        fs::remove_file(&path).expect("remove");
        update_user_approval(&path, "command", true).expect("update");
        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
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
