//! Text and tone of the compaction notices.

use super::{finished, started};
use crate::ui::Tone;

#[test]
fn the_start_notice_names_the_reason() {
    assert_eq!(started("manual"), "compaction started · manual");
}

#[test]
fn a_successful_compact_shows_a_one_line_preview() {
    let (text, tone) = finished(true, Some("first line\nsecond line"), None);
    assert_eq!(text, "compacted · first line");
    assert_eq!(tone, Tone::Neutral);
}

#[test]
fn a_failed_compact_is_loud() {
    let (text, tone) = finished(false, None, Some("no room"));
    assert_eq!(text, "compaction failed · no room");
    assert_eq!(tone, Tone::Error);
}

#[test]
fn a_noop_compact_says_so() {
    let (text, tone) = finished(false, None, None);
    assert_eq!(text, "compaction finished · no change");
    assert_eq!(tone, Tone::Neutral);
}
