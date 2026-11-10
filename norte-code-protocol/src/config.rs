//! Norte Code configuration.
//!
//! Loads, validates and merges `service.json`: a user file plus an optional
//! per-project overlay, where the project wins. Every key has a documented
//! default, and a validation error names the field that is wrong.
//!
//! Secrets are not stored in `service.json`. A provider names the environment
//! variables it accepts (the catalogue's `environment_keys`); a host may install
//! a [`KeyStore`](crate::keys::KeyStore) to add the OS keyring and the 0600
//! `secrets.json` fallback. The key is resolved on demand into an [`ApiKey`],
//! whose `Debug` output is redacted.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::keys::KeyStore;
use norte_code_wire::{BudgetAction, BudgetUnit};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only config schema revision this build understands.
pub const CONFIG_VERSION: u32 = 1;
/// Largest config file accepted.
const MAX_CONFIG_BYTES: usize = 1024 * 1024;

/// One server-owned agent definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Agent {
    /// Schema revision. Must be [`CONFIG_VERSION`].
    pub version: u32,
    /// Stable identifier used by RAEP attachment.
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Optional URL of a remote JSON definition for this agent.
    pub origin_url: Option<String>,
    /// Person or organization that publishes the agent.
    pub author: Option<String>,
    /// Agent a delegation uses when the request names none.
    ///
    /// `None` keeps the parent session's profile. When set, the name must
    /// refer to another configured agent; that agent supplies the child
    /// defaults, and it must also appear in `delegates`.
    #[serde(alias = "subagent")]
    pub default_subagent_delegation: Option<String>,
    /// Agent ids this profile may delegate to.
    ///
    /// An empty list disables delegation. `default_subagent_delegation`, when
    /// set, is the default target and must also appear here.
    pub delegates: Vec<String>,
    /// The provider a new session uses when none is named.
    ///
    /// It is separate from the model because a model name is a string and may
    /// carry slashes of its own — NVIDIA names its models `vendor/model`, and
    /// several OpenAI-compatible vendors do the same — so splitting a
    /// `provider/model` reference to find the two is wrong the moment a model
    /// name has one.
    pub default_provider: Option<String>,
    /// The model a new session uses when none is named.
    ///
    /// It is the model id and nothing else, slashes and all.
    pub default_model: Option<String>,
    /// Automatic compaction.
    pub compaction: CompactionConfig,
    /// Automatic retries for transient provider failures.
    pub retry: RetryConfig,
    /// The session's default reasoning setting.
    pub reasoning: ReasoningConfig,
    /// A system prompt override; `None` uses the built-in default.
    pub system_prompt: Option<String>,
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
    /// Client-only interface settings.
    ///
    /// This field is kept in the in-memory aggregate for compatibility with
    /// callers that build a `Config`, but it is never read from or written to
    /// `service.json`. The CLI loads its settings from `cli.json` through [`CliConfig::load`].
    #[serde(skip)]
    pub ui: CliConfig,
    /// Display settings for cost.
    pub cost: CostConfig,
    /// Optional spending limits.
    pub budgets: BudgetsConfig,
    /// Tool switches.
    pub tools: ToolsConfig,
    /// Agent Skills settings.
    pub skills: SkillsConfig,
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
    /// The host's provider-key source.
    ///
    /// It is installed after loading — the CLI installs the OS keyring and the
    /// 0600 fallback — and is never serialized, so a key cannot reach a config
    /// file through it. `None` resolves environment variables only.
    #[serde(skip)]
    pub keys: Option<Arc<dyn KeyStore>>,
}

/// The selected agent configuration used by the engine.
pub type Config = Agent;

/// Server configuration containing the available agents.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceConfig {
    /// Schema revision. Must be [`CONFIG_VERSION`].
    pub version: u32,
    /// Agent selected for new RAEP attachments.
    pub default_agent: String,
    /// Agents exposed by the service.
    pub agents: Vec<Agent>,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            default_agent: "norte-code".to_owned(),
            agents: vec![Agent::default()],
        }
    }
}

impl ServiceConfig {
    /// Returns the configured default agent.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the service has no valid default agent.
    pub fn default_agent(&self) -> Result<Agent, ConfigError> {
        self.agents
            .iter()
            .find(|agent| agent.name == self.default_agent)
            .cloned()
            .ok_or_else(|| {
                invalid(
                    "default_agent",
                    format!("does not name an agent: {}", self.default_agent),
                )
            })
    }

