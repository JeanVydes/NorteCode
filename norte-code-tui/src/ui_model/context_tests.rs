//! The context gauge: how full the window is, and where compaction starts.

use super::{ContextGauge, SessionStatsSummary};

fn stats(used: u64, window: u64) -> SessionStatsSummary {
    SessionStatsSummary {
        context_used: used,
        context_window: Some(window),
        ..SessionStatsSummary::default()
    }
}

#[test]
fn a_gauge_needs_both_the_usage_and_the_window() {
    assert!(ContextGauge::of(None).is_none());
    assert!(ContextGauge::of(Some(&SessionStatsSummary::default())).is_none());
}

#[test]
fn the_fraction_is_used_over_window() {
    let gauge = ContextGauge::of(Some(&stats(50, 200))).expect("gauge");

    assert!((gauge.fraction() - 0.25).abs() < f64::EPSILON);
}

#[test]
fn the_compaction_point_is_ninety_percent_of_the_window() {
    let gauge = ContextGauge::of(Some(&stats(10, 200_000))).expect("gauge");

    assert_eq!(gauge.compaction_limit(), 180_000);
    assert!(!gauge.over_limit());
}

#[test]
fn a_full_window_is_over_the_limit() {
    let gauge = ContextGauge::of(Some(&stats(185_000, 200_000))).expect("gauge");

    assert!(gauge.over_limit());
}

#[test]
fn the_band_turns_danger_at_ninety_percent() {
    use crate::components::palette::GaugeBand;

    assert_eq!(GaugeBand::of(0.0), GaugeBand::Calm);
    assert_eq!(GaugeBand::of(0.49), GaugeBand::Calm);
    assert_eq!(GaugeBand::of(0.50), GaugeBand::Filling);
    assert_eq!(GaugeBand::of(0.75), GaugeBand::Close);
    assert_eq!(GaugeBand::of(0.90), GaugeBand::Full);
    assert_eq!(GaugeBand::of(1.0), GaugeBand::Full);
}

#[test]
fn the_bands_have_four_distinct_colors() {
    use crate::components::palette::GaugeBand;

    let colors = [
        GaugeBand::Calm.color(),
        GaugeBand::Filling.color(),
        GaugeBand::Close.color(),
        GaugeBand::Full.color(),
    ];
    for (index, color) in colors.iter().enumerate() {
        assert!(!colors[..index].contains(color), "two bands share a color");
    }
}
