//! Retry notices: a failure streak renders as a single updating line.

use super::{Item, Tone, Transcript};

#[test]
fn consecutive_retry_lines_update_the_same_item() {
    let mut transcript = Transcript::new();
    transcript.push_retry("retry 1 · overloaded".to_string(), Tone::Warn);
    transcript.push_retry("retry 1 running".to_string(), Tone::Neutral);
    transcript.push_retry("retry 2 · overloaded".to_string(), Tone::Warn);
    transcript.push_retry("retry 2 running".to_string(), Tone::Neutral);
    let retries: Vec<_> = transcript
        .items
        .iter()
        .filter(|item| matches!(item, Item::Retry { .. }))
        .collect();
    assert_eq!(retries.len(), 1);
    assert!(matches!(
        retries[0],
        Item::Retry { text, tone } if text == "retry 2 running" && *tone == Tone::Neutral
    ));
}

#[test]
fn reasoning_blocks_toggle_with_clicks() {
    let mut transcript = Transcript::new();
    transcript.push_reasoning_text("alpha beta gamma delta");
    assert!(transcript.toggle_reasoning_at(0));
    let expanded = transcript.items.iter().find_map(|item| {
        if let Item::Reasoning { expanded, .. } = item {
            Some(*expanded)
        } else {
            None
        }
    });
    assert_eq!(expanded, Some(true));
    assert!(transcript.toggle_reasoning_at(0));
    let expanded = transcript.items.iter().find_map(|item| {
        if let Item::Reasoning { expanded, .. } = item {
            Some(*expanded)
        } else {
            None
        }
    });
    assert_eq!(expanded, Some(false));
}

#[test]
fn non_reasoning_rows_are_not_toggleable() {
    let mut transcript = Transcript::new();
    transcript.push(Item::Notice {
        text: "hi".to_string(),
        tone: Tone::Neutral,
    });
    assert!(!transcript.toggle_reasoning_at(0));
}

#[test]
fn a_retry_after_other_items_starts_a_new_line() {
    let mut transcript = Transcript::new();
    transcript.push_retry("retry 1 · overloaded".to_string(), Tone::Warn);
    transcript.push(Item::Assistant {
        text: "partial reply".to_string(),
    });
    transcript.push_retry("retry 1 · overloaded again".to_string(), Tone::Warn);
    let retries = transcript
        .items
        .iter()
        .filter(|item| matches!(item, Item::Retry { .. }))
        .count();
    assert_eq!(retries, 2);
}
