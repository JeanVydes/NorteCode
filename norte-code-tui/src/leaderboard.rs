//! Rankings derived from the shared telemetry report.

use std::collections::BTreeSet;

use norte_code_wire::TelemetryRecord;

/// What the ranking orders by.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LeaderboardGroup {
    #[default]
    Usage,
    Cost,
    Input,
    Output,
    Latency,
    FirstOutput,
    FirstText,
    CacheRate,
}

impl LeaderboardGroup {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Cost => "cost",
            Self::Input => "input tokens",
            Self::Output => "output tokens",
            Self::Latency => "latency",
            Self::FirstOutput => "first output",
            Self::FirstText => "first text",
            Self::CacheRate => "cache rate",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Usage => Self::Input,
            Self::Input => Self::Output,
            Self::Output => Self::Cost,
            Self::Cost => Self::Latency,
            Self::Latency => Self::FirstOutput,
            Self::FirstOutput => Self::FirstText,
            Self::FirstText => Self::CacheRate,
            Self::CacheRate => Self::Usage,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Usage => Self::CacheRate,
            Self::Input => Self::Usage,
            Self::Output => Self::Input,
            Self::Cost => Self::Output,
            Self::Latency => Self::Cost,
            Self::FirstOutput => Self::Latency,
            Self::FirstText => Self::FirstOutput,
            Self::CacheRate => Self::FirstText,
        }
    }
}

/// The entity each row represents.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LeaderboardSubject {
    #[default]
    Model,
    Provider,
    Session,
}

impl LeaderboardSubject {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Model => "models",
            Self::Provider => "providers",
            Self::Session => "sessions",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Model => Self::Provider,
            Self::Provider => Self::Session,
            Self::Session => Self::Model,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Model => Self::Session,
            Self::Provider => Self::Model,
            Self::Session => Self::Provider,
        }
    }
}

/// One ranked entity.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LeaderboardRow {
    pub(crate) name: String,
    pub(crate) requests: u64,
    pub(crate) turns: u64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    input_samples: u64,
    output_samples: u64,
    pub(crate) cost_usd: f64,
    pub(crate) metric_value: f64,
    pub(crate) metric_samples: u64,
}

impl LeaderboardRow {
    pub(crate) fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// Each metric and subject pair shares one selection and one shared report.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Leaderboard {
    pub(crate) group: LeaderboardGroup,
    pub(crate) subject: LeaderboardSubject,
    pub(crate) rows: Vec<LeaderboardRow>,
}

#[derive(Default)]
struct Accumulator {
    requests: u64,
    turns: BTreeSet<(String, u64)>,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    duration_total: u64,
    duration_samples: u64,
    first_output_total: u64,
    first_output_samples: u64,
    first_text_total: u64,
    first_text_samples: u64,
    cache_read_tokens: u64,
    input_samples: u64,
    output_samples: u64,
}

impl Accumulator {
    fn record(&mut self, record: &TelemetryRecord) {
        self.requests = self.requests.saturating_add(1);
        self.turns.insert((record.session.clone(), record.turn));
        if let Some(input) = record.input_tokens {
            self.input_tokens = self.input_tokens.saturating_add(input);
            self.input_samples = self.input_samples.saturating_add(1);
            self.cache_read_tokens = self
                .cache_read_tokens
                .saturating_add(record.cache_read_tokens.unwrap_or(0));
        }
        if let Some(output) = record.output_tokens {
            self.output_tokens = self.output_tokens.saturating_add(output);
            self.output_samples = self.output_samples.saturating_add(1);
        }
        self.cost_usd += record.cost_usd.unwrap_or(0.0);
        if record
            .metadata
            .get("source")
            .and_then(serde_json::Value::as_str)
            != Some("history")
            && record.duration_ms > 0
        {
            self.duration_total = self.duration_total.saturating_add(record.duration_ms);
            self.duration_samples = self.duration_samples.saturating_add(1);
        }
        if let Some(first_output) = record.first_token_ms {
            self.first_output_total = self.first_output_total.saturating_add(first_output);
            self.first_output_samples = self.first_output_samples.saturating_add(1);
        }
        if let Some(first_text) = record.first_text_ms {
            self.first_text_total = self.first_text_total.saturating_add(first_text);
            self.first_text_samples = self.first_text_samples.saturating_add(1);
        }
    }

