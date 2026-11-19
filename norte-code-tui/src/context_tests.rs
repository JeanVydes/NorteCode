//! Context gauge arithmetic.

use super::{ContextUsage, human_count};

fn usage(used: u64, window: Option<u64>) -> ContextUsage {
    ContextUsage { used, window }
}

#[test]
fn a_missing_window_has_nothing_to_report() {
    let none = usage(500, None);
    assert_eq!(none.fraction(), 0.0);
    assert_eq!(none.compaction_limit(), 0);
    assert!(!none.over_limit());
}

#[test]
fn the_fraction_and_percent_track_usage() {
    let usage = usage(64, Some(128));
    assert!((usage.fraction() - 0.5).abs() < f64::EPSILON);
}

#[test]
fn an_overfull_context_is_clamped_for_the_gauge() {
    let usage = usage(200, Some(100));
    assert_eq!(usage.fraction(), 1.0);
    assert!(usage.over_limit());
}

#[test]
fn the_compact_point_is_an_assumed_ninety_percent() {
    let at_limit = usage(90, Some(100));
    assert_eq!(at_limit.compaction_limit(), 90);
    assert!(at_limit.over_limit());
    assert!(!usage(89, Some(100)).over_limit());
}

#[test]
fn counts_shorten_to_k_and_m() {
    assert_eq!(human_count(999), "999");
    assert_eq!(human_count(12_000), "12k");
    assert_eq!(human_count(12_500), "12.5k");
    assert_eq!(human_count(2_000_000), "2m");
}
