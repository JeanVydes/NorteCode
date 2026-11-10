//! The service file: its agents and their defaults.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::loading::{invalid, merge, read_value};
use super::{Agent, CONFIG_VERSION, ConfigError};

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
