//! How full the attached session's context window is.

/// The fraction of the window where compaction is assumed to trigger.
///
/// The host decides this with its own configuration and does not send it,
/// so the compact point shown is an assumption, not a reported fact.
pub(crate) const COMPACTION_ASSUMED_PERCENT: u128 = 90;

/// The assumed compact point as a fraction, for a gauge threshold.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn compaction_threshold() -> f64 {
    COMPACTION_ASSUMED_PERCENT as f64 / 100.0
}

/// The context the session last reported holding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ContextUsage {
    /// Tokens the last request carried.
    pub(crate) used: u64,
    /// The model's declared window, when known and plausible.
    pub(crate) window: Option<u64>,
}

impl ContextUsage {
    /// The fraction of the window in use, in `0.0..=1.0`.
    pub(crate) fn fraction(self) -> f64 {
        let Some(window) = self.window.filter(|window| *window > 0) else {
            return 0.0;
        };
        #[allow(clippy::cast_precision_loss)]
        let fraction = self.used as f64 / window as f64;
        fraction.clamp(0.0, 1.0)
    }

    /// Tokens at which compaction is expected to trigger.
    pub(crate) fn compaction_limit(self) -> u64 {
        let Some(window) = self.window.filter(|window| *window > 0) else {
            return 0;
        };
        u64::try_from(u128::from(window) * COMPACTION_ASSUMED_PERCENT / 100).unwrap_or(u64::MAX)
    }

    /// Whether the assumed compact point has been reached.
    pub(crate) fn over_limit(self) -> bool {
        self.window.is_some() && self.used >= self.compaction_limit()
    }
}

/// A token count in the compact form used across the interface.
pub(crate) fn human_count(count: u64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    if count < 1_000_000 {
        let thousands = count / 1_000;
        let remainder = (count % 1_000) / 100;
        if remainder == 0 {
            return format!("{thousands}k");
        }
        return format!("{thousands}.{remainder}k");
    }
    let millions = count / 1_000_000;
    let remainder = (count % 1_000_000) / 100_000;
    if remainder == 0 {
        format!("{millions}m")
    } else {
        format!("{millions}.{remainder}m")
    }
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
