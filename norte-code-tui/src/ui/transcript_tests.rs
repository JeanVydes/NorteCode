//! Retry notices: a failure streak renders as a single updating line.

use std::sync::Arc;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use norte_code_wire::{HistoryItem, HistorySpeaker};
use ratatui::layout::Rect;
use terminalui::Outcome;

use super::{Item, KEEP_ITEMS, MAX_ITEMS, NOTICE_PREFIX_CHARS, Tone, Transcript};
use crate::ui::scroll_drag::Scrollbar;

fn scrollable_transcript() -> Transcript {
    let mut transcript = Transcript::new();
    for index in 0..50 {
        transcript.push(Item::Notice {
            text: format!("line {index}"),
            tone: Tone::Neutral,
        });
    }
    transcript.area = Rect::new(0, 0, 40, 10);
    transcript.view_height = 10;
    transcript.content_height = 100;
    transcript.first_visible = 0;
    transcript.scrollbar = Some(Scrollbar::new(39, 0, 10, 0, 1));
    transcript
}

#[test]
fn replayed_history_keeps_upload_markers_in_the_user_text() {
    let mut transcript = Transcript::new();
    transcript.replace_history(vec![HistoryItem {
        id: "h1".to_owned(),
        kind: "user".to_owned(),
        content: "see [Image #1] and [File #1]".to_owned(),
        created_at: 0,
        turn_id: None,
        role: HistorySpeaker::User,
        provenance: None,
    }]);
    assert!(matches!(
        transcript.items.as_slice(),
        [Item::User { text, .. }] if text == "see [Image #1] and [File #1]"
    ));
}

#[test]
fn a_long_notice_height_matches_the_width_it_is_painted_at() {
    let text = "unsupported media type `application/x-rare-format`";
    let width = 48usize;
    let height = Transcript::item_height(
        &Item::Notice {
            text: text.to_string(),
            tone: Tone::Warn,
        },
        width,
        crate::diff::DiffMode::Unified,
        None,
    );
    let text_width = width.saturating_sub(2 + NOTICE_PREFIX_CHARS).max(1);
    let lines = super::wrap(text, text_width).len();
    assert_eq!(usize::from(height), lines + 1);
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }
}

#[test]
fn clicking_the_scrollbar_track_pages_the_stream() {
    let mut transcript = scrollable_transcript();
    let outcome = transcript.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 39, 8));
    assert_eq!(outcome, Outcome::Consumed);
    assert_eq!(transcript.first_visible, 10);
    assert!(!transcript.dragging_thumb);
}

#[test]
fn dragging_the_scrollbar_thumb_moves_the_offset() {
    let mut transcript = scrollable_transcript();
    transcript.scrollbar = Some(Scrollbar::new(39, 0, 10, 0, 2));
    let _ = transcript.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 39, 0));
    assert!(transcript.dragging_thumb);
    let _ = transcript.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 39, 6));
    let _ = transcript.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 39, 6));
    assert!(!transcript.dragging_thumb);
    assert!(transcript.first_visible > 0);
}

#[test]
fn a_selection_never_includes_the_scrollbar_column() {
    let mut transcript = scrollable_transcript();
    let mut line = String::from("hello");
    line.push_str(&" ".repeat(34));
    line.push('░');
    transcript.painted = vec![line; 10];
    transcript.painted_copyable = vec![true; 10];
    transcript.painted_user = vec![false; 10];
    let text = transcript
        .selection_text(super::Selection {
            anchor: super::Pos { row: 0, col: 0 },
            cur: super::Pos { row: 0, col: 39 },
        })
        .expect("text");
    assert_eq!(text, "hello");
}

