//! Tests for the renderer, the edit preview and the argument readers.

use std::fmt::Write as _;
use std::sync::Arc;

use norte_code_protocol::{Delta, ServerEvent, ServerEventKind, SessionKey};

use crate::input::{Pinned, TodoItem, TodoStatus};
use crate::render::argument::todo_items;
use crate::render::dial::context_dial;
use crate::render::edit::{EditPreview, build_edit_preview, edit_preview};
use crate::render::loading::{LOADING_BAR_WIDTH, Phase, loading_frame};
use crate::render::renderer::{Block, Format, Renderer, Verbosity, tool_sections};
use crate::style::Style;

fn renderer(pinned: &Arc<Pinned>) -> Renderer {
    Renderer::new(
        Format::Text,
        Verbosity::Low,
        false,
        true,
        Style::plain(),
        Arc::clone(pinned),
    )
}

fn visible_renderer(pinned: &Arc<Pinned>) -> Renderer {
    Renderer::new(
        Format::Text,
        Verbosity::Medium,
        false,
        true,
        Style::plain(),
        Arc::clone(pinned),
    )
}

/// A renderer at `high`, the only level that draws the reasoning gutter.
fn reasoning_renderer(pinned: &Arc<Pinned>) -> Renderer {
    let mut renderer = visible_renderer(pinned);
    renderer.verbosity = Verbosity::High;
    renderer
}

#[test]
fn the_footer_names_the_session_from_medium_up() {
    // The key was only printed by `--debug` at startup, so returning to a
    // session meant scrolling to the top to copy it.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.requests = 1;
    renderer.input = 1_000;
    renderer.output = 100;
    renderer
        .event(&ServerEvent {
            request_id: 1,
            session: Some(SessionKey::new(
                "801605a1-dcb8-4d7b-91c0-7b17440274b1".to_owned(),
            )),
            kind: ServerEventKind::TurnStarted { turn: 1 },
        })
        .expect("event");

    let status = renderer.session_status();
    assert!(
        status.contains("801605a1"),
        "the footer names the session: {status}"
    );

    // `low` shows the answer alone, footer included.
    renderer.verbosity = Verbosity::Low;
    let quiet = renderer.session_status();
    assert!(
        !quiet.contains("801605a1"),
        "low keeps the footer free of the key: {quiet}"
    );
}

#[test]
fn the_levels_are_ordered_and_parse_by_name() {
    assert!(Verbosity::High.at_least(Verbosity::Medium));
    assert!(Verbosity::Medium.at_least(Verbosity::Medium));
    assert!(!Verbosity::Low.at_least(Verbosity::Medium));
    assert!(Verbosity::High.at_least(Verbosity::Low));

    assert_eq!(Verbosity::parse("low").expect("low"), Verbosity::Low);
    assert_eq!(
        Verbosity::parse("medium").expect("medium"),
        Verbosity::Medium
    );
    assert_eq!(Verbosity::parse("high").expect("high"), Verbosity::High);
    let error = Verbosity::parse("loud").expect_err("unknown is refused");
    assert!(error.contains("low, medium or high"), "{error}");
}

#[test]
fn low_shows_no_reasoning_and_medium_does_not_show_the_thought() {
    let pinned = Pinned::new();
    // `low` is the answer alone: not even the fact that it is thinking.
    let mut low = visible_renderer(&pinned);
    low.verbosity = Verbosity::Low;
    low.delta(&Delta::Reasoning {
        index: 0,
        text: "a thought".to_owned(),
    });
    assert_eq!(low.block, Block::None, "low opens no reasoning block");

    // `medium` says that it is thinking, and the text stays hidden.
    let mut medium = visible_renderer(&pinned);
    medium.delta(&Delta::Reasoning {
        index: 0,
        text: "a thought".to_owned(),
    });
    assert_eq!(
        medium.block,
        Block::None,
        "medium announces the thought without drawing it"
    );

    // `high` draws the thought in its gutter.
    let mut high = reasoning_renderer(&pinned);
    high.delta(&Delta::Reasoning {
        index: 0,
        text: "a thought".to_owned(),
    });
    assert_eq!(high.block, Block::Reasoning, "high draws the reasoning");
}

fn todo_started(call_id: &str, todos: &serde_json::Value) -> ServerEvent {
    ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::ToolStarted {
            turn: 1,
            call_id: call_id.to_owned(),
            tool: "todo".to_owned(),
            arguments: Some(serde_json::json!({ "todos": todos })),
            source: None,
        },
    }
}

