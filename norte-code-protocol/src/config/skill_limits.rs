//! Size and time bounds for Agent Skills and bundled scripts.

use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::loading::invalid;

/// Default largest `SKILL.md` accepted.
pub const DEFAULT_MAX_SKILL_FILE_BYTES: usize = 1024 * 1024;
/// Default largest body returned by the `skill` tool.
pub const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;
/// Default largest resource returned by `skill_resource`.
pub const DEFAULT_RESOURCE_BYTES: usize = 256 * 1024;
/// Default line count after which a skill warns.
pub const DEFAULT_RECOMMENDED_LINES: usize = 500;
/// Default longest shebang line accepted.
pub const DEFAULT_MAX_SHEBANG_BYTES: usize = 4096;
/// Default script output kept per stream.
pub const DEFAULT_MAX_SCRIPT_OUTPUT_BYTES: usize = 1024 * 1024;
/// Default script output that reaches the model per stream.
pub const DEFAULT_MAX_SCRIPT_MODEL_BYTES: usize = 32 * 1024;
/// Default timeout for a script call.
pub const DEFAULT_SCRIPT_TIMEOUT_MS: u64 = 120_000;
/// Longest timeout a script call may ask for.
pub const DEFAULT_MAX_SCRIPT_TIMEOUT_MS: u64 = 600_000;

/// Bounds applied while discovering skills and running their scripts.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillLimitsConfig {
    /// Largest `SKILL.md` accepted.
    pub max_skill_file_bytes: usize,
    /// Largest body returned by the `skill` tool.
    pub max_body_bytes: usize,
    /// Largest file returned by `skill_resource`.
    pub default_resource_bytes: usize,
    /// Line count after which a skill reports a warning.
    pub recommended_lines: usize,
    /// Longest shebang line accepted from a bundled script.
    pub max_shebang_bytes: usize,
    /// Script output kept per stream.
    pub max_script_output_bytes: usize,
    /// Script output that reaches the model per stream.
    pub max_script_model_bytes: usize,
    /// Timeout used when a script call asks for none.
    pub default_script_timeout_ms: u64,
    /// Longest timeout a script call may ask for.
    pub max_script_timeout_ms: u64,
}

impl Default for SkillLimitsConfig {
    fn default() -> Self {
        Self {
            max_skill_file_bytes: DEFAULT_MAX_SKILL_FILE_BYTES,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            default_resource_bytes: DEFAULT_RESOURCE_BYTES,
            recommended_lines: DEFAULT_RECOMMENDED_LINES,
            max_shebang_bytes: DEFAULT_MAX_SHEBANG_BYTES,
            max_script_output_bytes: DEFAULT_MAX_SCRIPT_OUTPUT_BYTES,
            max_script_model_bytes: DEFAULT_MAX_SCRIPT_MODEL_BYTES,
            default_script_timeout_ms: DEFAULT_SCRIPT_TIMEOUT_MS,
            max_script_timeout_ms: DEFAULT_MAX_SCRIPT_TIMEOUT_MS,
        }
    }
}

impl SkillLimitsConfig {
    /// Validates the skill bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] naming the first offending field.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let sizes = [
            ("max_skill_file_bytes", self.max_skill_file_bytes),
            ("max_body_bytes", self.max_body_bytes),
            ("default_resource_bytes", self.default_resource_bytes),
            ("recommended_lines", self.recommended_lines),
            ("max_shebang_bytes", self.max_shebang_bytes),
            ("max_script_output_bytes", self.max_script_output_bytes),
            ("max_script_model_bytes", self.max_script_model_bytes),
        ];
        for (field, value) in sizes {
            if value == 0 {
                return Err(invalid(
                    format!("skills.limits.{field}"),
                    "must be greater than zero",
                ));
            }
        }
        if self.max_body_bytes > self.max_skill_file_bytes
            || self.default_resource_bytes > self.max_skill_file_bytes
        {
            return Err(invalid(
                "skills.limits",
                "body and resource caps must not exceed max_skill_file_bytes",
            ));
        }
        if self.max_script_model_bytes > self.max_script_output_bytes {
            return Err(invalid(
                "skills.limits.max_script_model_bytes",
                "must not exceed max_script_output_bytes",
            ));
        }
        if self.default_script_timeout_ms == 0
            || self.default_script_timeout_ms > self.max_script_timeout_ms
        {
            return Err(invalid(
                "skills.limits.default_script_timeout_ms",
                "must be positive and not exceed max_script_timeout_ms",
            ));
        }
        Ok(())
    }
}
