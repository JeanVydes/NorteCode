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

mod agent;
mod approvals;
mod budget;
mod cost;
mod engine;
mod error;
mod interface;
mod key;
mod loading;
mod mcp;
mod paths;
mod service;
mod tools;

pub use agent::Agent;
pub use approvals::ApprovalsConfig;
pub use budget::{BudgetConfig, BudgetsConfig};
pub use cost::CostConfig;
pub use engine::{CompactionConfig, CompactionTrigger, ReasoningConfig, RetryConfig};
pub use error::{ConfigError, Loaded};
pub use interface::{ChartConfig, ChartDotSize, CliConfig, DiffStyle, LoadedCli, ServiceType};
pub use key::ApiKey;
pub use mcp::McpServerConfig;
pub(crate) use paths::atomic_write_with;
pub use paths::{
    add_user_mcp_server, ensure_user_cli_config, ensure_user_service_config, forget_user_approval,
    project_cli_path, project_config_path, remove_user_mcp_server, resolve_user_config_path,
    set_user_mcp_enabled, update_user_approval, update_user_default, user_cli_path,
    user_config_path,
};
pub use service::ServiceConfig;
pub use tools::{
    ConfinementConfig, DEFAULT_COMMAND_MAX_MODEL_BYTES, SandboxConfig, ScreenshotConfig,
    SkillsConfig, ToolsConfig,
};

/// The only config schema revision this build understands.
pub const CONFIG_VERSION: u32 = 1;

/// Largest config file accepted.
const MAX_CONFIG_BYTES: usize = 1024 * 1024;

/// The selected agent configuration used by the engine.
pub type Config = Agent;

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

#[cfg(test)]
mod ui_tests;

#[cfg(test)]
mod subagent_tests;