fn tool_finished(call_id: &str, tool: &str, is_error: bool) -> ServerEvent {
    ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::ToolFinished {
            turn: 1,
            call_id: call_id.to_owned(),
            tool: tool.to_owned(),
            is_error,
            error: None,
            output: None,
            truncated: false,
            duration_ms: 0,
        },
    }
}

#[test]
fn a_long_stdout_does_not_hide_stderr() {
    // The command text lists stdout before stderr, so one budget for the whole
    // block let a long stdout push the stderr out of view: a failing command
    // looked like it printed nothing wrong.
    let mut text = String::from("exit_status: 1\nduration_ms: 5\nstdout:\n");
    for line in 0..40 {
        let _ = writeln!(text, "out {line}");
    }
    text.push_str("stderr:\nboom\n");

    let sections = tool_sections(&text);
    let names: Vec<&str> = sections
        .iter()
        .filter_map(|section| section.name.as_deref())
        .collect();
    assert!(names.contains(&"stdout:"), "{names:?}");
    assert!(
        names.contains(&"stderr:"),
        "the stderr keeps its own section: {names:?}"
    );
    let stderr = sections
        .iter()
        .find(|section| section.name.as_deref() == Some("stderr:"))
        .expect("stderr section");
    assert_eq!(stderr.lines, vec!["boom".to_owned()]);
}

#[test]
fn a_summary_is_folded_away_and_a_plain_text_stays_one_section() {
    // The summary quotes lines already shown above it.
    let text = "stdout:\nhello\nsummary:\nhello\n";
    let sections = tool_sections(text);
    assert_eq!(sections.len(), 1, "{:?}", sections.len());
    assert_eq!(sections[0].name.as_deref(), Some("stdout:"));

    // A tool that writes no label still prints as one unnamed block.
    let plain = tool_sections("just text\nmore");
    assert_eq!(plain.len(), 1);
    assert!(plain[0].name.is_none());
    assert_eq!(
        plain[0].lines,
        vec!["just text".to_owned(), "more".to_owned()]
    );
}

#[test]
fn the_tool_result_preview_names_the_file_for_a_write_and_a_read() {
    // A write is about the file it wrote: the path leads and a glimpse of the
    // content follows. A read shows what came back. A tool with no result worth
    // showing has none, so the transcript stays about the work.
    let write = Renderer::tool_result("write", Some("src/lib.rs"), Some("fn main() {}"))
        .expect("a write has a preview");
    assert!(write.starts_with("src/lib.rs"), "{write}");
    assert!(write.contains("fn main() {}"), "{write}");

    let read = Renderer::tool_result("read", Some("Cargo.toml"), Some("12 lines"))
        .expect("a read has a preview");
    assert!(read.starts_with("Cargo.toml"), "{read}");

    let command =
        Renderer::tool_result("shell", None, Some("ok")).expect("a shell shows its output");
    assert_eq!(command, "ok");

    assert!(
        Renderer::tool_result("todo", None, Some("done")).is_none(),
        "a tool with nothing to show gets no preview"
    );
    assert!(
        Renderer::tool_result("write", None, Some("x")).is_none(),
        "a path is what makes a write's preview useful"
    );
}

#[test]
fn a_streamed_call_is_remembered_so_its_finish_does_not_repeat_it() {
    // A shell streams its output while it runs; the finish event carries the
    // same text. Without the record the transcript would show it twice, once as
    // it arrived and once in the result block right after.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.tool_event(&ServerEventKind::ToolOutput {
        turn: 1,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        stream: "stdout".to_owned(),
        text: "building…\n".to_owned(),
        last: false,
    });
    assert_eq!(renderer.streamed_calls(), ["call-1"]);

    // A chunk of a call that never streamed is not recorded, so its result is
    // still printed: an empty chunk is not output and must not claim otherwise.
    renderer.tool_event(&ServerEventKind::ToolOutput {
        turn: 1,
        call_id: "call-2".to_owned(),
        tool: "shell".to_owned(),
        stream: "stderr".to_owned(),
        text: String::new(),
        last: true,
    });
    assert_eq!(
        renderer.streamed_calls(),
        ["call-1"],
        "an empty chunk is not output"
    );
}

fn item(content: &str, status: TodoStatus) -> TodoItem {
    TodoItem {
        content: content.to_owned(),
        status,
    }
}

