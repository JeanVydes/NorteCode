//! The tools a session may use and how they run.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{ReadLimitsConfig, SkillLimitsConfig};

/// Default cap on the bytes of each command stream that reach the model.
pub const DEFAULT_COMMAND_MAX_MODEL_BYTES: usize = 128 * 1024;

/// Default time `ask_user` waits for an answer.
pub const DEFAULT_ASK_USER_TIMEOUT_MS: u64 = 600_000;

/// Default length of a verbose tool result shown to the client.
pub const DEFAULT_VERBOSE_TOOL_OUTPUT_CHARS: usize = 4_000;

/// Default bound for a shell call whose request asks for none.
pub const DEFAULT_SHELL_DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// Default ceiling for a shell call's requested bound.
pub const DEFAULT_SHELL_MAX_TIMEOUT_MS: u64 = 600_000;
/// Longest runtime the host may configure for a shell call.
pub const MAX_SHELL_TIMEOUT_MS: u64 = 86_400_000;

/// Default number of plugin messages one turn may inject.
pub const DEFAULT_PLUGIN_MESSAGES_PER_TURN: usize = 2;

/// Default byte cap for one plugin message.
pub const DEFAULT_PLUGIN_MESSAGE_MAX_BYTES: usize = 1024 * 1024;

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
    /// How long `ask_user` waits for a client answer.
    pub ask_user_timeout_ms: u64,
    /// Longest tool output the verbose client renderer shows.
    pub verbose_tool_output_chars: usize,
    /// Runtime bounds for shell calls, detached jobs included.
    pub shell_limits: ShellLimitsConfig,
    /// Size and output bounds for the `read` tool.
    pub read_limits: ReadLimitsConfig,
    /// How far a tool may reach.
    pub confinement: ConfinementConfig,
    /// Policy for user-role messages plugins ask the host to add.
    pub plugin_messages: PluginMessagesConfig,
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
            ask_user_timeout_ms: DEFAULT_ASK_USER_TIMEOUT_MS,
            verbose_tool_output_chars: DEFAULT_VERBOSE_TOOL_OUTPUT_CHARS,
            shell_limits: ShellLimitsConfig::default(),
            read_limits: ReadLimitsConfig::default(),
            confinement: ConfinementConfig::default(),
            plugin_messages: PluginMessagesConfig::default(),
            auto_turn_on_job_end: false,
            disabled: Vec::new(),
        }
    }
}

/// Runtime bounds for a shell call and the job it may become.
///
/// A request of `0` asks for no bound. When `allow_indefinite` is off, the
/// default bound is applied instead, which keeps a hung call distinguishable
/// from one that is still working. A host that turns it on gets unbounded
/// calls; the process remains cancellable and its process group is still
/// killed on cancel or timeout.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellLimitsConfig {
    /// Whether a request of `0` runs without a bound.
    pub allow_indefinite: bool,
    /// Bound used when the request asks for none, in milliseconds.
    pub default_timeout_ms: u64,
    /// Ceiling for a requested bound, in milliseconds.
    pub max_timeout_ms: u64,
}

impl Default for ShellLimitsConfig {
    fn default() -> Self {
        Self {
            allow_indefinite: true,
            default_timeout_ms: DEFAULT_SHELL_DEFAULT_TIMEOUT_MS,
            max_timeout_ms: DEFAULT_SHELL_MAX_TIMEOUT_MS,
        }
    }
}

/// Policy for user-role messages plugins ask the host to add.
///
/// Both this switch and the plugin's own permission must be on before a plugin
/// effect is applied. A plugin cannot grant itself this ability.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginMessagesConfig {
    /// Whether any plugin message effect is applied.
    pub enabled: bool,
    /// How many messages one turn may inject.
    pub max_messages_per_turn: usize,
    /// How large one message may be, counting text and inline media bytes.
    pub max_bytes: usize,
    /// Whether a plugin message may carry media parts.
    pub allow_media: bool,
}

impl Default for PluginMessagesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_messages_per_turn: DEFAULT_PLUGIN_MESSAGES_PER_TURN,
            max_bytes: DEFAULT_PLUGIN_MESSAGE_MAX_BYTES,
            allow_media: false,
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
    /// Size and time bounds for skills and bundled scripts.
    pub limits: SkillLimitsConfig,
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
            limits: SkillLimitsConfig::default(),
        }
    }
}