    /// Loads and validates a server configuration from user and project files.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when a file is unreadable, malformed or invalid.
    pub fn load(user: Option<&Path>, project: Option<&Path>) -> Result<Self, ConfigError> {
        let mut value = Value::Object(serde_json::Map::new());
        if let Some(path) = user
            && path.exists()
        {
            merge(&mut value, read_value(path)?);
        }
        if let Some(path) = project
            && path.exists()
        {
            merge(&mut value, read_value(path)?);
        }
        let service: ServiceConfig = if value.get("agents").is_none() {
            let agent: Agent =
                serde_json::from_value(value).map_err(|source| ConfigError::Parse {
                    path: PathBuf::from("<service.json>"),
                    source,
                })?;
            Self {
                version: agent.version,
                default_agent: agent.name.clone(),
                agents: vec![agent],
            }
        } else {
            serde_json::from_value(value).map_err(|source| ConfigError::Parse {
                path: PathBuf::from("<service.json>"),
                source,
            })?
        };
        service.validate()?;
        Ok(service)
    }

    /// Validates the service envelope and every agent.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] naming the first invalid field.
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
        if self.agents.is_empty() {
            return Err(invalid("agents", "must contain at least one agent"));
        }
        let mut names = std::collections::BTreeSet::new();
        for agent in &self.agents {
            agent.validate()?;
            if !names.insert(agent.name.clone()) {
                return Err(invalid(
                    "agents.name",
                    format!("duplicate agent name `{}`", agent.name),
                ));
            }
        }
        for agent in &self.agents {
            let mut delegates = std::collections::BTreeSet::new();
            for delegate in &agent.delegates {
                if !names.contains(delegate) {
                    return Err(invalid(
                        format!("agents.{}.delegates", agent.name),
                        format!("does not name a configured agent: {delegate}"),
                    ));
                }
                if !delegates.insert(delegate) {
                    return Err(invalid(
                        format!("agents.{}.delegates", agent.name),
                        format!("duplicate delegate `{delegate}`"),
                    ));
                }
            }
            if let Some(subagent) = &agent.default_subagent_delegation
                && !names.contains(subagent)
            {
                return Err(invalid(
                    format!("agents.{}.default_subagent_delegation", agent.name),
                    format!("does not name a configured agent: {subagent}"),
                ));
            }
            if let Some(subagent) = &agent.default_subagent_delegation
                && !delegates.contains(subagent)
            {
                return Err(invalid(
                    format!("agents.{}.default_subagent_delegation", agent.name),
                    format!("default subagent `{subagent}` must also appear in delegates"),
                ));
            }
        }
        self.default_agent()?;
        Ok(())
    }
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

impl Default for Agent {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            name: "norte-code".to_owned(),
            description: None,
            origin_url: None,
            author: None,
            default_subagent_delegation: None,
            delegates: Vec::new(),
            default_provider: None,
            default_model: None,
            compaction: CompactionConfig::default(),
            retry: RetryConfig::default(),
            reasoning: ReasoningConfig::default(),
            system_prompt: None,
            approvals: ApprovalsConfig::default(),
            max_iterations: None,
            extensions: BTreeMap::new(),
            ui: CliConfig::default(),
            cost: CostConfig::default(),
            budgets: BudgetsConfig::default(),
            tools: ToolsConfig::default(),
            skills: SkillsConfig::default(),
            mcp_servers: BTreeMap::new(),
            keys: None,
        }
    }
}

/// When an automatic compaction may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    /// Only at an idle turn boundary (and before the next turn on resume or
    /// switch to a smaller model).
    #[default]
    Idle,
    /// At the first safe model-request boundary inside a turn.
    Eager,
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
    pub trigger: CompactionTrigger,
    /// How many recent messages to keep verbatim.
    pub keep_recent: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.90,
            trigger: CompactionTrigger::default(),
            keep_recent: 6,
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

/// Approval rules that outlive one session.
///
/// A tool named in `deny` is refused, while a tool named in `allow` runs.
/// Denial wins when a tool is in both lists. These are deliberately tool-wide:
/// use `/allow` and `/deny` for session-scoped rules.
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