#[test]
fn an_empty_text_delta_does_not_leave_the_reasoning_block() {
    let pinned = Pinned::new();
    let mut renderer = reasoning_renderer(&pinned);
    renderer.delta(&Delta::Reasoning {
        index: 0,
        text: "thinking".to_owned(),
    });
    assert_eq!(renderer.block, Block::Reasoning);
    // A keepalive or the stream's trailing empty part must not close the
    // gutter: doing so split one thought into several `┌ thinking` blocks.
    renderer.delta(&Delta::Text {
        index: 0,
        text: String::new(),
    });
    assert_eq!(renderer.block, Block::Reasoning);
}

#[test]
fn an_empty_reasoning_delta_does_not_open_the_gutter() {
    let pinned = Pinned::new();
    let mut renderer = reasoning_renderer(&pinned);
    renderer.delta(&Delta::Reasoning {
        index: 0,
        text: String::new(),
    });
    assert_eq!(renderer.block, Block::None);
}

#[test]
fn a_text_delta_after_reasoning_closes_the_gutter() {
    let pinned = Pinned::new();
    let mut renderer = reasoning_renderer(&pinned);
    renderer.delta(&Delta::Reasoning {
        index: 0,
        text: "thinking".to_owned(),
    });
    renderer.delta(&Delta::Text {
        index: 1,
        text: "answer".to_owned(),
    });
    assert_eq!(renderer.block, Block::Answer);
}

#[test]
fn the_bar_comes_back_after_a_tool() {
    // The bar stops on the first answer token, which is right; a turn that
    // called a tool then goes on, and it used to do so with no signal at all.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.mark_working();
    assert!(
        renderer.loading_bar.is_some(),
        "the bar starts with the turn"
    );

    // The model answers with a call, which stops the bar like any answer.
    renderer.delta(&Delta::Text {
        index: 0,
        text: "calling a tool".to_owned(),
    });
    renderer.end_blocks();
    assert!(
        renderer.loading_bar.is_none(),
        "the answer stops the bar, as before"
    );

    // The tool finishes; the turn is not over, so the bar returns.
    let finished = tool_finished("call-1", "write", false);
    renderer.tool_event(&finished.kind);
    assert!(
        renderer.loading_bar.is_some(),
        "the bar is back for what the model says next"
    );
}

#[test]
fn the_frame_shape_tells_the_two_phases_apart() {
    // While waiting the highlight travels; while streaming it fills from the
    // left, so the shape says which one it is without reading the label.
    let waiting = loading_frame(Style::plain(), 0, Phase::Waiting);
    let streaming = loading_frame(Style::plain(), 0, Phase::Streaming);
    assert_ne!(waiting, streaming, "the two phases do not look alike");
    // Streaming is a fill: the first cell is painted and the last is not at
    // tick zero.
    assert!(streaming.starts_with('█'), "{streaming:?}");
    assert!(streaming.ends_with('░'), "{streaming:?}");
}

#[test]
fn loading_frame_is_legible_without_colour() {
    let frame = loading_frame(Style::plain(), 3, Phase::Waiting);
    assert_eq!(frame.chars().count(), LOADING_BAR_WIDTH);
    assert!(
        !frame.contains('\u{1b}'),
        "plain style must not emit escapes"
    );
}

#[test]
fn loading_frame_moves_with_the_tick() {
    assert_ne!(
        loading_frame(Style::plain(), 0, Phase::Waiting),
        loading_frame(Style::plain(), 2, Phase::Waiting)
    );
}

#[test]
fn a_cost_silent_provider_hides_the_missing_cost() {
    let pinned = Pinned::new();
    let mut renderer = renderer(&pinned);
    renderer.requests = 1;
    renderer.unpriced = 1;
    assert!(renderer.session_status().contains("cost not reported"));
    renderer.set_cost_silent(true);
    assert!(!renderer.session_status().contains("not reported"));
}

#[test]
fn the_context_dial_warns_as_the_window_fills() {
    let style = Style::plain();
    // A quarter full is neither near nor over the limit.
    let low = context_dial(style, 25_000, Some(100_000), 0.8, false);
    assert!(low.contains("25%"), "{low}");
    assert!(!low.contains("compact"), "{low}");
    // Past the threshold the dial says so.
    let over = context_dial(style, 90_000, Some(100_000), 0.8, false);
    assert!(over.contains("compact"), "{over}");
}

#[test]
fn the_context_dial_without_a_window_shows_only_the_count() {
    let dial = context_dial(Style::plain(), 12_000, None, 0.8, false);
    assert_eq!(dial, "ctx 12k");
}