#[test]
fn clicking_a_message_opens_the_menu_and_queues_actions() {
    let mut transcript = Transcript::new();
    transcript.area = Rect::new(0, 0, 40, 10);
    transcript.push(Item::User {
        turn: Some(3),
        text: "hello".to_string(),
    });
    transcript.heights = vec![2];
    transcript.row_map = vec![Some(0), Some(0)];
    transcript.first_visible = 0;

    transcript.open_menu(30, 0);
    assert!(transcript.menu.is_some());

    transcript.activate_menu(0);
    assert_eq!(
        transcript.take_message_actions(),
        vec![super::MessageAction::Fork { turn: Some(3) }]
    );

    transcript.open_menu(30, 0);
    transcript.activate_menu(2);
    assert_eq!(
        transcript.take_message_actions(),
        vec![super::MessageAction::Rewind { turn: Some(3) }]
    );
}

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
        turn: None,
        text: "run several tools".to_string(),
    });
    transcript.push_reasoning_text("I should ");
    transcript.push_assistant_text("ok ", None);
    transcript.push(Item::Tool {
        call_id: "1".to_string(),
        name: "read".to_string(),
        target: None,
        state: super::ToolState::Done,
        meta: None,
        diff: None,
        console: None,
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
        turn: None,
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
fn selection_omits_the_user_bar() {
    let mut transcript = Transcript::new();
    transcript.painted = vec!["▌ hello".to_string(), "world".to_string()];
    transcript.painted_user = vec![true, false];
    let text = transcript
        .selection_text(super::Selection {
            anchor: super::Pos { row: 0, col: 0 },
            cur: super::Pos { row: 1, col: 4 },
        })
        .expect("text");
    assert_eq!(text, "hello\nworld");
}

#[test]
fn wrapping_counts_cells_not_bytes() {
    let lines = super::wrap("área énfasis Ópera", 12);
    assert!(lines.iter().all(|line| super::display_width(line) <= 12));
    assert_eq!(
        lines.concat().replace(' ', ""),
        "áreaénfasisÓpera".replace(' ', "")
    );
}

#[test]
fn wrapping_keeps_explicit_newlines() {
    assert_eq!(super::wrap("uno\ndos", 10), vec!["uno", "dos"]);
    assert_eq!(super::wrap("hola\n", 10), vec!["hola", ""]);
}

#[test]
fn wrapping_splits_a_word_longer_than_the_line() {
    let lines = super::wrap("abcdefghijklmnop", 5);
    assert!(lines.iter().all(|line| super::display_width(line) <= 5));
    assert_eq!(lines.concat(), "abcdefghijklmnop");
}

fn history_item(id: &str, kind: &str, content: &str) -> norte_code_wire::HistoryItem {
    norte_code_wire::HistoryItem {
        id: id.to_string(),
        kind: kind.to_string(),
        content: content.to_string(),
        created_at: 0,
        turn_id: None,
        role: norte_code_wire::HistorySpeaker::Tool,
        provenance: None,
    }
}

#[test]
fn replayed_plugin_messages_show_their_origin() {
    let mut transcript = Transcript::new();
    transcript.replace_history(vec![HistoryItem {
        id: "message-1".to_owned(),
        kind: "user".to_owned(),
        content: "a screenshot".to_owned(),
        created_at: 0,
        turn_id: None,
        role: HistorySpeaker::User,
        provenance: Some(serde_json::json!({
            "plugin_id": "browser",
            "call_id": "call_1",
        })),
    }]);

    let Some(Item::User { text, .. }) = transcript.items.first() else {
        panic!("user row");
    };
    assert_eq!(text, "a screenshot\nfrom plugin browser");
}

#[test]
fn replayed_shell_calls_keep_the_command_and_its_result() {
    let mut transcript = Transcript::new();
    transcript.replace_history(vec![
        history_item("1", "tool_call", "shell {\n  \"script\": \"echo hi\"\n}"),
        history_item("2", "tool_result", "hi"),
    ]);
    let Some(Item::Tool {
        name,
        target,
        console,
        ..
    }) = transcript.items.first()
    else {
        panic!("tool row");
    };
    assert_eq!(name, "shell");
    assert_eq!(target.as_deref(), Some("echo hi"));
    let console = console.as_ref().expect("console");
    assert_eq!(console.lines(), &["hi".to_string()]);
}

#[test]
fn replayed_edit_calls_keep_their_diff() {
    let mut transcript = Transcript::new();
    transcript.replace_history(vec![norte_code_wire::HistoryItem {
        id: "call-1".to_string(),
        kind: "tool_call".to_string(),
        content: "edit {\n  \"path\": \"x.rs\",\n  \"edits\": [{ \"search\": \"old\", \"replace\": \"new\" }]\n}"
            .to_string(),
        created_at: 0,
        turn_id: None,
        role: norte_code_wire::HistorySpeaker::Tool,
        provenance: None,
    }]);
    let Some(Item::Tool { diff, target, .. }) = transcript.items.first() else {
        panic!("tool row");
    };
    assert_eq!(target.as_deref(), Some("x.rs"));
    let diff = diff.as_ref().expect("diff from the call arguments");
    assert_eq!(
        diff.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["old", "new"]
    );
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
        lines: vec![
            crate::diff::Line {
                kind: crate::diff::LineKind::Removed,
                text: "old".to_string(),
            },
            crate::diff::Line {
                kind: crate::diff::LineKind::Added,
                text: "new".to_string(),
            },
        ],
    };
    let area = Rect::new(0, 0, 40, 4);
    let mut buf = Buffer::empty(area);
    super::paint_diff(
        &mut buf,
        area,
        &diff,
        crate::diff::DiffMode::Split,
        "file.txt",
        4,
        &theme,
    );
    let painted: Vec<char> = (0..40)
        .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
        .collect();
    let text: String = painted.into_iter().collect();
    assert!(text.contains('−'), "{text:?}");
    assert!(text.contains("new"), "{text:?}");
    assert_eq!(diff.rows(crate::diff::DiffMode::Split), 1);
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
fn copying_a_transcript_selection_uses_the_shared_clipboard() {
    let clipboard = Arc::new(crate::clipboard::FakeClipboard::default());
    let mut transcript = Transcript::new();
    transcript.set_clipboard(clipboard.clone());
    transcript.painted = vec!["hello world".to_string()];
    transcript.selection = Some(super::Selection {
        anchor: super::Pos { row: 0, col: 0 },
        cur: super::Pos { row: 0, col: 5 },
    });
    transcript.finish_selection();
    assert_eq!(clipboard.written(), vec!["hello"]);
    assert_eq!(
        transcript.take_toasts(),
        vec![super::ToastRequest {
            text: "copied 5 chars to clipboard".to_owned(),
            tone: Tone::Neutral,
        }]
    );
    assert!(transcript.items.is_empty());
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
        turn: None,
        text: "hi".to_string(),
    });
    transcript.push(Item::Notice {
        text: "a host notice".to_string(),
        tone: Tone::Neutral,
    });
    transcript.area = Rect::new(0, 0, 20, 6);
    transcript.first_visible = 0;
    transcript.view_height = 6;
    transcript.compute_heights(20, terminalui::Theme::default());
    transcript.first_visible = 0;
    transcript.view_height = 6;
    let buffer = Buffer::empty(Rect::new(0, 0, 20, 6));
    let row_map = transcript.build_row_map();
    transcript.capture_painted(Rect::new(0, 0, 20, 6), &buffer, &row_map);
    assert_eq!(transcript.heights, vec![2, 3]);
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
        turn: None,
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

#[test]
fn the_painted_flags_do_not_accumulate_across_frames() {
    use ratatui::buffer::Buffer;

    let mut transcript = Transcript::new();
    let area = Rect::new(0, 0, 10, 2);
    let buffer = Buffer::empty(area);
    transcript.capture_painted(area, &buffer, &[]);
    transcript.capture_painted(area, &buffer, &[]);

    assert_eq!(transcript.painted.len(), 2);
    assert_eq!(transcript.painted_copyable.len(), 2);
    assert_eq!(transcript.painted_user.len(), 2);
}

#[test]
fn clearing_the_stream_drops_the_render_cache() {
    let mut transcript = Transcript::new();
    transcript.push(Item::User {
        turn: None,
        text: "**hello**".to_owned(),
    });
    transcript.docs.push(None);
    transcript.heights.push(2);
    transcript.painted.push("hello".to_owned());
    transcript.painted_copyable.push(true);
    transcript.painted_user.push(true);
    transcript.row_map.push(Some(0));

    transcript.clear_all();

    assert_eq!(transcript.docs.len(), 0);
    assert_eq!(transcript.heights.len(), 0);
    assert_eq!(transcript.painted.len(), 0);
    assert_eq!(transcript.painted_copyable.len(), 0);
    assert_eq!(transcript.painted_user.len(), 0);
    assert_eq!(transcript.row_map.len(), 0);
}

#[test]
fn a_long_session_keeps_only_recent_items() {
    let mut transcript = Transcript::new();
    for index in 0..MAX_ITEMS + 25 {
        transcript.push(Item::Notice {
            text: format!("line {index}"),
            tone: Tone::Neutral,
        });
    }

    let count = transcript.items.len();
    assert!((KEEP_ITEMS..=MAX_ITEMS).contains(&count), "count {count}");
    let last = transcript.items.last().expect("last item");
    assert!(matches!(last, Item::Notice { text, .. } if text == "line 1024"));
}

#[test]
fn hit_testing_follows_the_rendered_scroll_offset() {
    use ratatui::buffer::Buffer;
    use terminalui::{Tree, node};

    let mut transcript = Transcript::new();
    for index in 0..20 {
        transcript.push(Item::Assistant {
            turn: Some(index),
            text: format!("filler {index}"),
        });
    }
    transcript.push(Item::Assistant {
        turn: Some(99),
        text: "[docs](https://example.com/path)".to_owned(),
    });

    let area = Rect::new(0, 0, 60, 6);
    let theme = terminalui::Theme::default();
    let mut tree = Tree::new(node("stream", transcript));
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);

    let stream = tree.find::<Transcript>("stream").expect("stream");
    assert!(stream.first_visible > 0);
    let local_row = stream
        .row_map
        .iter()
        .position(|item| *item == Some(20))
        .expect("the linked message is visible");
    assert_eq!(stream.message_at(local_row), Some(20));
    assert_eq!(
        stream.link_at_screen(area.y + u16::try_from(local_row).unwrap_or(0), area.x + 1),
        Some("https://example.com/path")
    );
}

