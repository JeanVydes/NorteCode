//! How full the context window is.
//!
//! The footer paints this as a gauge. The window comes from the model, the
//! used part from the session's usage; when either is missing there is nothing
//! honest to paint.

use crate::ui_model::SessionStatsSummary;

/// The fraction of the window a compaction is assumed to trigger at.
///
/// The host decides this with its own configuration and does not send it, so
/// the footer marks the limit as an assumption rather than a fact.
pub(crate) const COMPACTION_ASSUMED_THRESHOLD: f64 = 0.90;

/// How full the context is, and where compaction is expected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ContextGauge {
    /// The tokens the session holds, as of the last reported request.
    pub(crate) used: u64,
    /// The window the model declares.
    pub(crate) window: u64,
}

impl ContextGauge {
    /// The gauge the session's usage describes, when it describes one.
    pub(crate) fn of(stats: Option<&SessionStatsSummary>) -> Option<Self> {
        let stats = stats?;
        let window = stats.context_window.filter(|window| *window > 0)?;
        Some(Self {
            used: stats.context_used,
            window,
        })
    }

    /// How full the window is, from 0.0 to 1.0 and beyond.
    ///
    /// The gauge only needs a fraction to paint and to compare against the
    /// thresholds, so a lossy conversion to `f64` is the right tool here: a
    /// token count above 2^53 is not a window any model declares.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn fraction(self) -> f64 {
        self.used as f64 / self.window as f64
    }

    /// The tokens at which the assumed compaction triggers.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub(crate) fn compaction_limit(self) -> u64 {
        (self.window as f64 * COMPACTION_ASSUMED_THRESHOLD) as u64
    }

    /// Whether the assumed compaction point has been reached.
    pub(crate) fn over_limit(self) -> bool {
        self.used >= self.compaction_limit()
    }

    /// The percentage full, rounded and saturating.
    pub(crate) fn percent(self) -> u64 {
        if self.window == 0 {
            return 0;
        }
        // `used * 100` can overflow a narrow window, so the multiplication is
        // done in 128 bits and clamped back.
        let percent = (u128::from(self.used) * 100) / u128::from(self.window);
        u64::try_from(percent.min(999)).unwrap_or(999)
    }

    /// How many cells of a bar of `cells` are filled.
    pub(crate) fn filled_cells(self, cells: usize) -> usize {
        if self.window == 0 {
            return 0;
        }
        let cells_u128 = u128::try_from(cells).unwrap_or(1);
        let filled = (u128::from(self.used) * cells_u128).div_ceil(u128::from(self.window));
        usize::try_from(filled).unwrap_or(cells).min(cells)
    }
}
