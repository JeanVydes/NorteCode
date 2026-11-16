//! The agent a session runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::keys::KeyStore;

use super::loading::{invalid, merge, read_text, read_value, validate_pair};
use super::{
    ApiKey, ApprovalsConfig, BudgetsConfig, CONFIG_VERSION, CliConfig, CompactionConfig, Config,
    ConfigError, CostConfig, Loaded, McpServerConfig, ReasoningConfig, RetryConfig, SkillsConfig,
    ToolsConfig,
};

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
