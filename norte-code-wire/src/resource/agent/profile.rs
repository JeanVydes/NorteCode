//! Safe configuration summaries attached to discovered agents.

use serde::{Deserialize, Serialize};

/// A useful, non-secret view of an agent profile for clients that can show
/// more than its name and model. Paths, commands, environment values, API
/// credentials and the system prompt are never included.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentProfileSummary {
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Person or organization that publishes the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Public definition URL, when configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_url: Option<String>,
    /// Configured default target for delegation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_subagent_delegation: Option<String>,
    /// Agents this profile may delegate to.
    #[serde(default)]
    pub delegates: Vec<String>,
    /// The default reasoning mode.
    #[serde(default)]
    pub reasoning: AgentReasoningSummary,
    /// Automatic compaction behavior.
    #[serde(default)]
    pub compaction: AgentCompactionSummary,
    /// Retry behavior for transient provider failures.
    #[serde(default)]
    pub retry: AgentRetrySummary,
    /// Limits on delegated child sessions.
    #[serde(default)]
    pub delegation: AgentDelegationSummary,
    /// Maximum model/tool iterations in a turn, when bounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
    /// Available tools and command restrictions.
    #[serde(default)]
    pub tools: AgentToolSummary,
    /// Tool-wide approval rules.
    #[serde(default)]
    pub approvals: AgentApprovalSummary,
    /// Configured spending limits.
    #[serde(default)]
    pub budgets: AgentBudgetSummary,
    /// Skill discovery and execution policy.
    #[serde(default)]
    pub skills: AgentSkillsSummary,
    /// Plugin permissions; this does not imply that a plugin is installed.
    #[serde(default)]
    pub plugins: Vec<AgentPluginSummary>,
    /// MCP server names and their safe-to-show routing summary.
    #[serde(default)]
    pub mcp_servers: Vec<AgentMcpSummary>,
    /// Whether a custom system prompt is configured. Its contents stay private.
    #[serde(default)]
    pub custom_system_prompt: bool,
}

/// The selected default reasoning mode.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(tag = "mode", content = "value", rename_all = "snake_case")]
pub enum AgentReasoningSummary {
    /// Let the model decide.
    #[default]
    Automatic,
    /// Reasoning is disabled where supported.
    Disabled,
    /// A provider/model reasoning effort.
    Effort(String),
    /// A token budget for reasoning.
    Budget(u32),
    /// An unrecognized setting; the original value is intentionally omitted.
    Custom,
}

/// Whether and when the host compacts conversation history.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentCompactionSummary {
    /// Whether automatic compaction is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Context fraction that triggers compaction.
    #[serde(default)]
    pub threshold: f64,
    /// The safe point where compaction may run.
    #[serde(default)]
    pub trigger: AgentCompactionTrigger,
    /// Recent messages kept verbatim.
    #[serde(default)]
    pub keep_recent: usize,
}

/// When automatic compaction can run.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCompactionTrigger {
    /// At an idle turn boundary.
    #[default]
    Idle,
    /// At the first safe request boundary inside a turn.
    Eager,
}

/// Bounds for retries of transient failures.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRetrySummary {
    /// Retry attempts after the first request.
    #[serde(default)]
    pub max_attempts: u32,
    /// Largest accepted retry delay.
    #[serde(default)]
    pub max_delay_ms: u64,
    /// Initial backoff when the provider gives no retry delay.
    #[serde(default)]
    pub backoff_ms: u64,
}

/// Bounds on child sessions and their queued messages.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentDelegationSummary {
    /// Maximum delegation nesting depth.
    #[serde(default)]
    pub max_depth: usize,
    /// Concurrent child sessions allowed.
    #[serde(default)]
    pub max_concurrent: usize,
    /// Parent messages held while children run.
    #[serde(default)]
    pub max_held_messages: usize,
}

/// Tool availability and command confinement policy.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentToolSummary {
    /// Whether web fetch is available.
    #[serde(default)]
    pub web_fetch: bool,
    /// Whether finished jobs start their own turn.
    #[serde(default)]
    pub auto_turn_on_job_end: bool,
    /// Whether tool paths are restricted to the workspace.
    #[serde(default)]
    pub workspace_only: bool,
    /// Whether shell commands run in the platform sandbox.
    #[serde(default)]
    pub sandbox_enabled: bool,
    /// Whether sandboxed commands may use the network.
    #[serde(default)]
    pub sandbox_network: bool,
    /// Whether shell calls may run without a time limit.
    #[serde(default)]
    pub allow_indefinite_shell: bool,
    /// Default shell timeout.
    #[serde(default)]
    pub shell_default_timeout_ms: u64,
    /// Maximum shell timeout.
    #[serde(default)]
    pub shell_max_timeout_ms: u64,
    /// Tools omitted from the model's available tool set.
    #[serde(default)]
    pub disabled: Vec<String>,
}

/// Explicit tool-wide allow and deny rules.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentApprovalSummary {
    /// Tools allowed without asking.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Tools always refused.
    #[serde(default)]
    pub deny: Vec<String>,
}

/// Spending limits for each configured scope.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AgentBudgetSummary {
    /// One message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<AgentBudget>,
    /// One session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<AgentBudget>,
    /// One day across sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_daily: Option<AgentBudget>,
}

/// One configured spending limit.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentBudget {
    /// What it measures.
    pub unit: crate::BudgetUnit,
    /// The threshold.
    pub limit: f64,
    /// What happens at the threshold.
    pub action: crate::BudgetAction,
}

/// Skill discovery and execution switches.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSkillsSummary {
    /// Whether skills are discovered and their tools registered.
    #[serde(default)]
    pub enabled: bool,
    /// Whether interoperable `.agents/skills` roots are searched.
    #[serde(default)]
    pub interop: bool,
    /// Whether bundled scripts can run.
    #[serde(default)]
    pub allow_scripts: bool,
    /// Whether skills may pre-approve tools.
    #[serde(default)]
    pub honor_allowed_tools: bool,
    /// Maximum valid skills kept.
    #[serde(default)]
    pub max_skills: usize,
    /// Number of extra discovery roots, without exposing their paths.
    #[serde(default)]
    pub extra_path_count: usize,
}

/// A configured plugin permission.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentPluginSummary {
    /// Plugin id from the agent configuration.
    #[serde(default)]
    pub id: String,
    /// Whether it may request user-role messages.
    #[serde(default)]
    pub allow_user_messages: bool,
}

/// A configured MCP server without its command, URL, args, headers or env.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentMcpSummary {
    /// Name used by the agent.
    #[serde(default)]
    pub name: String,
    /// Whether the server is started.
    #[serde(default)]
    pub enabled: bool,
    /// Whether it uses remote HTTP transport.
    #[serde(default)]
    pub remote: bool,
    /// Count of tools explicitly allowed, or zero when unrestricted.
    #[serde(default)]
    pub allow_count: usize,
    /// Count of tools explicitly denied.
    #[serde(default)]
    pub deny_count: usize,
}
