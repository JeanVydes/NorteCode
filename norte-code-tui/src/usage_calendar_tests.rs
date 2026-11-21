//! Usage is folded into the correct day of the trailing 52-week grid.

use chrono::NaiveDate;
use norte_code_wire::{TelemetryRecord, TelemetryReport};

use super::UsageCalendar;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("valid test date")
}

fn report(entries: &[(NaiveDate, u64, u64)]) -> TelemetryReport {
    TelemetryReport {
        records: entries
            .iter()
            .map(|(day, input, output)| TelemetryRecord {
                id: day.to_string(),
                started_at: format!("{day}T12:00:00Z"),
                input_tokens: Some(*input),
                output_tokens: Some(*output),
                ..TelemetryRecord::default()
            })
            .collect(),
        ..TelemetryReport::default()
    }
}

#[test]
fn usage_falls_into_the_week_and_weekday_of_its_timestamp() {
    let data = report(&[(date(2026, 11, 19), 100, 20), (date(2026, 11, 21), 50, 0)]);
    let calendar = UsageCalendar::from_report(Some(&data), date(2026, 11, 21));

    assert_eq!(calendar.total_tokens(), 170);
    assert_eq!(calendar.tokens(51, 4), 120);
    assert_eq!(calendar.tokens(51, 6), 50);
}

#[test]
fn future_days_in_the_current_week_are_marked() {
    let calendar = UsageCalendar::from_report(None, date(2026, 11, 19));

    assert!(!calendar.is_future(51, 4));
    assert!(calendar.is_future(51, 5));
    assert!(calendar.is_future(51, 6));
    assert!(!calendar.is_future(50, 6));
}

#[test]
fn records_outside_the_trailing_year_are_ignored() {
    let data = report(&[(date(2025, 11, 22), 90, 90), (date(2026, 11, 22), 90, 90)]);
    let calendar = UsageCalendar::from_report(Some(&data), date(2026, 11, 21));

    assert_eq!(calendar.total_tokens(), 0);
}

#[test]
fn levels_follow_the_quartiles_of_active_days() {
    let data = report(&[
        (date(2026, 11, 15), 10, 0),
        (date(2026, 11, 16), 20, 0),
        (date(2026, 11, 17), 30, 0),
        (date(2026, 11, 18), 40, 0),
    ]);
    let calendar = UsageCalendar::from_report(Some(&data), date(2026, 11, 19));

    assert_eq!(calendar.level(51, 0), 1);
    assert_eq!(calendar.level(51, 1), 2);
    assert_eq!(calendar.level(51, 2), 3);
    assert_eq!(calendar.level(51, 3), 4);
    assert_eq!(calendar.level(51, 4), 0);
}

#[test]
fn month_labels_follow_the_week_axis() {
    let calendar = UsageCalendar::from_report(None, date(2026, 11, 21));
    let names: Vec<&str> = calendar
        .month_labels()
        .iter()
        .map(|(_, name)| *name)
        .collect();

    assert_eq!(names.first(), Some(&"Dec"));
    assert_eq!(names.last(), Some(&"Nov"));
    assert_eq!(names.len(), 12);
}