#[test]
fn replaying_a_long_history_is_bounded() {
    let mut transcript = Transcript::new();
    let items = (0..MAX_ITEMS + 25)
        .map(|index| history_item(&index.to_string(), "user", "hello"))
        .collect();
    transcript.replace_history(items);

    let count = transcript.items.len();
    assert!((KEEP_ITEMS..=MAX_ITEMS).contains(&count), "count {count}");
}

#[test]
fn user_and_model_text_use_the_active_theme_foreground() {
    use ratatui::buffer::Buffer;
    use ratatui::style::Style;
    use terminalui::{Tree, node};

    use crate::theme::{ThemeCatalog, ThemeChoice, ThemeKind};

    let mut transcript = Transcript::new();
    transcript.push(Item::User {
        turn: Some(1),
        text: "userword".to_owned(),
    });
    transcript.push(Item::Assistant {
        turn: Some(1),
        text: "modelword".to_owned(),
    });

    let catalog = ThemeCatalog::default();
    let theme = catalog.theme(&ThemeChoice::BuiltIn(ThemeKind::ByeRetina));
    let area = Rect::new(0, 0, 60, 8);
    let mut tree = Tree::new(node("stream", transcript));
    let mut buf = Buffer::empty(area);
    buf.set_style(area, Style::default().bg(theme.background));
    tree.render(area, &mut buf, &theme);

    for word in ["userword", "modelword"] {
        let first = &word[..1];
        let cell = buf
            .content
            .iter()
            .find(|cell| cell.symbol() == first)
            .unwrap_or_else(|| panic!("no painted cell for {word}"));
        assert_eq!(cell.fg, theme.foreground, "contrast for {word}");
    }
}
