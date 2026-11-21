//! A GitHub-shaped year of usage, aggregated by calendar day.
//!
//! Columns are weeks starting on Sunday, rows are weekdays. Each cell holds
//! the input plus output tokens burned on that day. The window ends today, so
//! the last week is partial; its remaining days are reported as future.

use chrono::{DateTime, Datelike, Duration, Month, NaiveDate};
use norte_code_wire::TelemetryReport;

/// Weeks drawn by the annual heatmap.
pub(crate) const CALENDAR_WEEKS: usize = 52;
/// Cells in the grid: one per weekday in every week.
pub(crate) const CALENDAR_DAYS: usize = CALENDAR_WEEKS * 7;

/// Token usage for the trailing year, with display levels computed once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageCalendar {
    /// Sunday of the oldest week in the grid.
    start: NaiveDate,
    /// Last day counted, normally today.
    end: NaiveDate,
    /// Input plus output tokens, indexed from `start`.
    tokens: Vec<u64>,
    /// Relative intensity from 0 to 4, indexed like `tokens`.
    levels: Vec<u8>,
    /// Month changes, as `(week, short name)` pairs.
    month_labels: Vec<(usize, &'static str)>,
}

impl UsageCalendar {
    /// Aggregate a report into the 52 weeks ending on `end`.
    pub(crate) fn from_report(report: Option<&TelemetryReport>, end: NaiveDate) -> Self {
        let sunday_offset = i64::from(end.weekday().num_days_from_sunday());
        let current_sunday = end
            .checked_sub_signed(Duration::days(sunday_offset))
            .unwrap_or(end);
        let start = current_sunday
            .checked_sub_signed(Duration::days(51 * 7))
            .unwrap_or(current_sunday);
        let mut tokens: Vec<u64> = vec![0; CALENDAR_DAYS];
        if let Some(report) = report {
            for record in &report.records {
                let Ok(stamp) = DateTime::parse_from_rfc3339(&record.started_at) else {
                    continue;
                };
                let date = stamp.date_naive();
                let Ok(offset) = usize::try_from((date - start).num_days()) else {
                    continue;
                };
                let Some(cell) = tokens.get_mut(offset) else {
                    continue;
                };
                if date > end {
                    continue;
                }
                *cell = (*cell).saturating_add(record.input_tokens.unwrap_or(0));
                *cell = (*cell).saturating_add(record.output_tokens.unwrap_or(0));
            }
        }
        let levels = levels(&tokens);
        let month_labels = month_labels(start, end);
        Self {
            start,
            end,
            tokens,
            levels,
            month_labels,
        }
    }

    /// Intensity of one day, from 0 for unused to 4 for the busiest tier.
    pub(crate) fn level(&self, week: usize, weekday: usize) -> u8 {
        let index = week * 7 + weekday;
        self.levels.get(index).copied().unwrap_or(0)
    }

    /// Tokens burned on one day.
    #[cfg(test)]
    pub(crate) fn tokens(&self, week: usize, weekday: usize) -> u64 {
        let index = week * 7 + weekday;
        self.tokens.get(index).copied().unwrap_or(0)
    }

    /// Whether a cell belongs to the current week after today.
    pub(crate) fn is_future(&self, week: usize, weekday: usize) -> bool {
        let index = i64::try_from(week * 7 + weekday).unwrap_or(i64::MAX);
        let Some(date) = self.start.checked_add_signed(Duration::days(index)) else {
            return false;
        };
        date > self.end
    }

    /// Where month names belong on the week axis.
    pub(crate) fn month_labels(&self) -> &[(usize, &'static str)] {
        &self.month_labels
    }

    /// Tokens burned in the whole window.
    #[cfg(test)]
    pub(crate) fn total_tokens(&self) -> u64 {
        self.tokens.iter().copied().sum()
    }
}

fn levels(tokens: &[u64]) -> Vec<u8> {
    let mut active: Vec<u64> = tokens.iter().copied().filter(|value| *value > 0).collect();
    active.sort_unstable();
    let quartile = |percent: usize| -> u64 {
        if active.is_empty() {
            return 0;
        }
        let index = active.len().saturating_mul(percent).saturating_sub(1) / 100;
        active[index.min(active.len() - 1)]
    };
    let cutoffs = [quartile(25), quartile(50), quartile(75)];
    tokens
        .iter()
        .map(|value| match *value {
            0 => 0,
            value if value <= cutoffs[0] => 1,
            value if value <= cutoffs[1] => 2,
            value if value <= cutoffs[2] => 3,
            _ => 4,
        })
        .collect()
}

fn month_labels(start: NaiveDate, end: NaiveDate) -> Vec<(usize, &'static str)> {
    let mut labels = Vec::new();
    let mut previous = None;
    for week in 0..CALENDAR_WEEKS {
        let sunday = i64::from(u32::try_from(week).unwrap_or(0)) * 7;
        let Some(thursday) = start.checked_add_signed(Duration::days(sunday + 3)) else {
            break;
        };
        if thursday > end {
            break;
        }
        let Ok(month) = Month::try_from(u8::try_from(thursday.month()).unwrap_or(0)) else {
            break;
        };
        if previous != Some(month) {
            let first_of_month = NaiveDate::from_ymd_opt(thursday.year(), thursday.month(), 1);
            if first_of_month.is_some_and(|first| first >= start) {
                labels.push((week, month_name(month)));
            }
            previous = Some(month);
        }
    }
    labels
}

fn month_name(month: Month) -> &'static str {
    match month {
        Month::January => "Jan",
        Month::February => "Feb",
        Month::March => "Mar",
        Month::April => "Apr",
        Month::May => "May",
        Month::June => "Jun",
        Month::July => "Jul",
        Month::August => "Aug",
        Month::September => "Sep",
        Month::October => "Oct",
        Month::November => "Nov",
        Month::December => "Dec",
    }
}

#[cfg(test)]
#[path = "usage_calendar_tests.rs"]
mod tests;
