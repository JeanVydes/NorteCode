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
fn reasoning_deltas_merge_into_one_block_per_turn() {
    let mut transcript = Transcript::new();
    transcript.push(Item::User {
        text: "run several tools".to_string(),
    });
    transcript.push_reasoning_text("I should ");
    transcript.push_assistant_text("ok ");
    transcript.push(Item::Tool {
        call_id: "1".to_string(),
        name: "read".to_string(),
        target: None,
        state: super::ToolState::Done,
        meta: None,
    });
    transcript.push_reasoning_text("explore the repo.");
    let thoughts: Vec<_> = transcript
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Reasoning { text, .. } = item {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(thoughts, vec!["I should explore the repo."]);
}

#[test]
fn reasoning_in_a_new_turn_gets_its_own_block() {
    let mut transcript = Transcript::new();
    transcript.push_reasoning_text("first turn");
    transcript.push(Item::User {
        text: "next".to_string(),
    });
    transcript.push_reasoning_text("second turn");
    let thoughts: Vec<_> = transcript
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Reasoning { text, .. } = item {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(thoughts, vec!["first turn", "second turn"]);
}

#[test]
fn split_diff_paints_both_sides_in_one_row() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use terminalui::Theme;

    let theme = Theme {
        background: Color::Reset,
        foreground: Color::Reset,
        primary: Color::Reset,
        secondary: Color::Reset,
        muted: Color::Reset,
        accent: Color::Reset,
        destructive: Color::Red,
        success: Color::Green,
        warning: Color::Reset,
        border: Color::Reset,
        ring: Color::Reset,
        border_set: Color::Reset,
    };
    let diff = crate::diff::Diff {
        removed: vec!["old".to_string()],
        added: vec!["new".to_string()],
    };
    let area = Rect::new(0, 0, 40, 4);
    let mut buf = Buffer::empty(area);
    super::paint_diff(
        &mut buf,
        area,
        &diff,
        crate::diff::DiffMode::Split,
        4,
        &theme,
    );
    let painted: Vec<char> = (0..40)
        .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
        .collect();
    let text: String = painted.into_iter().collect();
    assert!(text.trim_start().starts_with("−"), "{text:?}");
    assert!(text.contains("new"), "{text:?}");
    assert_eq!(
        diff.rows(crate::diff::DiffMode::Split),
        2,
        "one diff row plus its header"
    );
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
fn selection_extracts_the_painted_cells() {
    let mut transcript = Transcript::new();
    transcript.painted = vec!["hello world".to_string(), "second line".to_string()];
    assert_eq!(
        transcript
            .selection_text(super::Selection {
                anchor: super::Pos { row: 0, col: 0 },
                cur: super::Pos { row: 1, col: 5 },
            })
            .as_deref(),
        Some("hello world\nsecond")
    );
}

#[test]
fn reversed_selection_extracts_from_the_visual_start() {
    let mut transcript = Transcript::new();
    transcript.painted = vec!["hello world".to_string()];
    assert_eq!(
        transcript
            .selection_text(super::Selection {
                anchor: super::Pos { row: 0, col: 10 },
                cur: super::Pos { row: 0, col: 6 },
            })
            .as_deref(),
        Some("world")
    );
}

#[test]
fn a_degenerate_selection_copies_nothing() {
    let mut transcript = Transcript::new();
    transcript.painted = vec!["line".to_string()];
    assert_eq!(
        transcript.selection_text(super::Selection {
            anchor: super::Pos { row: 0, col: 2 },
            cur: super::Pos { row: 0, col: 2 },
        }),
        None
    );
}

#[test]
fn notice_rows_are_excluded_from_the_selection() {
    let mut transcript = Transcript::new();
    transcript.painted = vec![
        "hello".to_string(),
        "x notice row".to_string(),
        "world".to_string(),
    ];
    transcript.painted_copyable = vec![true, false, true];
    assert_eq!(
        transcript
            .selection_text(super::Selection {
                anchor: super::Pos { row: 0, col: 0 },
                cur: super::Pos { row: 2, col: 4 },
            })
            .as_deref(),
        Some("hello\nworld")
    );
}

#[test]
fn painted_flags_mark_notice_rows_as_not_copyable() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    let mut transcript = Transcript::new();
    transcript.push(Item::User {
        text: "hi".to_string(),
    });
    transcript.push(Item::Notice {
        text: "a host notice".to_string(),
        tone: Tone::Neutral,
    });
    transcript.area = Rect::new(0, 0, 20, 6);
    transcript.first_visible = 0;
    transcript.view_height = 6;
    transcript.compute_heights(20);
    transcript.first_visible = 0;
    transcript.view_height = 6;
    let buffer = Buffer::empty(Rect::new(0, 0, 20, 6));
    transcript.capture_painted(Rect::new(0, 0, 20, 6), &buffer);
    assert_eq!(transcript.heights, vec![2, 2]);
    assert_eq!(
        transcript.painted_copyable,
        vec![true, true, false, false, false, false]
    );
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