/// How the client shows what it does.
///
/// A subset of the configuration that only the interface reads: the engine
/// never consults it, and a host with no interface — a script driving the
/// service — is free to ignore the whole block.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CliConfig {
    /// The service endpoint used by the CLI.
    pub service_url: String,
    /// Whether lifecycle commands may manage the local service process.
    pub service_type: ServiceType,
    /// Optional raw bearer token for the service.
    pub service_auth: Option<String>,
    /// Agent identifier sent when attaching to the service.
    pub agent_id: String,
    /// How much of the conversation to show: `low` (the answer alone), `medium`
    /// (also that tools run and that the model is thinking) or `high` (the
    /// reasoning, what each tool was asked and what it returned).
    ///
    /// `--verbose` overrides it for one run. It is a string rather than an enum
    /// so a value the client does not know is reported beside the ones it does,
    /// instead of failing the whole file.
    pub verbosity: String,
    /// What happens to a message typed while a turn is running: `reject`,
    /// `queue` or `interrupt`. An unknown value is reported beside the ones
    /// the client knows instead of failing the whole file, so the
    /// renderer can name them.
    pub queue_mode: String,
    /// How an `edit` call's diff is drawn.
    pub diff_style: DiffStyle,
    /// Configuration shared by the terminal's halftone charts.
    pub charts: ChartConfig,
}

/// The service type configured for the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    /// The service is managed as a local process by lifecycle commands.
    #[default]
    Local,
    /// The service is remote and cannot be started or stopped by this CLI.
    Remote,
}

/// The result of loading the client-only `cli.json` file.
#[derive(Clone, Debug)]
pub struct LoadedCli {
    /// The merged interface settings.
    pub config: CliConfig,
    /// Unknown keys, as dotted paths, to report to the client.
    pub warnings: Vec<String>,
}

impl CliConfig {
    /// Loads client settings from a user file and an optional
    /// project overlay. The project file wins for every key it sets.
    ///
    /// Unlike [`Config::load`], this function never reads `service.json`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when a file cannot be read or is not valid JSON.
    pub fn load(user: Option<&Path>, project: Option<&Path>) -> Result<LoadedCli, ConfigError> {
        let mut value = Value::Object(serde_json::Map::new());
        if let Some(path) = user
            && path.exists()
        {
            merge(&mut value, read_value(path)?);
        }
        if let Some(path) = project
            && path.exists()
        {
            merge(&mut value, read_value(path)?);
        }
        let mut warnings = Vec::new();
        let config: CliConfig = serde_ignored::deserialize(value, |path| {
            warnings.push(path.to_string());
        })
        .map_err(|source| ConfigError::Parse {
            path: PathBuf::from("<cli.json>"),
            source,
        })?;
        Ok(LoadedCli { config, warnings })
    }

    /// Parses client-only interface settings from JSON.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the JSON is invalid or does not match the
    /// interface schema.
    pub fn from_json(text: &str) -> Result<LoadedCli, ConfigError> {
        let value = read_text(text)?;
        let mut warnings = Vec::new();
        let config: CliConfig = serde_ignored::deserialize(value, |path| {
            warnings.push(path.to_string());
        })
        .map_err(|source| ConfigError::Parse {
            path: PathBuf::from("<cli.json>"),
            source,
        })?;
        Ok(LoadedCli { config, warnings })
    }
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            service_url: "http://127.0.0.1:8787".to_owned(),
            service_type: ServiceType::Local,
            service_auth: None,
            agent_id: "norte-code".to_owned(),
            verbosity: "medium".to_owned(),
            queue_mode: "queue".to_owned(),
            diff_style: DiffStyle::default(),
            charts: ChartConfig::default(),
        }
    }
}

/// Visual settings shared by chart widgets.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartConfig {
    /// Contrast of the halftone ramp, from 0 to 100.
    pub contrast: u8,
    /// Probability that a candidate dot is drawn, from 0 to 100.
    pub density: u8,
    /// Character scale used for halftone dots.
    pub dot_size: ChartDotSize,
}

impl Default for ChartConfig {
    fn default() -> Self {
        Self {
            contrast: 75,
            density: 82,
            dot_size: ChartDotSize::Medium,
        }
    }
}

/// Character scale for halftone chart marks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartDotSize {
    /// Fine single-cell dots.
    Small,
    /// Balanced dots for normal terminal sizes.
    #[default]
    Medium,
    /// Dense dots with stronger visual weight.
    Large,
}

/// How the diff of an `edit` call is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffStyle {
    /// Removed and added lines interleaved, the way a patch reads: one column,
    /// which is what fits a narrow terminal.
    #[default]
    Unified,
    /// The file before and after, side by side. Wider, and the only shape that
    /// shows a change inside a line rather than the two whole lines.
    Split,
}

