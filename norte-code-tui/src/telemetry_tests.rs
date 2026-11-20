//! Interval labels and cycle.

use super::{ChartInterval, short_label};

#[test]
fn the_interval_cycle_wraps() {
    assert_eq!(ChartInterval::Minute.next(), ChartInterval::Hour);
    assert_eq!(ChartInterval::Hour.next(), ChartInterval::Day);
    assert_eq!(ChartInterval::Day.next(), ChartInterval::Minute);
}

#[test]
fn each_interval_has_a_label() {
    assert_eq!(ChartInterval::Minute.label(), "minute");
    assert_eq!(ChartInterval::Hour.label(), "hour");
    assert_eq!(ChartInterval::Day.label(), "day");
}

#[test]
fn axis_labels_fit_the_gutter() {
    let key = "2026-11-19T10:30";
    assert_eq!(short_label(key, ChartInterval::Minute), "10:30");
    assert_eq!(short_label(key, ChartInterval::Hour), "10:00");
    assert_eq!(short_label(key, ChartInterval::Day), "11-19");
}
