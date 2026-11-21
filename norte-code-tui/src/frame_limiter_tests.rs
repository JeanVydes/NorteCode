//! Frame pacing behavior.

use std::time::{Duration, Instant};

use super::{FRAME_INTERVAL, FrameLimiter};

#[test]
fn the_first_frame_is_allowed_immediately() {
    let now = Instant::now() + Duration::from_secs(1);
    let mut limiter = FrameLimiter::new();

    assert!(limiter.allow(now));
    assert!(!limiter.allow(now));
}

#[test]
fn the_next_frame_opens_after_one_interval() {
    let now = Instant::now() + Duration::from_secs(1);
    let mut limiter = FrameLimiter::new();
    assert!(limiter.allow(now));

    assert!(limiter.allow(now + FRAME_INTERVAL));
}

#[test]
fn remaining_time_shrinks_until_the_next_frame() {
    let now = Instant::now() + Duration::from_secs(1);
    let mut limiter = FrameLimiter::new();
    assert_eq!(limiter.remaining(now), Duration::ZERO);
    assert!(limiter.allow(now));

    assert_eq!(limiter.remaining(now), FRAME_INTERVAL);
    assert_eq!(
        limiter.remaining(now + FRAME_INTERVAL / 2),
        FRAME_INTERVAL / 2
    );
    assert_eq!(limiter.remaining(now + FRAME_INTERVAL), Duration::ZERO);
}