#[test]
fn a_successful_todo_call_updates_the_footer() {
    let pinned = Pinned::new();
    let mut renderer = renderer(&pinned);
    renderer
        .event(&todo_started(
            "call-1",
            &serde_json::json!([
                {"content": "inspect", "status": "completed"},
                {"content": "implement", "status": "in_progress"}
            ]),
        ))
        .expect("started");
    assert!(pinned.todos().is_empty());
    renderer
        .event(&tool_finished("call-1", "todo", false))
        .expect("finished");
    assert_eq!(
        pinned.todos(),
        vec![
            item("inspect", TodoStatus::Completed),
            item("implement", TodoStatus::InProgress)
        ]
    );
}

#[test]
fn a_failed_todo_call_leaves_the_footer_unchanged() {
    let pinned = Pinned::new();
    let mut renderer = renderer(&pinned);
    renderer
        .event(&todo_started(
            "call-1",
            &serde_json::json!([{"content": "keep", "status": "pending"}]),
        ))
        .expect("started");
    renderer
        .event(&tool_finished("call-1", "todo", false))
        .expect("finished");
    renderer
        .event(&todo_started(
            "call-2",
            &serde_json::json!([{"content": "discard", "status": "pending"}]),
        ))
        .expect("started");
    renderer
        .event(&tool_finished("call-2", "todo", true))
        .expect("failed");
    assert_eq!(pinned.todos(), vec![item("keep", TodoStatus::Pending)]);
}

#[test]
fn an_empty_todo_list_clears_the_footer() {
    let pinned = Pinned::new();
    let mut renderer = renderer(&pinned);
    renderer
        .event(&todo_started(
            "call-1",
            &serde_json::json!([{"content": "one", "status": "pending"}]),
        ))
        .expect("started");
    renderer
        .event(&tool_finished("call-1", "todo", false))
        .expect("finished");
    renderer
        .event(&todo_started("call-2", &serde_json::json!([])))
        .expect("started");
    renderer
        .event(&tool_finished("call-2", "todo", false))
        .expect("finished");
    assert!(pinned.todos().is_empty());
}

#[test]
fn a_non_todo_call_does_not_touch_the_footer() {
    let pinned = Pinned::new();
    let mut renderer = renderer(&pinned);
    renderer
        .event(&todo_started(
            "call-1",
            &serde_json::json!([{"content": "not a todo", "status": "pending"}]),
        ))
        .expect("started");
    renderer
        .event(&tool_finished("call-1", "write", false))
        .expect("finished");
    assert!(pinned.todos().is_empty());
}

#[test]
fn malformed_todo_arguments_are_ignored() {
    assert!(todo_items(None).is_none());
    assert!(todo_items(Some(&serde_json::json!({}))).is_none());
    assert!(todo_items(Some(&serde_json::json!({"todos": "nope"}))).is_none());
    assert!(
        todo_items(Some(
            &serde_json::json!({"todos": [{"content": "x", "status": "unknown"}]})
        ))
        .is_none()
    );
    assert!(todo_items(Some(&serde_json::json!({"todos": [{"status": "pending"}]}))).is_none());
    assert_eq!(
        todo_items(Some(
            &serde_json::json!({"todos": [{"content": "x", "status": "pending"}]})
        )),
        Some(vec![item("x", TodoStatus::Pending)])
    );
}

fn preview(source: &str, edits: &serde_json::Value) -> EditPreview {
    let arguments = serde_json::json!({ "edits": edits });
    build_edit_preview(
        "src/lib.rs".to_owned(),
        arguments["edits"].as_array().expect("edits"),
        Some(source.to_owned()),
    )
}

#[test]
fn edit_preview_shows_context_and_line_numbers() {
    let source =
        "fn main() {\n    let one = 1;\n    let two = 2;\n    old();\n    let three = 3;\n}\n";
    let preview = preview(
        source,
        &serde_json::json!([{
            "search": "    old();\n",
            "replace": "    new();\n"
        }]),
    );
    let edit = &preview.boxes[0];
    assert_eq!(edit.old_range, Some((4, 4)));
    assert_eq!(edit.new_range, Some((4, 4)));
    assert_eq!(
        edit.rows
            .iter()
            .map(|row| (
                row.left.as_ref().and_then(|cell| cell.number),
                row.right.as_ref().and_then(|cell| cell.number)
            ))
            .collect::<Vec<_>>(),
        vec![
            (Some(2), Some(2)),
            (Some(3), Some(3)),
            (Some(4), Some(4)),
            (Some(5), Some(5)),
            (Some(6), Some(6))
        ]
    );
    let changed = &edit.rows[2];
    assert_eq!(changed.left.as_ref().expect("old").text, "    old();");
    assert_eq!(changed.right.as_ref().expect("new").text, "    new();");
    assert!(changed.left.as_ref().expect("old").changed);
    assert!(changed.right.as_ref().expect("new").changed);
    assert!(!edit.rows[0].left.as_ref().expect("context").changed);
}

