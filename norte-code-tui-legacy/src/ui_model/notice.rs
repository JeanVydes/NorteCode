//! The one-line message the footer shows.
//!
//! A notice has a severity, because a failed export and an imported session
//! should not look the same. Informative notices fade; an error or a link the
//! reader still needs stays until something replaces it.

use std::time::{Duration, Instant};

/// How serious a notice is, and therefore how long it stays.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Severity {
    /// Something happened that needs no action.
    Info,
    /// Something the reader should read but that is not a failure.
    Warning,
    /// Something failed.
    Error,
    /// A link or a path the reader has to copy, which must not vanish.
    Sticky,
}

impl Severity {
    /// The longest an informational notice stays before it leaves.
    const LIFETIME: Duration = Duration::from_secs(9);

    /// How long a notice of this severity stays, when it expires at all.
    const fn lifetime(self) -> Option<Duration> {
        match self {
            Self::Info | Self::Warning => Some(Self::LIFETIME),
            Self::Error | Self::Sticky => None,
        }
    }

    /// How notices of different severities rank when they compete for the line.
    const fn priority(self) -> u8 {
        match self {
            Self::Info => 0,
            Self::Warning => 1,
            Self::Sticky => 2,
            Self::Error => 3,
        }
    }
}

/// A one-line message with its severity and age.
#[derive(Clone, Debug)]
pub(crate) struct Notice {
    text: String,
    severity: Severity,
    since: Instant,
}

impl Notice {
    /// A notice that starts now.
    pub(crate) fn new(text: impl Into<String>, severity: Severity) -> Self {
        Self {
            text: text.into(),
            severity,
            since: Instant::now(),
        }
    }

    /// An ordinary notice.
    pub(crate) fn info(text: impl Into<String>) -> Self {
        Self::new(text, Severity::Info)
    }

    /// A failure.
    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self::new(text, Severity::Error)
    }

    /// Something the reader should read that is not a failure.
    pub(crate) fn warning(text: impl Into<String>) -> Self {
        Self::new(text, Severity::Warning)
    }

    /// A notice that stays until it is replaced or dismissed.
    pub(crate) fn sticky(text: impl Into<String>) -> Self {
        Self::new(text, Severity::Sticky)
    }

    /// What the notice says.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// How serious it is.
    pub(crate) const fn severity(&self) -> Severity {
        self.severity
    }

    /// Whether this notice deserves to replace the one showing.
    ///
    /// A newer notice of the same or higher severity wins; a lower one waits
    /// rather than hiding something the reader still has to read.
    pub(crate) fn overrides(&self, current: &Self) -> bool {
        self.severity.priority() >= current.severity.priority()
    }

    /// Whether the notice outlived its welcome.
    pub(crate) fn expired(&self) -> bool {
        self.severity
            .lifetime()
            .is_some_and(|lifetime| self.since.elapsed() >= lifetime)
    }

    /// How long the notice has been showing.
    pub(crate) fn age(&self) -> Duration {
        self.since.elapsed()
    }
}
