//! Frame loop measurement behavior.

use std::time::{Duration, Instant};

use super::{Metrics, Phase};

#[test]
fn phase_timings_report_last_mean_and_worst() {
    let mut metrics = Metrics::new();
    metrics.record(Phase::Drain, Duration::from_millis(2));
    metrics.record(Phase::Drain, Duration::from_millis(6));
    metrics.record(Phase::Drain, Duration::from_millis(4));

    let timing = metrics
        .snapshot(Instant::now(), 0, false)
        .phases
        .get(Phase::Drain);
    assert_eq!(timing.last, Duration::from_millis(4));
    assert_eq!(timing.average, Duration::from_millis(4));
    assert_eq!(timing.max, Duration::from_millis(6));
}

#[test]
fn frame_and_loop_rates_use_the_recent_window() {
    let mut metrics = Metrics::new();
    let now = Instant::now() + Duration::from_secs(1);
    for offset in [900, 600, 300] {
        let drawn_at = now
            .checked_sub(Duration::from_millis(offset))
            .expect("test clock has room");
        metrics.frame_drawn(drawn_at, Duration::from_millis(1));
    }
    metrics.frame_drawn(now, Duration::from_millis(1));
    for offset in [900, 600, 300, 0] {
        let iterated_at = now
            .checked_sub(Duration::from_millis(offset))
            .expect("test clock has room");
        metrics.iteration(iterated_at);
    }

    let snapshot = metrics.snapshot(now, 2, true);
    assert_eq!(snapshot.frames_drawn, 4);
    assert_eq!(snapshot.queued_events, 2);
    assert!(snapshot.active);
    assert!(
        (snapshot.drawn_fps - 4.44).abs() < 0.1,
        "{}",
        snapshot.drawn_fps
    );
    assert!(
        (snapshot.loop_fps - 4.44).abs() < 0.1,
        "{}",
        snapshot.loop_fps
    );
}

#[test]
fn skipped_frames_and_timeouts_are_counted() {
    let mut metrics = Metrics::new();
    for _ in 0..3 {
        metrics.frame_skipped();
        metrics.poll_timeout();
        metrics.input_event();
    }
    metrics.server_events(7);

    let snapshot = metrics.snapshot(Instant::now(), 0, false);
    assert_eq!(snapshot.frames_skipped, 3);
    assert_eq!(snapshot.poll_timeouts, 3);
    assert_eq!(snapshot.input_events, 3);
    assert_eq!(snapshot.server_events, 7);
    assert_eq!(snapshot.frames_drawn, 0);
}
