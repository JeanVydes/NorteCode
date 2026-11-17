//! The clock a message is stamped with.
//!
//! A message carries the moment the host recorded it, in milliseconds since
//! the epoch. Printing that number is unreadable, so the transcript stamps a
//! message with how long it has been since the session started.

/// The moment a session started, in milliseconds since the epoch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SessionClock {
    started_at: Option<i64>,
}

impl SessionClock {
    /// A clock that has not started.
    #[must_use]
    #[cfg(test)]
    pub(crate) const fn new() -> Self {
        Self { started_at: None }
    }

    /// Starts the clock at a moment, unless it is already running.
    pub(crate) fn mark(&mut self, milliseconds: i64) {
        if self.started_at.is_none() {
            self.started_at = Some(milliseconds);
        }
    }

    /// Starts the clock over, for a new session.
    pub(crate) fn reset(&mut self) {
        self.started_at = None;
    }

    /// The elapsed time as `mm:ss`, or `--:--` before the clock started.
    pub(crate) fn stamp(&self, milliseconds: i64) -> String {
        let Some(started_at) = self.started_at else {
            return "--:--".to_owned();
        };
        let elapsed = milliseconds.saturating_sub(started_at).max(0) / 1_000;
        let minutes = elapsed / 60;
        let seconds = elapsed % 60;
        format!("{minutes:02}:{seconds:02}")
    }
}
