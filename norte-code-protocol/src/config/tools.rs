//! The tools a session may use and how they run.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
const DEFAULT_SCREENSHOT_MAX_DIMENSION: u32 = 1568;
/// The largest a promoted screenshot may be after re-encoding.
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