impl DiffStyle {
    /// The name the configuration uses.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unified => "unified",
            Self::Split => "split",
        }
    }
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
    pub unit: BudgetUnit,
    /// The limit.
    pub limit: f64,
    /// `warn`, `ask` or `stop`.
    pub action: BudgetAction,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            unit: BudgetUnit::Usd,
            limit: 0.0,
            action: BudgetAction::Warn,
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
    /// How a browser screenshot reaches the model.
    pub screenshots: ScreenshotConfig,
    /// Whether a finished job starts a turn by itself.
    ///
    /// A detached shell or subagent reports its result through steering, which
    /// a running turn sees at its next safe point — the same way a message
    /// the user sends mid-turn is queued and not waited for. When no turn is
    /// running, the report sits in the queue until the next one. With this on,
    /// the service starts a turn of its own the moment a job ends, so the
    /// model reads the result right away instead of when the next user
    /// message happens to arrive. Off by default: it spends tokens without
    /// being asked.
    pub auto_turn_on_job_end: bool,
    /// Tools a new session does not get, by the name the model would call them.
    ///
    /// A tool named here is never registered: it is not in the schema the model
    /// is given, not in the request, and not in the registry a call is looked up
    /// in. That is the point of doing it at registration rather than at call
    /// time — a tool the model can see is a tool it will try, and a refusal
    /// after the fact costs a round trip and teaches it nothing.
    ///
    /// It bites when a session is *created*, and only then. The declarations
    /// are part of the prompt prefix, so a set that changed under a running
    /// session would invalidate the provider's cached prefix and re-bill the
    /// whole conversation. A session made while a tool was enabled keeps it for
    /// its whole life, and editing this list does not reach back into one that
    /// already exists.
    pub disabled: Vec<String>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            web_fetch: false,
            command_max_model_bytes: DEFAULT_COMMAND_MAX_MODEL_BYTES,
            confinement: ConfinementConfig::default(),
            screenshots: ScreenshotConfig::default(),
            auto_turn_on_job_end: false,
            disabled: Vec::new(),
        }
    }
}

/// Longest side, in pixels, a promoted screenshot keeps.
///
/// The number is the one the vendors' own guidance converges on for a legible
/// image: a page captured at a larger size is downscaled rather than refused,
/// because the text on it is what the model is being asked to read.
const DEFAULT_SCREENSHOT_MAX_DIMENSION: u32 = 1568;

/// The largest a promoted screenshot may be after re-encoding, in bytes.
const DEFAULT_SCREENSHOT_MAX_BYTES: u64 = 4_000_000;

/// How a browser screenshot reaches the model.
///
/// `browser_use` answers with its capture as base64 inside the tool result, and
/// a tool result carries text only on the chat completions wire, so the image
/// never reached the model: it was told how many bytes there were and nothing
/// else. The host promotes the capture into a user message, which carries an
/// image on every wire — this is what bounds, and what can turn off, that
/// promotion.
///
/// Turning it off leaves the tool's answer exactly as it is: the model is still
/// told that a screenshot was taken and how big it is, and never sees it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenshotConfig {
    /// Whether a capture becomes an image the model can look at.
    pub enabled: bool,
    /// Longest side, in pixels, a capture keeps. A larger one is downscaled.
    pub max_dimension: u32,
    /// The largest a capture may be after re-encoding, in bytes.
    pub max_bytes: u64,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_dimension: DEFAULT_SCREENSHOT_MAX_DIMENSION,
            max_bytes: DEFAULT_SCREENSHOT_MAX_BYTES,
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

impl Agent {
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

impl Agent {
    /// Loads and validates a configuration from a user file and an optional
    /// project overlay. The project wins for every key it sets.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when a file cannot be read, is not valid JSON or
    /// fails validation.
    pub fn load(user: Option<&Path>, project: Option<&Path>) -> Result<Loaded, ConfigError> {
        let mut value = Value::Object(serde_json::Map::new());
        if let Some(path) = user
            && path.exists()
        {
            merge(&mut value, read_value(path)?);
        }
        if let Some(path) = project
            && path.exists()
        {
            merge(&mut value, read_value(path)?);
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

    /// Checks every provider and model that are written together.
    ///
    /// Both or neither: a model with no provider cannot be resolved, and a
    /// provider with no model names half a session. Neither is checked for a
    /// slash — a model name is a string that may carry one, and the provider is
    /// its own key.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when exactly one of a pair is set, or
    /// when a provider is blank.
    fn validate_model_pairs(&self) -> Result<(), ConfigError> {
        validate_pair(
            "default_provider",
            "default_model",
            self.default_provider.as_deref(),
            self.default_model.as_deref(),
        )?;
        Ok(())
    }

    /// Checks the invariants the loader cannot express.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] naming the first field that is wrong.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.name.trim().is_empty() {
            return Err(invalid("name", "must not be empty"));
        }
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
        self.validate_model_pairs()?;
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
            if let Some(budget) = budget
                && budget.limit <= 0.0
            {
                return Err(invalid(
                    format!("budgets.{scope}.limit"),
                    "must be greater than zero",
                ));
            }
        }
        Ok(())
    }

