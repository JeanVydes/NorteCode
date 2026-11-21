//! Frame pacing for the run loop.

use std::time::{Duration, Instant};

/// Maximum number of painted frames per second.
pub(crate) const FRAME_RATE_CAP: u32 = 60;

const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / (FRAME_RATE_CAP as u64));

/// Allows at most one frame per display interval.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameLimiter {
    last: Option<Instant>,
}

impl Default for FrameLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameLimiter {
    pub(crate) fn new() -> Self {
        Self { last: None }
    }

    /// Reserve the next frame slot when enough time has passed.
    pub(crate) fn allow(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|last| now.duration_since(last) < FRAME_INTERVAL)
        {
            return false;
        }
        self.last = Some(now);
        true
    }

    /// How long to wait before the next frame slot opens.
    pub(crate) fn remaining(self, now: Instant) -> Duration {
        let Some(last) = self.last else {
            return Duration::ZERO;
        };
        FRAME_INTERVAL.saturating_sub(now.saturating_duration_since(last))
    }
}

#[cfg(test)]
#[path = "frame_limiter_tests.rs"]
mod tests;
