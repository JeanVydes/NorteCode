//! The transcript notices for a context compaction.

use crate::ui::Tone;

/// What the transcript says when compaction starts.
pub(crate) fn started(reason: impl std::fmt::Display) -> String {
    format!("compaction started · {reason}")
}

/// What the transcript says when compaction ends, and how loud it is.
pub(crate) fn finished(
    compacted: bool,
    summary: Option<&str>,
    error: Option<&str>,
) -> (String, Tone) {
    if let Some(error) = error {
        return (format!("compaction failed · {error}"), Tone::Error);
    }
    if compacted {
        let preview = summary
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .map(|summary| {
                let first = summary.lines().next().unwrap_or(summary);
                let clipped: String = first.chars().take(100).collect();
                format!(" · {clipped}")
            })
            .unwrap_or_default();
        return (format!("compacted{preview}"), Tone::Neutral);
    }
    ("compaction finished · no change".to_owned(), Tone::Neutral)
}

#[cfg(test)]
#[path = "compact_notice_tests.rs"]
mod tests;