    /// Resolves a provider's API key.
    ///
    /// The names come from the catalogue's provider entry (`environment_keys`),
    /// in the order it lists them, and the first one that is set and not empty
    /// wins. When the host installed a [`KeyStore`], it continues from there:
    /// its cache, the OS keyring and the 0600 secrets file. Without one, the
    /// environment is the only source.
    ///
    /// A provider that names no variable is not an error here — it is a
    /// provider that needs no credential, which is what a local endpoint is.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Secret`] when no source holds a value.
    pub fn resolve_api_key(
        &self,
        provider: &str,
        environment_keys: &[String],
    ) -> Result<ApiKey, ConfigError> {
        match &self.keys {
            Some(keys) => keys.resolve(provider, environment_keys),
            None => crate::keys::environment_key(provider, environment_keys),
        }
    }

    /// The key already known without asking the OS keyring.
    ///
    /// This is what redaction uses. A host's store contributes its environment,
    /// cache and 0600-file keys; the OS keyring is deliberately not queried, so
    /// a startup pass does not unlock a keyring once per provider.
    #[must_use]
    pub fn known_api_key(&self, provider: &str, environment_keys: &[String]) -> Option<ApiKey> {
        match &self.keys {
            Some(keys) => keys.known(provider, environment_keys),
            None => crate::keys::environment_value(environment_keys),
        }
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

/// Checks a provider and a model that are written together.
///
/// Both or neither: a model with no provider cannot be resolved, and a provider
/// with no model names half a session. Neither is checked for a slash — a model
/// id is a string that may carry one, and the provider is its own key.
fn validate_pair(
    provider_path: &str,
    model_path: &str,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<(), ConfigError> {
    match (provider, model) {
        (Some(provider), Some(_)) if provider.trim().is_empty() => {
            Err(invalid(provider_path, "must not be empty"))
        }
        (Some(_), None) => Err(invalid(
            provider_path,
            format!("is set without a {model_path} to go with it"),
        )),
        (None, Some(_)) => Err(invalid(
            model_path,
            format!("is set without a {provider_path} to go with it"),
        )),
        _ => Ok(()),
    }
}

/// The user service path: `$XDG_CONFIG_HOME/norte-code/service.json`, or the
/// platform equivalent.
#[must_use]
pub fn user_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("norte-code").join("service.json"))
}

/// Default server configuration shipped with this build.
const DEFAULT_SERVICE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../default/service.json"
));

/// Default client configuration shipped with this build.
const DEFAULT_CLI_JSON: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../default/cli.json"));

/// Creates the user service configuration when it does not exist.
///
/// Existing files are never overwritten. `None` means that no user config
/// directory could be determined and the caller must keep using in-memory
/// defaults.
///
/// # Errors
///
/// Returns [`ConfigError`] when the default cannot be written.
pub fn ensure_user_service_config() -> Result<Option<PathBuf>, ConfigError> {
    ensure_default_config_file(user_config_path(), DEFAULT_SERVICE_JSON)
}

/// The client configuration path: `$XDG_CONFIG_HOME/norte-code/cli.json`, or the
/// platform equivalent.
#[must_use]
pub fn user_cli_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("norte-code").join("cli.json"))
}

/// Creates the user CLI configuration when it does not exist.
///
/// Existing files are never overwritten.
///
/// # Errors
///
/// Returns [`ConfigError`] when the default cannot be written.
pub fn ensure_user_cli_config() -> Result<Option<PathBuf>, ConfigError> {
    ensure_default_config_file(user_cli_path(), DEFAULT_CLI_JSON)
}

fn ensure_default_config_file(
    path: Option<PathBuf>,
    contents: &str,
) -> Result<Option<PathBuf>, ConfigError> {
    let Some(path) = path else {
        return Ok(None);
    };
    if path.exists() {
        return Ok(Some(path));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: path.clone(),
            source,
        })?;
    }
    atomic_write(&path, contents.as_bytes())?;
    Ok(Some(path))
}

/// The per-project overlay path under `root`.
#[must_use]
pub fn project_config_path(root: &Path) -> PathBuf {
    root.join(".norte-code").join("service.json")
}

/// The per-project client overlay path under `root`.
#[must_use]
pub fn project_cli_path(root: &Path) -> PathBuf {
    root.join(".norte-code").join("cli.json")
}