    fn into_row(
        self,
        name: String,
        group: LeaderboardGroup,
        subject: LeaderboardSubject,
    ) -> LeaderboardRow {
        let (metric_value, metric_samples) = match group {
            LeaderboardGroup::Usage => (
                if subject == LeaderboardSubject::Session {
                    metric_number(self.input_tokens.saturating_add(self.output_tokens))
                } else {
                    metric_number(self.requests)
                },
                self.requests,
            ),
            LeaderboardGroup::Cost => (self.cost_usd, self.requests),
            LeaderboardGroup::Input => (metric_number(self.input_tokens), self.input_samples),
            LeaderboardGroup::Output => (metric_number(self.output_tokens), self.output_samples),
            LeaderboardGroup::Latency => (
                average(self.duration_total, self.duration_samples),
                self.duration_samples,
            ),
            LeaderboardGroup::FirstOutput => (
                average(self.first_output_total, self.first_output_samples),
                self.first_output_samples,
            ),
            LeaderboardGroup::FirstText => (
                average(self.first_text_total, self.first_text_samples),
                self.first_text_samples,
            ),
            LeaderboardGroup::CacheRate => (
                if self.input_tokens > 0 {
                    (metric_number(self.cache_read_tokens) / metric_number(self.input_tokens))
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                },
                u64::from(self.input_tokens > 0),
            ),
        };
        LeaderboardRow {
            name,
            requests: self.requests,
            turns: u64::try_from(self.turns.len()).unwrap_or(u64::MAX),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            input_samples: self.input_samples,
            output_samples: self.output_samples,
            cost_usd: self.cost_usd,
            metric_value,
            metric_samples,
        }
    }
}

fn key(record: &TelemetryRecord, subject: LeaderboardSubject) -> &str {
    match subject {
        LeaderboardSubject::Model => record.resolved_model.as_deref().unwrap_or(&record.model),
        LeaderboardSubject::Provider => &record.provider,
        LeaderboardSubject::Session => &record.session,
    }
}

impl Leaderboard {
    /// Builds one ranking, capped at the ten largest entries.
    pub(crate) fn build(
        records: &[TelemetryRecord],
        group: LeaderboardGroup,
        subject: LeaderboardSubject,
    ) -> Self {
        let mut totals = std::collections::BTreeMap::<String, Accumulator>::new();
        for record in records {
            let entry = totals.entry(key(record, subject).to_owned()).or_default();
            entry.record(record);
        }
        let mut rows: Vec<LeaderboardRow> = totals
            .into_iter()
            .map(|(name, value)| value.into_row(name, group, subject))
            .collect();
        rows.sort_by(|left, right| {
            let order = match group {
                LeaderboardGroup::Usage if subject == LeaderboardSubject::Session => {
                    right.total_tokens().cmp(&left.total_tokens())
                }
                LeaderboardGroup::Usage => right.requests.cmp(&left.requests),
                LeaderboardGroup::Cost => right
                    .cost_usd
                    .total_cmp(&left.cost_usd)
                    .then_with(|| right.requests.cmp(&left.requests)),
                LeaderboardGroup::Input
                | LeaderboardGroup::Output
                | LeaderboardGroup::CacheRate => right.metric_value.total_cmp(&left.metric_value),
                LeaderboardGroup::Latency
                | LeaderboardGroup::FirstOutput
                | LeaderboardGroup::FirstText => {
                    match (left.metric_samples == 0, right.metric_samples == 0) {
                        (true, false) => std::cmp::Ordering::Greater,
                        (false, true) => std::cmp::Ordering::Less,
                        _ => left.metric_value.total_cmp(&right.metric_value),
                    }
                }
            };
            order.then_with(|| left.name.cmp(&right.name))
        });
        rows.truncate(10);
        Self {
            group,
            subject,
            rows,
        }
    }
}

fn average(total: u64, samples: u64) -> f64 {
    if samples == 0 {
        f64::INFINITY
    } else {
        metric_number(total) / metric_number(samples)
    }
}

fn metric_number(value: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let value = value as f64;
    value
}

#[cfg(test)]
#[path = "leaderboard_tests.rs"]
mod tests;
