//! Tool rules that outlive one session.

use serde::{Deserialize, Serialize};

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