/// The file a change to the user's configuration is written to.
///
/// The path the process was started with wins, so `--config FILE` is the file
/// that is edited rather than a second one the user never looks at again; the
/// platform user path is the fallback. `None` means there is nowhere to write,
/// which the caller reports instead of guessing.
#[must_use]
pub fn resolve_user_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
    explicit.map(Path::to_path_buf).or_else(user_config_path)
}

/// Sets `default_provider` and `default_model` in a config file, preserving the
/// rest of it.
///
/// The two are written together and kept apart, because a model name is a string
/// that may carry slashes and a provider is its own key: the pair is what
/// `run` and the pickers resolve, and neither is ever split to find the other.
///
/// # Errors
///
/// Returns [`ConfigError`] when either name is empty, or when the file cannot be
/// read or written.
pub fn update_user_default(path: &Path, provider: &str, model: &str) -> Result<(), ConfigError> {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() {
        return Err(invalid("default_provider", "the provider name is empty"));
    }
    if model.is_empty() {
        return Err(invalid("default_model", "the model name is empty"));
    }
    let mut service = if path.exists() {
        ServiceConfig::load(Some(path), None)?
    } else {
        ServiceConfig::default()
    };
    let default_agent = service.default_agent.clone();
    let agent = service
        .agents
        .iter_mut()
        .find(|agent| agent.name == default_agent)
        .ok_or_else(|| invalid("default_agent", "the default agent does not exist"))?;
    agent.default_provider = Some(provider.to_owned());
    agent.default_model = Some(model.to_owned());
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let mut body = serde_json::to_string_pretty(&service).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    body.push('\n');
    atomic_write(path, body.as_bytes())
}

/// Writes a config file atomically, preserving an existing symlink and mode.
fn atomic_write(path: &Path, body: &[u8]) -> Result<(), ConfigError> {
    atomic_write_with(path, body, false)
}

/// Writes a file atomically, preserving an existing symlink and mode.
///
/// A fixed temporary name would race with another Norte Code process, and a
/// truncating write would lose the file if the process died halfway. Write a
/// unique sibling with `create_new`, then rename it over the target.
///
/// `force_private` makes the result mode 0600 even when the target already
/// existed with broader permissions. The secrets file uses it; a config file
/// keeps the mode a person gave it.
#[allow(clippy::too_many_lines)]
pub(crate) fn atomic_write_with(
    path: &Path,
    body: &[u8],
    force_private: bool,
) -> Result<(), ConfigError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[cfg(not(unix))]
    let _ = force_private;
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
            .unwrap_or("service.json");
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
            if force_private {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o600))?;
                }
            } else if let Some(permissions) = &permissions {
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

/// Removes a tool from the config's approval lists.
///
/// It is what "ask again" means, and it has to touch the file for the same
/// reason [`update_user_approval`] does: a rule forgotten in memory and kept on
/// disk comes back at the next restart, which is a rule nobody can get rid of.
///
/// A tool that is in neither list is not an error: forgetting a rule that was
/// never written is what a person asks for when they press the key twice, and
/// refusing would be the panel arguing with the press.
///
/// # Errors
///
/// Returns a [`ConfigError`] when the tool is unnamed, when the file is not a
/// JSON object, or when it cannot be read or written.
pub fn forget_user_approval(path: &Path, tool: &str) -> Result<(), ConfigError> {
    let tool = tool.trim();
    if tool.is_empty() {
        return Err(invalid("approvals", "the tool name is empty"));
    }
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_value(path)?;
    let Some(root) = root.as_object_mut() else {
        return Err(invalid(
            "approvals",
            "the config root must be a JSON object",
        ));
    };
    let Some(approvals) = root.get_mut("approvals").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let mut changed = false;
    for list in ["allow", "deny"] {
        if let Some(entries) = approvals.get_mut(list).and_then(Value::as_array_mut) {
            let before = entries.len();
            entries.retain(|entry| entry.as_str() != Some(tool));
            changed |= entries.len() != before;
        }
    }
    if !changed {
        return Ok(());
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

/// The `mcpServers` map of a config file, as a mutable JSON object.
///
/// It is created when the file does not have it, and refused when it has one of
/// the wrong shape: a `mcpServers` that is not an object is a file a person
/// edited by hand and got wrong, and writing over it would lose what they meant.
fn mcp_servers_mut(root: &mut Value) -> Result<&mut serde_json::Map<String, Value>, ConfigError> {
    let root = root
        .as_object_mut()
        .ok_or_else(|| invalid("mcpServers", "the config root must be a JSON object"))?;
    let entry = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    entry
        .as_object_mut()
        .ok_or_else(|| invalid("mcpServers", "must be a JSON object"))
}

/// Reads the file, applies `edit`, and writes it back, preserving the rest.
fn edit_config(
    path: &Path,
    edit: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    let mut root = if path.exists() {
        read_value(path)?
    } else {
        Value::Object(serde_json::Map::new())
    };
    edit(&mut root)?;
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

/// Adds one MCP server to the config file at `mcpServers.<name>`.
///
/// The entry is written as a program to run — `npx`, a path, anything the
/// platform can execute — with its arguments and an `enabled` flag. An existing
/// entry of the same name is replaced, because adding a server that is already
/// named is what a person does to change it.
///
/// # Errors
///
/// Returns [`ConfigError`] when the name or the command is empty, or when the
/// file cannot be read or written.
pub fn add_user_mcp_server(
    path: &Path,
    name: &str,
    command: &str,
    args: &[String],
) -> Result<(), ConfigError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("mcpServers", "the server name is empty"));
    }
    if command.trim().is_empty() {
        return Err(invalid("mcpServers", "the server command is empty"));
    }
    edit_config(path, |root| {
        let servers = mcp_servers_mut(root)?;
        servers.insert(
            name.to_owned(),
            serde_json::json!({
                "command": command.trim(),
                "args": args,
                "enabled": true,
            }),
        );
        Ok(())
    })
}

/// Turns one MCP server on or off in the config file.
///
/// It sets `mcpServers.<name>.enabled`, which is the same flag the config
/// carries and the one that keeps an entry without starting it.
///
/// # Errors
///
/// Returns [`ConfigError`] when the name is empty, no server has it, or the file
/// cannot be read or written.
pub fn set_user_mcp_enabled(path: &Path, name: &str, enabled: bool) -> Result<(), ConfigError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("mcpServers", "the server name is empty"));
    }
    edit_config(path, |root| {
        let servers = mcp_servers_mut(root)?;
        let Some(entry) = servers.get_mut(name) else {
            return Err(invalid(
                "mcpServers",
                format!("no server named `{name}` in the config"),
            ));
        };
        let entry = entry
            .as_object_mut()
            .ok_or_else(|| invalid("mcpServers", format!("`{name}` is not a JSON object")))?;
        entry.insert("enabled".to_owned(), Value::Bool(enabled));
        Ok(())
    })
}

