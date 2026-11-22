//! Bounds applied by the `read` tool.

use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::loading::invalid;

/// Default largest file the tool opens, in bytes.
pub const DEFAULT_READ_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Default largest text the tool returns, in bytes.
pub const DEFAULT_READ_MAX_OUTPUT_BYTES: u64 = 64 * 1024;
/// Default number of lines the tool returns.
pub const DEFAULT_READ_MAX_LINES: usize = 2000;
/// Default number of regex matches the tool returns.
pub const DEFAULT_READ_MAX_MATCHES: usize = 200;
/// Default width at which a long line is cut, in characters.
pub const DEFAULT_READ_MAX_LINE_WIDTH: usize = 500;

/// Size and output bounds for the `read` tool.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadLimitsConfig {
    /// Largest file the tool opens.
    pub max_file_bytes: u64,
    /// Largest text the tool returns.
    pub max_output_bytes: u64,
    /// Number of lines returned by a read without a line range.
    pub max_lines: usize,
    /// Number of matches returned by a regex read.
    pub max_matches: usize,
    /// Width at which a long line is cut.
    pub max_line_width: usize,
}

impl Default for ReadLimitsConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_READ_MAX_FILE_BYTES,
            max_output_bytes: DEFAULT_READ_MAX_OUTPUT_BYTES,
            max_lines: DEFAULT_READ_MAX_LINES,
            max_matches: DEFAULT_READ_MAX_MATCHES,
            max_line_width: DEFAULT_READ_MAX_LINE_WIDTH,
        }
    }
}

impl ReadLimitsConfig {
    /// Validates the read bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] naming the first offending field.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.max_file_bytes == 0 {
            return Err(invalid(
                "tools.read_limits.max_file_bytes",
                "must be greater than zero",
            ));
        }
        if self.max_output_bytes == 0 || self.max_output_bytes > self.max_file_bytes {
            return Err(invalid(
                "tools.read_limits.max_output_bytes",
                "must be positive and not exceed max_file_bytes",
            ));
        }
        if self.max_lines == 0 || self.max_matches == 0 || self.max_line_width == 0 {
            return Err(invalid(
                "tools.read_limits",
                "max_lines, max_matches and max_line_width must be greater than zero",
            ));
        }
        Ok(())
    }
}
