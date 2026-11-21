//! Rankings derived from the shared telemetry report.

use std::collections::BTreeSet;

use norte_code_wire::TelemetryRecord;

/// What the ranking orders by.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LeaderboardGroup {
    #[default]
    Usage,
    Cost,
}

impl LeaderboardGroup {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Cost => "cost",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Usage => Self::Cost,
            Self::Cost => Self::Usage,
        }
    }

    pub(crate) fn previous(self) -> Self {
        self.next()
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
    pub(crate) cost_usd: f64,
}

impl LeaderboardRow {
    pub(crate) fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// The six rankings share one selection and one shared report.
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
}

fn key(record: &TelemetryRecord, subject: LeaderboardSubject) -> &str {
    match subject {
        LeaderboardSubject::Model => &record.model,
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
        let mut totals: std::collections::BTreeMap<String, Accumulator> =
            std::collections::BTreeMap::new();
        for record in records {
            let entry = totals.entry(key(record, subject).to_owned()).or_default();
            entry.requests += 1;
            entry.turns.insert((record.session.clone(), record.turn));
            entry.input_tokens += record.input_tokens.unwrap_or(0);
            entry.output_tokens += record.output_tokens.unwrap_or(0);
            entry.cost_usd += record.cost_usd.unwrap_or(0.0);
        }
        let mut rows: Vec<LeaderboardRow> = totals
            .into_iter()
            .map(|(name, value)| LeaderboardRow {
                name,
                requests: value.requests,
                turns: u64::try_from(value.turns.len()).unwrap_or(u64::MAX),
                input_tokens: value.input_tokens,
                output_tokens: value.output_tokens,
                cost_usd: value.cost_usd,
            })
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

#[cfg(test)]
#[path = "leaderboard_tests.rs"]
mod tests;
