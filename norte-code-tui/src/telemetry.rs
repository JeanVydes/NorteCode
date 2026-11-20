//! Shared vocabulary for the telemetry charts.

/// The bucket size the telemetry charts group requests into.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChartInterval {
    /// One bucket per minute.
    Minute,
    /// One bucket per hour.
    #[default]
    Hour,
    /// One bucket per day.
    Day,
}

/// A short time label for the chart's x axis. Buckets are RFC 3339 prefixes;
/// minute and hour intervals show the clock time, days show month and day.
pub(crate) fn short_label(key: &str, interval: ChartInterval) -> String {
    match interval {
        ChartInterval::Minute => key.get(11..16).unwrap_or(key).to_owned(),
        ChartInterval::Hour => key
            .get(11..13)
            .map_or_else(|| key.to_owned(), |hour| format!("{hour}:00")),
        ChartInterval::Day => key.get(5..10).unwrap_or(key).to_owned(),
    }
}

impl ChartInterval {
    /// The label shown in the chart caption.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }

    /// The next interval in the cycle.
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Minute => Self::Hour,
            Self::Hour => Self::Day,
            Self::Day => Self::Minute,
        }
    }
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod tests;