#[test]
fn edit_preview_numbers_later_edits_after_inserted_lines() {
    let source = "a\nb\nc\nd\n";
    let preview = preview(
        source,
        &serde_json::json!([
            {"search": "b\n", "replace": "b\nx\n"},
            {"search": "d\n", "replace": "D\n"}
        ]),
    );
    assert_eq!(preview.boxes[0].new_range, Some((2, 3)));
    let second = &preview.boxes[1];
    assert_eq!(second.old_range, Some((5, 5)));
    assert_eq!(second.new_range, Some((5, 5)));
    let changed = second
        .rows
        .iter()
        .find(|row| row.left.as_ref().is_some_and(|cell| cell.changed))
        .expect("changed row");
    assert_eq!(changed.left.as_ref().expect("old").text, "d");
    assert_eq!(changed.right.as_ref().expect("new").text, "D");
}

#[test]
fn edit_preview_marks_a_deletion_without_a_result_range() {
    let source = "a\nb\nc\n";
    let preview = preview(
        source,
        &serde_json::json!([{"search": "b\n", "replace": ""}]),
    );
    let edit = &preview.boxes[0];
    assert_eq!(edit.old_range, Some((2, 2)));
    assert_eq!(edit.new_range, None);
    assert_eq!(edit.rows[1].left.as_ref().expect("old").text, "b");
    assert!(edit.rows[1].right.is_none());
    assert_eq!(edit.rows[2].left.as_ref().expect("old").text, "c");
    assert_eq!(edit.rows[2].right.as_ref().expect("new").text, "c");
}

#[test]
fn edit_preview_locates_a_whitespace_only_match() {
    let source = "fn main() {\n    old();\n}\n";
    let preview = preview(
        source,
        &serde_json::json!([{
            "search": "      old();\n}",
            "replace": "      new();\n}"
        }]),
    );
    let edit = &preview.boxes[0];
    assert!(edit.approximate);
    assert_eq!(edit.old_range, Some((2, 3)));
    assert_eq!(edit.rows[1].left.as_ref().expect("old").text, "    old();");
    assert_eq!(
        edit.rows[1].right.as_ref().expect("new").text,
        "      new();"
    );
    assert_eq!(edit.rows[2].right.as_ref().expect("context").text, "}");
}

#[test]
fn edit_preview_without_a_source_has_no_row_numbers() {
    let arguments = serde_json::json!({
        "path": "src/lib.rs",
        "edits": [{"search": "one\ntwo", "replace": "three"}]
    });
    let preview = build_edit_preview(
        "src/lib.rs".to_owned(),
        arguments["edits"].as_array().expect("edits"),
        None,
    );
    let edit = &preview.boxes[0];
    assert!(edit.old_range.is_none());
    assert_eq!(edit.rows[0].left.as_ref().expect("old").number, None);
    assert_eq!(edit.rows[0].left.as_ref().expect("old").text, "one");
    assert_eq!(edit.rows[1].left.as_ref().expect("old").text, "two");
    assert_eq!(edit.rows[0].right.as_ref().expect("new").text, "three");
}

#[test]
fn edit_preview_uses_the_captured_source() {
    let arguments = serde_json::json!({
        "path": "/a/path/that/does/not/exist.rs",
        "edits": [{"search": "two\n", "replace": "TWO\n"}]
    });
    let preview = edit_preview(Some(&arguments), Some("one\ntwo\nthree\n"))
        .expect("preview from captured source");
    let edit = &preview.boxes[0];
    assert_eq!(edit.old_range, Some((2, 2)));
    assert_eq!(edit.rows[1].left.as_ref().expect("old").text, "two");
    assert_eq!(edit.rows[1].right.as_ref().expect("new").text, "TWO");
}

#[test]
fn edit_preview_without_edits_is_not_shown() {
    let arguments = serde_json::json!({"path": "src/lib.rs", "edits": []});
    assert!(edit_preview(Some(&arguments), None).is_none());
    assert!(edit_preview(None, None).is_none());
}