/// Removes one MCP server from the config file.
///
/// A name that is not there is not an error: removing one twice is what a person
/// does when they are not sure it worked, and refusing the second press is the
/// file arguing with them.
///
/// # Errors
///
/// Returns [`ConfigError`] when the name is empty, or when the file cannot be
/// read or written.
pub fn remove_user_mcp_server(path: &Path, name: &str) -> Result<(), ConfigError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("mcpServers", "the server name is empty"));
    }
    edit_config(path, |root| {
        let servers = mcp_servers_mut(root)?;
        servers.remove(name);
        Ok(())
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
#[path = "config_tests.rs"]
mod tests;

#[cfg(test)]
mod ui_tests {
    use super::*;

    #[test]
    fn the_ui_block_is_a_subset_and_says_nothing_by_default() {
        // A config file that does not mention `ui` must behave exactly as it did
        // before the block existed, so every field has a default that is what
        // the client already did.
        let loaded = CliConfig::from_json("{}").expect("empty cli");
        let config = loaded.config;
        assert_eq!(config.verbosity, "medium");
        assert_eq!(config.diff_style, DiffStyle::Unified);
        assert_eq!(config.service_url, "http://127.0.0.1:8787");
        assert_eq!(config.service_type, ServiceType::Local);
        assert_eq!(config.service_auth, None);
    }

    #[cfg(test)]
    mod service_config_tests {
        use super::*;

        #[test]
        fn service_json_selects_a_named_agent() {
            let service: ServiceConfig = serde_json::from_str(
                r#"{
                    "version": 1,
                    "default_agent": "reviewer",
                    "agents": [
                        {"name": "builder", "description": null},
                        {"name": "reviewer", "author": "team"}
                    ]
                }"#,
            )
            .expect("service");
            service.validate().expect("valid service");
            let agent = service.default_agent().expect("default");
            assert_eq!(agent.name, "reviewer");
            assert_eq!(agent.author.as_deref(), Some("team"));
        }

        #[test]
        fn service_json_rejects_duplicate_agent_names() {
            let error = serde_json::from_str::<ServiceConfig>(
                r#"{"default_agent":"same","agents":[{"name":"same"},{"name":"same"}]}"#,
            )
            .expect("parse")
            .validate()
            .expect_err("duplicate names must fail");
            assert!(error.to_string().contains("duplicate agent name"));
        }
    }

    #[test]
    fn service_settings_are_client_only_and_preserve_raw_auth() {
        let loaded = CliConfig::from_json(
            r#"{"service_url":"https://agent.example/raep","service_type":"remote","service_auth":"raw-token"}"#,
        )
        .expect("service settings");
        assert_eq!(loaded.config.service_url, "https://agent.example/raep");
        assert_eq!(loaded.config.service_type, ServiceType::Remote);
        assert_eq!(loaded.config.service_auth.as_deref(), Some("raw-token"));
    }

    #[test]
    fn the_diff_style_is_named_the_way_a_person_writes_it() {
        // The two names are what the configuration accepts, so they are part of
        // the interface and not a detail of the enum.
        let loaded = CliConfig::from_json(r#"{"diff_style":"split"}"#).expect("split");
        let config = loaded.config;
        assert_eq!(config.diff_style, DiffStyle::Split);
        let loaded = CliConfig::from_json(r#"{"diff_style":"unified"}"#).expect("unified");
        assert_eq!(loaded.config.diff_style, DiffStyle::Unified);
        // A name that is not one of the two is refused rather than silently
        // falling back, because a person who wrote `side-by-side` should be told.
        let error = CliConfig::from_json(r#"{"diff_style":"diagonal"}"#)
            .expect_err("an unknown style is refused");
        assert!(
            error.to_string().contains("diagonal"),
            "the error names the value: {error}"
        );
    }

    #[test]
    fn verbosity_is_read_as_a_string_so_the_client_can_explain_a_bad_one() {
        // The enum lives in the client, not here: the engine never reads this
        // and a config crate that refused the value would make every host
        // reimplement the message.
        let loaded = CliConfig::from_json(r#"{"verbosity":"high"}"#).expect("high");
        let config = loaded.config;
        assert_eq!(config.verbosity, "high");
        let loaded = CliConfig::from_json(r#"{"verbosity":"loud"}"#).expect("any string");
        let config = loaded.config;
        assert_eq!(
            config.verbosity, "loud",
            "the client is what refuses it, with the levels it knows"
        );
    }

    #[test]
    fn the_style_names_round_trip() {
        assert_eq!(DiffStyle::Unified.name(), "unified");
        assert_eq!(DiffStyle::Split.name(), "split");
        assert_eq!(DiffStyle::default(), DiffStyle::Unified);
    }

    #[test]
    fn disabled_tools_are_read_from_the_file() {
        // The field as a person writes it, and the two cases that matter: the
        // list is read, and a file that does not mention it keeps every tool
        // rather than failing to load.
        let dir = std::env::temp_dir().join(format!("norte-disabled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"version":1,"tools":{"disabled":["shell","todo"]}}"#,
        )
        .expect("write");
        let loaded = Config::load(Some(&path), None).expect("load");
        assert_eq!(
            loaded.config.tools.disabled,
            vec!["shell".to_owned(), "todo".to_owned()],
            "the list is read from the file"
        );
        std::fs::write(&path, r#"{"version":1}"#).expect("write");
        let loaded = Config::load(Some(&path), None).expect("load");
        assert!(
            loaded.config.tools.disabled.is_empty(),
            "a file that does not mention it withholds nothing: {:?}",
            loaded.config.tools.disabled
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod subagent_tests {
    use super::*;

    #[test]
    fn subagent_defaults_to_the_parent_agent() {
        let loaded = Config::from_json("{}").expect("empty config");
        assert_eq!(loaded.config.default_subagent_delegation, None);
    }

    #[test]
    fn subagent_agent_round_trips() {
        let loaded =
            Config::from_json(r#"{"default_subagent_delegation":"research"}"#).expect("config");
        assert_eq!(
            loaded.config.default_subagent_delegation.as_deref(),
            Some("research")
        );
    }

    #[test]
    fn service_rejects_an_unknown_subagent_agent() {
        let service = ServiceConfig {
            default_agent: "main".to_owned(),
            agents: vec![Agent {
                name: "main".to_owned(),
                default_subagent_delegation: Some("missing".to_owned()),
                ..Agent::default()
            }],
            ..ServiceConfig::default()
        };
        let error = service.validate().expect_err("unknown agent");
        assert!(
            error
                .to_string()
                .contains("does not name a configured agent")
        );
    }

    #[test]
    fn checked_in_service_template_is_valid() {
        let service: ServiceConfig = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../default/service.json"
        )))
        .expect("service template");
        service.validate().expect("valid service template");
    }
}
