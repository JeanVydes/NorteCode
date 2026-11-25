//! Retry notices: a failure streak renders as a single updating line.

use std::sync::Arc;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use norte_code_client::config::RetentionConfig;
use norte_code_wire::{HistoryItem, HistoryItemKind, HistorySpeaker};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::Outcome;
use terminalui::components::console::Console;

use super::{Item, NOTICE_PREFIX_CHARS, Tone, Transcript};
use crate::ui::scroll_drag::Scrollbar;

fn painted_line(line: &str) -> Vec<String> {
    line.chars().map(|symbol| symbol.to_string()).collect()
}

fn buffer_text(buffer: &Buffer, area: Rect) -> String {
    (0..area.height)
        .flat_map(|y| {
            (0..area.width).map(move |x| buffer[(x, y)].symbol().chars().next().unwrap_or(' '))
        })
        .collect()
}

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
        kind: HistoryItemKind::User,
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
    transcript.painted = vec![painted_line(&line); 10];
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
fn consecutive_reasoning_deltas_merge_into_one_block() {
    let mut transcript = Transcript::new();
    transcript.push_reasoning_text("I should ");
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
fn reasoning_after_text_or_a_tool_starts_a_new_block() {
    let mut transcript = Transcript::new();
    transcript.push_reasoning_text("I should ");
    transcript.push_assistant_text("ok ", None);
    transcript.push(Item::Tool {
        call_id: "1".to_string(),
        name: "read".to_string(),
        target: None,
        state: super::ToolState::Done,
        expanded: false,
        meta: None,
        diff: None,
        output: None,
        error: None,
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
    assert_eq!(thoughts, vec!["I should ", "explore the repo."]);
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
    transcript.painted = vec![painted_line("▌ hello"), painted_line("world")];
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

fn history_item(id: &str, kind: HistoryItemKind, content: &str) -> norte_code_wire::HistoryItem {
    norte_code_wire::HistoryItem {
        id: id.to_string(),
        kind,
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
        kind: HistoryItemKind::User,
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
        history_item(
            "1",
            HistoryItemKind::ToolCall,
            "shell {\n  \"script\": \"echo hi\"\n}",
        ),
        history_item("2", HistoryItemKind::ToolResult, "hi"),
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
    assert!(!console.is_output_visible());
}

#[test]
fn shell_console_can_be_hidden_and_its_rows_expanded_separately() {
    let mut transcript = Transcript::new();
    let lines: Vec<String> = (0..12).map(|index| format!("row {index}")).collect();
    transcript.push(Item::Tool {
        call_id: "call-1".to_string(),
        name: "shell".to_string(),
        target: Some("echo hi".to_string()),
        state: super::ToolState::Running,
        expanded: false,
        meta: None,
        diff: None,
        output: None,
        error: None,
        console: Some(Box::new(Console::new("echo hi").set_lines(lines))),
    });
    assert!(!matches!(
        transcript.items.first(),
        Some(Item::Tool {
            console: Some(console),
            ..
        }) if console.is_expanded()
    ));

    assert!(transcript.toggle_console_at(2));
    assert!(matches!(
        transcript.items.first(),
        Some(Item::Tool {
            console: Some(console),
            ..
        }) if !console.is_output_visible()
    ));
    assert!(transcript.toggle_console_at(2));
    assert!(matches!(
        transcript.items.first(),
        Some(Item::Tool {
            console: Some(console),
            ..
        }) if console.is_output_visible() && !console.is_expanded()
    ));
    assert!(transcript.toggle_console_at(3));
    assert!(matches!(
        transcript.items.first(),
        Some(Item::Tool {
            console: Some(console),
            ..
        }) if console.is_expanded()
    ));
}

#[test]
fn expanded_shell_output_renders_every_retained_line() {
    use terminalui::{Theme, Tree, node};

    let mut transcript = Transcript::new();
    let lines: Vec<String> = (0..12).map(|index| format!("row {index}")).collect();
    transcript.push(Item::Tool {
        call_id: "call-1".to_string(),
        name: "shell".to_string(),
        target: Some("echo hi".to_string()),
        state: super::ToolState::Done,
        expanded: false,
        meta: None,
        diff: None,
        output: None,
        error: None,
        console: Some(Box::new(
            Console::new("echo hi").set_lines(lines).with_expanded(true),
        )),
    });

    let area = Rect::new(0, 0, 60, 16);
    let theme = Theme::default();
    let mut tree = Tree::new(node("stream", transcript));
    let mut buffer = Buffer::empty(area);
    tree.render(area, &mut buffer, &theme);
    let text = buffer_text(&buffer, area);
    assert!(text.contains("row 0"), "got: {text}");
    assert!(text.contains("row 11"), "got: {text}");
    assert!(text.contains("12 lines"), "got: {text}");
}

#[test]
fn long_shell_output_wraps_to_the_console_width() {
    use terminalui::{Theme, Tree, node};

    let mut transcript = Transcript::new();
    transcript.push(Item::Tool {
        call_id: "call-1".to_string(),
        name: "shell".to_string(),
        target: Some("echo long".to_string()),
        state: super::ToolState::Done,
        expanded: false,
        meta: None,
        diff: None,
        output: None,
        error: None,
        console: Some(Box::new(
            Console::new("echo long")
                .set_lines(vec!["abcdefghijklmnopqrstuvwxyz0123456789".into()]),
        )),
    });

    let area = Rect::new(0, 0, 24, 10);
    let mut tree = Tree::new(node("stream", transcript));
    let mut buffer = Buffer::empty(area);
    tree.render(area, &mut buffer, &Theme::default());
    let text = buffer_text(&buffer, area);
    assert!(text.contains("abcdefghijklmnopq"), "got: {text}");
    assert!(text.contains("rstuvwxyz01234567"), "got: {text}");
}

#[test]
fn shell_header_counts_new_output_as_it_arrives() {
    use terminalui::{Theme, Tree, node};

    let mut transcript = Transcript::new();
    transcript.push(Item::Tool {
        call_id: "call-1".to_string(),
        name: "shell".to_string(),
        target: Some("echo hi".to_string()),
        state: super::ToolState::Running,
        expanded: false,
        meta: None,
        diff: None,
        output: None,
        error: None,
        console: Some(Box::new(Console::new("echo hi"))),
    });

    let area = Rect::new(0, 0, 60, 8);
    let theme = Theme::default();
    let mut tree = Tree::new(node("stream", transcript));
    let mut buffer = Buffer::empty(area);
    tree.render(area, &mut buffer, &theme);
    let text = buffer_text(&buffer, area);
    assert!(text.contains("0 lines"), "got: {text}");

    assert!(
        tree.find_mut::<Transcript>("stream")
            .expect("stream")
            .append_console_output("call-1", "one", norte_code_wire::OutputStream::Stdout,)
    );
    let mut buffer = Buffer::empty(area);
    tree.render(area, &mut buffer, &theme);
    let text = buffer_text(&buffer, area);
    assert!(text.contains("1 lines"), "got: {text}");
}

#[test]
fn replayed_edit_calls_keep_their_diff() {
    let mut transcript = Transcript::new();
    transcript.replace_history(vec![norte_code_wire::HistoryItem {
        id: "call-1".to_string(),
        kind: HistoryItemKind::ToolCall,
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
fn tool_output_starts_collapsed_and_can_be_toggled() {
    let mut transcript = Transcript::new();
    transcript.push(Item::Tool {
        call_id: "read-1".to_owned(),
        name: "read".to_owned(),
        target: Some("src/main.rs".to_owned()),
        state: super::ToolState::Done,
        expanded: false,
        meta: None,
        diff: None,
        output: Some("file contents".to_owned()),
        error: None,
        console: None,
    });

    assert!(matches!(
        transcript.items.first(),
        Some(Item::Tool {
            expanded: false,
            ..
        })
    ));
    assert!(transcript.toggle_tool_at(0));
    assert!(matches!(
        transcript.items.first(),
        Some(Item::Tool { expanded: true, .. })
    ));
}

#[test]
fn replayed_tool_results_attach_to_their_call() {
    let mut transcript = Transcript::new();
    transcript.replace_history(vec![
        HistoryItem {
            id: "call-1".to_owned(),
            kind: HistoryItemKind::ToolCall,
            content: "read {\n  \"path\": \"src/main.rs\"\n}".to_owned(),
            created_at: 0,
            turn_id: None,
            role: HistorySpeaker::Tool,
            provenance: None,
        },
        HistoryItem {
            id: "result-1".to_owned(),
            kind: HistoryItemKind::ToolResult,
            content: "file contents".to_owned(),
            created_at: 1,
            turn_id: None,
            role: HistorySpeaker::Tool,
            provenance: None,
        },
    ]);

    assert!(matches!(
        transcript.items.first(),
        Some(Item::Tool { output: Some(output), expanded: false, .. }) if output == "file contents"
    ));
}

#[test]
fn verbose_activity_groups_reasoning_and_live_tools() {
    let mut transcript = Transcript::new();
    transcript.set_tool_preferences(false, true);
    transcript.start_thinking();
    transcript.push_reasoning_text("checking the project");
    transcript.push(Item::Tool {
        call_id: "shell-1".to_owned(),
        name: "shell".to_owned(),
        target: Some("cargo check".to_owned()),
        state: super::ToolState::Running,
        expanded: false,
        meta: None,
        diff: None,
        output: None,
        error: None,
        console: Some(Box::new(Console::new("cargo check"))),
    });

    let Some(Item::Activity {
        entries,
        expanded,
        status: super::ActivityStatus::Tool { name, context, .. },
        ..
    }) = transcript.items.first()
    else {
        panic!("verbose activity group");
    };
    assert_eq!(entries.len(), 2);
    assert!(expanded);
    assert_eq!(name, "shell");
    assert_eq!(context.as_deref(), Some("cargo check"));

    transcript.finish_tool("shell-1", false, None, None);
    transcript.finish_activity();
    assert!(matches!(
        transcript.items.first(),
        Some(Item::Activity {
            expanded: false,
            status: super::ActivityStatus::Complete,
            ..
        })
    ));
    assert!(transcript.toggle_tool_at(0));
    assert!(matches!(
        transcript.items.first(),
        Some(Item::Activity { expanded: true, .. })
    ));
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
    transcript.painted = vec![painted_line("hello world"), painted_line("second line")];
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
    transcript.painted = vec![painted_line("hello world")];
    transcript.selection = Some(super::Selection {
        anchor: super::Pos { row: 0, col: 0 },
        cur: super::Pos { row: 0, col: 5 },
    });
    transcript.finish_selection();
    assert_eq!(clipboard.written(), vec!["hello"]);
    assert_eq!(
        transcript.take_toasts(),
        vec![super::ToastRequest {
            text: "selection copied (5 characters)".to_owned(),
            tone: Tone::Neutral,
        }]
    );
    assert!(transcript.items.is_empty());
}

#[test]
fn reversed_selection_extracts_from_the_visual_start() {
    let mut transcript = Transcript::new();
    transcript.painted = vec![painted_line("hello world")];
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
    transcript.painted = vec![painted_line("line")];
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
        painted_line("hello"),
        painted_line("x notice row"),
        painted_line("world"),
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
    transcript.painted.push(painted_line("hello"));
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
    let retention = RetentionConfig::default();
    let mut transcript = Transcript::new();
    for index in 0..retention.transcript_max_items + 25 {
        transcript.push(Item::Notice {
            text: format!("line {index}"),
            tone: Tone::Neutral,
        });
    }

    let count = transcript.items.len();
    assert!(
        (retention.transcript_keep_items..=retention.transcript_max_items).contains(&count),
        "count {count}"
    );
    let last = transcript.items.last().expect("last item");
    assert!(matches!(
        last,
        Item::Notice { text, .. }
            if *text == format!("line {}", retention.transcript_max_items + 24)
    ));
}

#[test]
fn a_long_session_trims_at_the_configured_limits() {
    let retention = RetentionConfig {
        transcript_max_items: 12,
        transcript_keep_items: 4,
        ..RetentionConfig::default()
    };
    let mut transcript = Transcript::with_retention(retention);
    for index in 0..20 {
        transcript.push(Item::Notice {
            text: format!("line {index}"),
            tone: Tone::Neutral,
        });
    }

    assert_eq!(transcript.items.len(), 4);
    let texts: Vec<_> = transcript
        .items
        .iter()
        .map(|item| match item {
            Item::Notice { text, .. } => text.as_str(),
            _ => "unexpected item",
        })
        .collect();
    assert_eq!(texts, ["line 16", "line 17", "line 18", "line 19"]);
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
    let retention = RetentionConfig::default();
    let mut transcript = Transcript::new();
    let items = (0..retention.transcript_max_items + 25)
        .map(|index| history_item(&index.to_string(), HistoryItemKind::User, "hello"))
        .collect();
    transcript.replace_history(items);

    let count = transcript.items.len();
    assert!(
        (retention.transcript_keep_items..=retention.transcript_max_items).contains(&count),
        "count {count}"
    );
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

#[test]
fn scrolling_into_a_long_item_shows_its_middle_rows_once() {
    use terminalui::{Theme, Tree, node};

    let mut transcript = Transcript::new();
    let text = (0..20)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    transcript.push(Item::Notice {
        text,
        tone: Tone::Neutral,
    });
    transcript.push(Item::Notice {
        text: "FOLLOWER".to_owned(),
        tone: Tone::Warn,
    });

    let area = Rect::new(0, 0, 60, 8);
    let theme = Theme::default();
    let mut tree = Tree::new(node("stream", transcript));
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);

    {
        let stream = tree.find_mut::<Transcript>("stream").expect("stream");
        stream.follow_tail = false;
        stream.first_visible = 16;
    }
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let lines: Vec<String> = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect();

    assert!(lines[0].contains("line 16"), "got: {lines:?}");
    assert!(lines[1].contains("line 17"), "got: {lines:?}");
    assert!(lines[5].contains("FOLLOWER"), "got: {lines:?}");
    assert!(
        !lines.iter().any(|line| line.contains("line 0")),
        "scrolled-off row stayed pinned: {lines:?}"
    );
}

#[test]
fn the_thinking_row_does_not_cover_items_when_scrolled_up() {
    use terminalui::{Theme, Tree, node};

    let mut transcript = Transcript::new();
    for index in 0..40 {
        transcript.push(Item::Notice {
            text: format!("row {index}"),
            tone: Tone::Neutral,
        });
    }
    transcript.start_thinking();

    let area = Rect::new(0, 0, 60, 6);
    let theme = Theme::default();
    let mut tree = Tree::new(node("stream", transcript));
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let bottom_text = buffer_text(&buf, area);
    assert!(bottom_text.contains("thinking"), "got: {bottom_text}");

    {
        let stream = tree.find_mut::<Transcript>("stream").expect("stream");
        stream.follow_tail = false;
        stream.first_visible = 0;
    }
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let top_text = buffer_text(&buf, area);
    assert!(!top_text.contains("thinking"), "got: {top_text}");
    assert!(top_text.contains("row 0"), "got: {top_text}");
}

#[test]
fn an_oversized_block_height_saturates() {
    let item = Item::User {
        text: String::new(),
        turn: None,
    };

    assert_eq!(
        Transcript::item_height(
            &item,
            80,
            crate::diff::DiffMode::Unified,
            Some(usize::from(u16::MAX))
        ),
        u16::MAX
    );
}
