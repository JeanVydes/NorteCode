//! Tests for the renderer, the edit preview and the argument readers.

use std::fmt::Write as _;
use std::sync::Arc;

use norte_code_protocol::{Delta, ServerEvent, ServerEventKind, SessionKey};

use crate::input::{Pinned, TodoItem, TodoStatus};
use crate::render::argument::todo_items;
use crate::render::dial::context_dial;
use crate::render::edit::{
    EditPreview, build_edit_preview, edit_preview, edit_range, preview_path,
};

use crate::render::renderer::{Block, Format, Renderer, Verbosity, call_ending, tool_sections};
use crate::style::Style;

/// The text of an entry's rows, which is what the assertions are about.
///
/// A row is text and an ink, and most tests ask about the text: spelling every
/// assertion as `.iter().map(...)` would bury the thing under test.
fn texts(entry: &crate::transcript::Entry) -> Vec<String> {
    entry.lines.iter().map(|line| line.text.clone()).collect()
}

fn renderer(pinned: &Arc<Pinned>) -> Renderer {
    Renderer::new(
        Format::Text,
        Verbosity::Low,
        true,
        Style::plain(),
        Arc::clone(pinned),
    )
}

fn visible_renderer(pinned: &Arc<Pinned>) -> Renderer {
    Renderer::new(
        Format::Text,
        Verbosity::Medium,
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

/// A `ToolStarted` kind for a shell call, with the arguments as the tool sends
/// them now: flat, `script` at the top level.
fn shell_started(call_id: &str, arguments: serde_json::Value) -> ServerEventKind {
    ServerEventKind::ToolStarted {
        turn: 1,
        call_id: call_id.to_owned(),
        tool: "shell".to_owned(),
        arguments: Some(arguments),
        source: None,
    }
}

#[test]
fn the_command_that_ran_is_shown_under_the_call() {
    // `command_summary` read `{"command": {"kind": "shell", "script": …}}`, the
    // nested shape the tool used before the schema was flattened. Every call
    // since then missed the lookup, so the transcript showed a command's output
    // with no line saying what produced it.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.tool_event(&shell_started(
        "call-1",
        serde_json::json!({ "script": "cargo test --workspace", "shell": "sh" }),
    ));

    assert_eq!(
        Renderer::command_summary(Some(&serde_json::json!({
            "script": "cargo test --workspace",
            "shell": "sh"
        }))),
        Some("cargo test --workspace".to_owned()),
        "the flat form is the one the tool sends"
    );
    // The nested form still resolves: a transcript replayed from the store was
    // written by a build that sent it.
    assert_eq!(
        Renderer::command_summary(Some(&serde_json::json!({
            "command": { "kind": "shell", "script": "ls -la" }
        }))),
        Some("ls -la".to_owned()),
        "an old transcript still names its command"
    );
    // And a request with no command at all is not invented.
    assert_eq!(
        Renderer::command_summary(Some(&serde_json::json!({ "path": "a.rs" }))),
        None
    );
}

#[test]
fn a_long_command_is_clipped_to_one_line() {
    // The `$` line is one row of the transcript, so a script with newlines or a
    // thousand characters cannot be allowed to make the call's header a block.
    let script = format!("echo {}\n{}\nand more", "x".repeat(400), "y".repeat(400));
    let summary = Renderer::command_summary(Some(&serde_json::json!({ "script": script })))
        .expect("a command");
    assert!(summary.chars().count() <= 161, "one line: {summary:?}");
    assert!(!summary.contains('\n'), "no newline survives: {summary:?}");
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
fn the_report_and_the_footer_name_the_same_effort() {
    // They disagreed when each resolved it separately: the footer used the
    // command line's setting and `/debug` the session's override, which is empty
    // in a session started with `--reasoning`. One value, read twice.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.set_reasoning("high".to_owned());
    let report = crate::render::report_reasoning(Style::plain(), "high");
    assert!(
        report.join("\n").contains("effective: high"),
        "the report names it: {report:?}"
    );
    assert!(
        renderer.session_status().contains("high"),
        "and the footer agrees: {}",
        renderer.session_status()
    );
    assert!(
        renderer.session_status().contains('█'),
        "as a bar, not as a word: {}",
        renderer.session_status()
    );
}

#[test]
fn the_reasoning_effort_is_on_the_footer_and_goes_when_it_is_unset() {
    // Nothing else on screen names the effort, so this row is the only place a
    // `/reasoning` or a downgrade to fit a new model is visible.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    assert!(
        !renderer.session_status().contains('█'),
        "an unset effort draws no bar: {}",
        renderer.session_status()
    );
    renderer.set_reasoning("high".to_owned());
    assert!(
        renderer.session_status().contains("███· high"),
        "the effort is shown, and the bar says which rung it is: {}",
        renderer.session_status()
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

/// A `read` that started, so a test can watch the row and its ending.
fn read_started(call_id: &str, path: &str) -> ServerEvent {
    ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::ToolStarted {
            turn: 1,
            call_id: call_id.to_owned(),
            tool: "read".to_owned(),
            arguments: Some(serde_json::json!({ "path": path })),
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
            detached: false,
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

    // A name the renderer does not know gets no preview: there is one shell, so
    // a finish carrying another shell's name would be a stale second one.
    assert!(
        Renderer::tool_result("shell_bg", None, Some("started")).is_none(),
        "one shell: a second name is not summarised"
    );

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
fn a_read_shows_its_page_only_at_high() {
    // The page a reader read is what the model needs and a person following the
    // work does not: it buries the next `→` under a screenful of a file they did
    // not ask to see. `medium` names the read — the file, its kind and how much
    // it holds — and leaves the page out; `low` is the answer alone and shows
    // neither; `high` is the transcript and shows the whole thing.
    let output =
        "read src/lib.rs (utf8, 42 line(s), 1200 bytes)\n  1- fn main() {}\n  2-     println!();\n";
    let path = Some("src/lib.rs");

    let high = Renderer::tool_result_for("read", Verbosity::High, path, Some(output))
        .expect("high shows the page");
    assert!(high.contains("fn main() {}"), "{high}");

    let medium = Renderer::tool_result_for("read", Verbosity::Medium, path, Some(output))
        .expect("medium names the read");
    assert_eq!(medium, "src/lib.rs (utf8, 42 line(s), 1200 bytes)");
    assert!(
        !medium.contains("fn main() {}"),
        "the page stays out of medium: {medium}"
    );

    assert!(
        Renderer::tool_result_for("read", Verbosity::Low, path, Some(output)).is_none(),
        "low is the answer alone"
    );

    // The tools whose result a person must not have to ask for are untouched:
    // a failed command, a command's output, a write's landed content.
    assert!(
        Renderer::tool_result_for("shell", Verbosity::Medium, None, Some("ok"))
            .is_some_and(|text| text == "ok"),
        "a command still shows its output at medium"
    );
    assert!(
        Renderer::tool_result_for("write", Verbosity::Low, Some("a.rs"), Some("fn x() {}"))
            .is_some_and(|text| text.contains("fn x() {}")),
        "a write still names what landed"
    );
}

#[test]
fn a_detached_call_is_not_drawn_as_a_failure() {
    // The three endings of a call, told apart by their mark. A detach is an
    // error the model is right to see — it did not run to the end — and it is
    // not a failure: the process is still running as a job. Drawing it as
    // `✗ failed` reads as something having gone wrong at the moment something
    // went right, and it is the one ending a person would act on by mistake.
    let style = Style::plain();
    let detached = call_ending(style, "shell", true, true, 1200);
    let failed = call_ending(style, "shell", true, false, 1200);
    let done = call_ending(style, "shell", false, false, 1200);

    assert!(
        detached.contains("let go of"),
        "a detached call says what it was: {detached}"
    );
    assert!(
        detached.contains('◌'),
        "and it does not carry the failure mark: {detached}"
    );
    assert!(
        !detached.contains('✗') && !detached.contains("failed"),
        "nothing about it reads as a failure: {detached}"
    );

    // The other two are unchanged.
    assert!(
        failed.contains('✗') && failed.contains("failed"),
        "{failed}"
    );
    assert!(done.contains('✓') && !done.contains("failed"), "{done}");

    // The three are told apart by their mark alone, which is what a person
    // scans for.
    assert_ne!(detached, failed);
    assert_ne!(detached, done);
    assert_ne!(failed, done);
}

#[test]
fn a_chunk_after_a_call_finished_does_not_open_a_second_box() {
    // A shell's process can outlive its call, and its last chunk can arrive
    // after the finish was drawn: the reader hands over whatever a pipe read
    // returned, and the detach that ended the call does not stop it. Opening a
    // second box for the same call reads as two calls, and the output of the one
    // that is still running belongs to its job rather than to the transcript.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.tool_event(&ServerEventKind::ToolOutput {
        turn: 1,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        stream: "stdout".to_owned(),
        text: "first\n".to_owned(),
        last: false,
    });
    renderer.tool_event(&ServerEventKind::ToolFinished {
        turn: 1,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        is_error: true,
        error: Some("the caller stopped waiting".to_owned()),
        output: Some("what arrived".to_owned()),
        truncated: false,
        detached: true,
        duration_ms: 12,
    });
    assert!(
        renderer.output_gutter().is_none(),
        "the finish closed the box"
    );

    // The chunk that arrives late: it must not reopen it.
    renderer.tool_event(&ServerEventKind::ToolOutput {
        turn: 1,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        stream: "stdout".to_owned(),
        text: "late\n".to_owned(),
        last: false,
    });
    assert!(
        renderer.output_gutter().is_none(),
        "a finished call's box stays closed"
    );

    // A different call still gets its own.
    renderer.tool_event(&ServerEventKind::ToolOutput {
        turn: 1,
        call_id: "call-2".to_owned(),
        tool: "shell".to_owned(),
        stream: "stdout".to_owned(),
        text: "other\n".to_owned(),
        last: false,
    });
    assert_eq!(
        renderer.output_gutter(),
        Some("call-2"),
        "another call opens its own box"
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
fn the_first_reasoning_token_keeps_the_one_bar_at_medium() {
    // The bar says "waiting for the first token"; a reasoning token is that
    // first token, so the bar goes on as "streaming" — one bar, telling the
    // truth as it changes. A second `thinking…` line used to be printed beside
    // it without stopping it, so the bar was pushed onto a line of its own and
    // the old frame stayed on screen.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.mark_working();
    assert!(renderer.waiting.is_some(), "the wait starts with the turn");

    renderer.delta(&Delta::Reasoning {
        index: 0,
        text: "thinking".to_owned(),
    });
    assert!(
        renderer.waiting.is_some(),
        "the bar survives the first token so it can say `streaming`"
    );

    // The first answer token ends the wait: the bar goes and the answer is what
    // is left, which is the last of the three states.
    renderer.delta(&Delta::Text {
        index: 1,
        text: "answer".to_owned(),
    });
    assert!(
        renderer.waiting.is_none(),
        "the bar disappears once the answer arrives"
    );
}

#[test]
fn thinking_is_announced_when_nothing_else_says_so() {
    // The reasoning arriving at `medium` says the model is thinking without
    // drawing the thought, and the spinner is what says it. What matters is
    // that the activity is set: it is what the frame draws, and a turn that
    // showed nothing while the model worked would look like a hang.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.delta(&Delta::Reasoning {
        index: 0,
        text: "thinking".to_owned(),
    });
    // The spinner takes over: a thought arriving means the provider answered,
    // and the frame says `thinking` — not `streaming`, which would claim the
    // answer had started when only the thought has. The `thinking…` line is what
    // a turn with no spinner running shows instead.
    assert_eq!(renderer.waiting, Some(crate::render::Wait::Thinking));
    // The thought reaches the conversation through the event, which is the one
    // place an event becomes an entry: `delta` moves the state, and the event
    // that carried it makes the entry. Two writers would put it in twice.
    let reasoning = renderer
        .transcript
        .entries()
        .iter()
        .filter(|entry| entry.kind == crate::transcript::Kind::Reasoning)
        .count();
    assert_eq!(
        reasoning, 0,
        "the direct call is state only; the event is what records"
    );
}

#[test]
fn a_summary_with_no_answer_still_stops_the_bar() {
    // A turn that only called tools has no answer text, so `block` is `None`
    // when the request settles. The summary was printed onto the bar's own line
    // and left there, because the bar was only stopped for a non-empty block.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.mark_working();
    renderer.usage_event(&ServerEventKind::RequestReconstructed {
        turn: 1,
        index: 0,
        usage: norte_code_protocol::Usage::default(),
        cost_usd: Some(0.0),
        truncated: false,
        duration_ms: 3_000,
        first_token_ms: Some(2_300),
        tokens_per_second: None,
        cache_hit: false,
        tools: vec!["shell".to_owned()],
    });
    assert!(
        renderer.waiting.is_none(),
        "the summary must not be written onto the bar's line"
    );
}

#[test]
fn the_bar_comes_back_after_a_tool() {
    // The bar stops on the first answer token, which is right; a turn that
    // called a tool then goes on, and it used to do so with no signal at all.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.mark_working();
    assert!(renderer.waiting.is_some(), "the bar starts with the turn");

    // The model answers with a call, which stops the bar like any answer.
    renderer.delta(&Delta::Text {
        index: 0,
        text: "calling a tool".to_owned(),
    });
    renderer.end_blocks();
    assert!(
        renderer.waiting.is_none(),
        "the answer stops the bar, as before"
    );

    // The tool finishes; the turn is not over, so the bar returns.
    let finished = tool_finished("call-1", "write", false);
    renderer.tool_event(&finished.kind);
    assert!(
        renderer.waiting.is_some(),
        "the bar is back for what the model says next"
    );
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
    assert!(
        pinned.todos().is_empty(),
        "the footer waits for the call to finish"
    );
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
    assert!(pinned.todos().is_empty(), "an empty list clears the footer");
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
    assert!(
        pinned.todos().is_empty(),
        "a call that is not `todo` leaves the footer alone"
    );
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

#[test]
fn streamed_output_is_boxed_and_closed_by_a_chunk_with_no_bytes() {
    // The closing chunk a tool sends carries no bytes, so it has to be acted on
    // before emptiness is checked: returning early on it left the box open, and
    // the `╰` landed after whatever printed next. The box also stays open across
    // the partial lines a pipe read hands over.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    let output = |renderer: &mut Renderer, text: &str, last: bool| {
        renderer.tool_event(&ServerEventKind::ToolOutput {
            turn: 1,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: "stdout".to_owned(),
            text: text.to_owned(),
            last,
        });
    };

    // Two chunks that split a line, then the empty closing one.
    output(&mut renderer, "running 5", false);
    output(&mut renderer, " tests\nok\n", false);
    assert!(
        renderer.output_gutter().is_some(),
        "the box is open while the tool writes"
    );
    output(&mut renderer, "", true);
    assert!(
        renderer.output_gutter().is_none(),
        "an empty closing chunk still closes the box"
    );
}

#[test]
fn a_finished_call_closes_the_box_and_its_result_is_not_repeated() {
    // The streamed text was already shown, so the finish must not print it
    // again; and it has to close the box first, or the bracket is orphaned.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.tool_event(&ServerEventKind::ToolOutput {
        turn: 1,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        stream: "stdout".to_owned(),
        text: "output\n".to_owned(),
        last: false,
    });
    assert!(renderer.output_gutter().is_some());
    renderer.tool_event(&ServerEventKind::ToolFinished {
        turn: 1,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        is_error: false,
        error: None,
        output: Some("output\n".to_owned()),
        truncated: false,
        detached: false,
        duration_ms: 5,
    });
    assert!(
        renderer.output_gutter().is_none(),
        "the finish closes the box"
    );
    assert!(
        renderer.streamed_calls().is_empty(),
        "and its result was already shown, so it is not kept to repeat"
    );
}

#[test]
fn a_line_split_across_chunks_waits_until_it_is_whole() {
    // A pipe read ends wherever it ends, so one row arrives as two chunks:
    // `stdout[01] p` then `arcial... completa`. Writing each chunk as it came
    // put half a word on a row of its own, which is what the screen showed.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    let chunk = |renderer: &mut Renderer, stream: &str, text: &str| {
        renderer.tool_event(&ServerEventKind::ToolOutput {
            turn: 1,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: stream.to_owned(),
            text: text.to_owned(),
            last: false,
        });
    };

    // Half a line waits in its own stream's buffer.
    chunk(&mut renderer, "stdout", "stdout[01] p");
    assert_eq!(
        renderer.output_pending(),
        [("stdout".to_owned(), "stdout[01] p".to_owned())],
        "a line without a newline waits for the rest of itself"
    );

    // The rest joins it, and the buffer is emptied rather than left at "".
    chunk(&mut renderer, "stdout", "arcial... completa\n");
    assert!(
        renderer.output_pending().is_empty(),
        "the line was whole, so nothing waits now"
    );

    // Each stream keeps its own half-line: they are read by two tasks.
    chunk(&mut renderer, "stdout", "uno ");
    chunk(&mut renderer, "stderr", "dos ");
    let mut waiting = renderer.output_pending();
    waiting.sort();
    assert_eq!(
        waiting,
        [
            ("stderr".to_owned(), "dos ".to_owned()),
            ("stdout".to_owned(), "uno ".to_owned()),
        ],
        "each stream waits on its own line"
    );
}

#[test]
fn a_line_that_never_ends_is_shown_once_it_is_long_enough() {
    // A progress bar writes without newlines. Holding it for a newline that
    // never comes would show nothing at all while the tool runs.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    let chunk = |renderer: &mut Renderer, text: &str| {
        renderer.tool_event(&ServerEventKind::ToolOutput {
            turn: 1,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: "stdout".to_owned(),
            text: text.to_owned(),
            last: false,
        });
    };
    chunk(&mut renderer, &"#".repeat(500));
    assert!(
        !renderer.output_pending().is_empty(),
        "half a kilobyte is not enough to give up waiting"
    );
    chunk(&mut renderer, &"#".repeat(100));
    assert!(
        renderer.output_pending().is_empty(),
        "past the cap the line is shown rather than held"
    );
}

#[test]
fn the_box_stops_printing_rows_past_its_height() {
    // Without a bound, a command that prints thousands of lines pushes the
    // transcript off the screen: the box shows its first rows and counts the
    // rest rather than growing with the output.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    for index in 0..40 {
        renderer.tool_event(&ServerEventKind::ToolOutput {
            turn: 1,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: "stdout".to_owned(),
            text: format!("line {index}\n"),
            last: false,
        });
    }
    assert_eq!(
        renderer.output_rows_seen(),
        40,
        "every line that arrived is counted, whether shown or not"
    );
    renderer.tool_event(&ServerEventKind::ToolOutput {
        turn: 1,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        stream: "stdout".to_owned(),
        text: String::new(),
        last: true,
    });
    assert_eq!(renderer.output_rows_seen(), 0, "closing resets the count");
}

#[test]
fn the_output_box_is_bounded_by_the_ceiling_without_a_screen() {
    // A renderer with no terminal has no screen to measure, so the ceiling is
    // the bound: the box holds that many rows and counts the rest.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    let limit = renderer.output_limit_for_test();
    assert_eq!(limit, 400, "no screen means the ceiling");
    for index in 0..450 {
        renderer.tool_event(&ServerEventKind::ToolOutput {
            turn: 1,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: "stdout".to_owned(),
            text: format!("line {index}\n"),
            last: false,
        });
    }
    assert_eq!(
        renderer.output_rows_len(),
        limit,
        "the box holds the bound and no more"
    );
    assert_eq!(
        renderer.output_rows_seen(),
        450,
        "and counts everything that arrived"
    );
}

#[test]
fn every_reasoning_level_draws_its_own_bar() {
    // A control that cannot be told apart from the one below it is not showing
    // anything, so the levels are checked to be distinct rather than merely
    // plausible -- and in the order the settings are ordered by, which is the
    // one thing a bar is for.
    let style = Style::plain();
    let plain: Vec<String> = ["off", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .map(|level| crate::render::dial::reasoning_bar(style, level))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for (level, bar) in ["off", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .zip(&plain)
    {
        assert!(
            seen.insert(bar.clone()),
            "`{level}` draws a bar already used: {bar}"
        );
    }
    let filled = |bar: &str| bar.chars().filter(|c| *c == '\u{2588}').count();
    assert_eq!(filled(&plain[0]), 0, "off is the empty bar: {} ", plain[0]);
    assert!(
        filled(&plain[1]) > 0,
        "low is almost nothing, not nothing: {}",
        plain[1]
    );
    assert!(
        plain
            .windows(2)
            .all(|pair| filled(&pair[0]) <= filled(&pair[1])),
        "the bar never shrinks as the effort grows: {plain:?}"
    );
    assert_eq!(filled(&plain[5]), 4, "max fills it: {}", plain[5]);
    // The label stays beside the bar: the bar says where, the word says what.
    for (level, bar) in ["off", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .zip(&plain)
    {
        assert!(bar.contains(level), "`{level}` is named: {bar}");
    }
}

#[test]
fn a_setting_that_is_not_a_rung_draws_no_bar() {
    // `auto` and a token budget are settings of another kind. Drawing a bar for
    // them would claim an effort on a scale they are not on.
    let style = Style::plain();
    let auto = crate::render::dial::reasoning_bar(style, "auto");
    assert!(!auto.contains('\u{2588}'), "auto claims no rung: {auto}");
    assert!(auto.contains("auto"), "{auto}");
    let budget = crate::render::dial::reasoning_bar(style, "budget 4096 tokens");
    assert!(
        !budget.contains('\u{2588}'),
        "a budget claims no rung: {budget}"
    );
    assert!(budget.contains("4096"), "{budget}");
}

#[test]
fn the_footer_names_the_directory_the_work_is_in() {
    // The root is what a person checks when a tool reads the wrong tree, and
    // the footer is where they are looking. It cannot be set in a test -- the
    // process's own directory is the test runner's and `set_var` is unsafe --
    // so what is checked is that the footer carries the path it was given and
    // that the switch turns it off.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.session = Some("f5868da6-0000-4000-8000-000000000000".to_owned());
    let shown = renderer.session_status();
    let cwd = std::env::current_dir().expect("a directory");
    let expected = crate::render::helpers::short_path_in(
        &cwd,
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .as_deref(),
    );
    assert!(
        shown.contains(&expected),
        "the footer names `{expected}`: {shown}"
    );
    // The line is still one line with the switch off, which is what a narrow
    // terminal or a pipe needs.
    renderer.show_workdir = false;
    let without = renderer.session_status();
    assert!(
        !without.contains(&expected),
        "and it goes when it is asked to: {without}"
    );
    assert!(
        without.contains("session"),
        "the rest of the line stays: {without}"
    );
}

#[test]
fn the_unified_diff_reads_as_a_patch() {
    // The rows are what the terminal shows, so this is the only place the shape
    // can be checked: a `+` above its `-`, or the wrong line number, still
    // looks like a diff and is wrong.
    let arguments = serde_json::json!({
        "path": "src/lib.rs",
        "edits": [{"search": "two\n", "replace": "TWO\n"}]
    });
    let preview = edit_preview(Some(&arguments), Some("one\ntwo\nthree\n")).expect("preview");
    let edit = &preview.boxes[0];
    let lines = Renderer::unified_lines(Style::plain(), &preview.path, edit, 60);
    // The rows are text and an ink, so this is the only place the shape can be
    // checked: the text alone, and which of it left and which arrived.
    let plain: Vec<String> = lines.iter().map(|(_, text)| text.clone()).collect();
    let joined = plain.join("\n");

    // What left comes before what arrived, which is how a patch reads.
    let removed = plain
        .iter()
        .position(|line| line.contains("- two"))
        .expect("the removed line");
    let added = plain
        .iter()
        .position(|line| line.contains("+ TWO"))
        .expect("the added line");
    assert!(removed < added, "the old line leads:\n{joined}");
    // The context is one row per line, not one per side.
    assert_eq!(
        plain.iter().filter(|line| line.contains("one")).count(),
        1,
        "context is not drawn twice:\n{joined}"
    );
    // The frame is the view's, so the rows carry none of it: a `│` in a row is
    // a `│` a person selects along with the code they meant to copy.
    assert!(!joined.contains('╭'), "no rule in a row:\n{joined}");
    assert!(!joined.contains('│'), "no bar in a row:\n{joined}");
    // And what left reads red, what arrived green, the context quiet.
    let ink_of = |needle: &str| {
        lines
            .iter()
            .find(|(_, text)| text.contains(needle))
            .map(|(ink, _)| *ink)
            .expect(needle)
    };
    assert_eq!(ink_of("- two"), crate::transcript::Ink::Red);
    assert_eq!(ink_of("+ TWO"), crate::transcript::Ink::Green);
    assert_eq!(ink_of("one"), crate::transcript::Ink::Faint);
}

#[test]
fn the_unified_diff_clips_to_the_width_it_was_given() {
    // A row wider than the terminal wraps and breaks the box, which is the one
    // thing a box is for. `width` is the box's own width: `edit_box_width`
    // takes the two columns of indentation off before it hands one over, so a
    // row of `width` plus those two is what fits the screen.
    let arguments = serde_json::json!({
        "path": "src/lib.rs",
        "edits": [{
            "search": "short\n",
            "replace": format!("{}\n", "x".repeat(200))
        }]
    });
    let preview = edit_preview(Some(&arguments), Some("short\n")).expect("preview");
    let edit = &preview.boxes[0];
    let width = 60;
    for (_, line) in Renderer::unified_lines(Style::plain(), &preview.path, edit, width) {
        let visible = line.chars().count();
        assert!(
            visible <= width + 2,
            "a row of {visible} columns does not fit {width} plus its indent: {line:?}"
        );
    }
}

#[test]
fn the_split_view_still_draws_two_panels() {
    // The style is a choice, and the one that was there before is still there:
    // a config that names `split` gets what it always got.
    let arguments = serde_json::json!({
        "path": "src/lib.rs",
        "edits": [{"search": "two\n", "replace": "TWO\n"}]
    });
    let preview = edit_preview(Some(&arguments), Some("one\ntwo\nthree\n")).expect("preview");
    let edit = &preview.boxes[0];
    let unified = Renderer::unified_lines(Style::plain(), &preview.path, edit, 60);
    let joined: Vec<&str> = unified.iter().map(|(_, text)| text.as_str()).collect();
    let joined = joined.join("\n");
    assert!(
        !joined.contains("original"),
        "the unified view has no panels: {joined}"
    );
    assert!(
        !joined.contains("edit 1/1"),
        "the heading is the box's, not a row: {joined}"
    );
    assert_eq!(
        Renderer::edit_title(&preview.path, edit),
        format!(
            "edit 1/1 · {} · {}",
            edit_range(edit),
            preview_path(&preview.path)
        ),
        "both views name the same edit"
    );
}

#[test]
fn the_conversation_is_kept_as_it_arrives() {
    // The transcript is the same conversation the printing produces, recorded
    // rather than drawn. It exists so the interface can re-wrap and scroll,
    // which the printing cannot: what went to a terminal cannot be asked back.
    use crate::transcript::Kind;

    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    let delta = |kind: &str, text: &str| ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::Delta {
            turn: 1,
            delta: match kind {
                "reasoning" => Delta::Reasoning {
                    index: 0,
                    text: text.to_owned(),
                },
                _ => Delta::Text {
                    index: 0,
                    text: text.to_owned(),
                },
            },
        },
    };

    renderer
        .event(&delta("reasoning", "thinking"))
        .expect("event");
    renderer.event(&delta("text", "Hel")).expect("event");
    renderer.event(&delta("text", "lo")).expect("event");
    renderer
        .event(&todo_started("c1", &serde_json::json!([])))
        .expect("event");

    let entries = renderer.transcript.entries();
    let kinds: Vec<Kind> = entries.iter().map(|entry| entry.kind).collect();
    // At `medium` the thought is not kept: the spinner says the model is
    // working, and the thought itself is the longest thing it writes and the
    // least of it is worth reading. `high` asks for it, and the test below.
    assert_eq!(
        kinds,
        [Kind::Answer, Kind::ToolStarted],
        "each event is one entry, in the order they arrived: {entries:?}"
    );
    assert_eq!(
        texts(&entries[0]),
        ["Hello"],
        "the two tokens are one answer, not two lines"
    );
    // And a tool starting closes the answer: what comes after is a new thing.
    assert_eq!(entries[1].lines.len(), 1, "the header, and no command here");

    // At `high` the thought is kept, and it is its own entry: the reasoning and
    // the answer are not the same thing, and a transcript that merged them
    // would read as one voice.
    renderer.verbosity = crate::render::Verbosity::High;
    renderer.transcript.clear();
    renderer
        .event(&delta("reasoning", "thinking"))
        .expect("event");
    renderer.event(&delta("text", "Hello")).expect("event");
    // The thought leads the answer, and the gutter closes when the answer
    // starts: a box that stayed open would put the answer inside it.
    let entries = renderer.transcript.entries();
    let thought = entries
        .iter()
        .find(|entry| entry.kind == Kind::Reasoning)
        .expect("the thought is kept at high");
    assert_eq!(texts(thought), ["thinking"]);
    let answer = entries
        .iter()
        .position(|entry| entry.kind == Kind::Answer)
        .expect("the answer follows");
    let thought = entries
        .iter()
        .position(|entry| entry.kind == Kind::Reasoning)
        .expect("and it is before it");
    assert!(thought < answer, "the thought leads: {entries:?}");
}

#[test]
fn a_call_is_one_row_that_changes_when_it_ends() {
    // A tool used to be two rows — one announcing it and one reporting how it
    // went — so a suite of ten calls put twenty lines in the conversation and
    // the reader had to pair them up by the id beside them. One call is one
    // row: it says what is running while it runs, and the same row gains its
    // outcome when it ends.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);

    renderer
        .event(&read_started("call-1", "README.md"))
        .expect("started");
    let running = renderer.transcript.entries();
    assert_eq!(running.len(), 1, "the announcement is the whole of it");
    assert_eq!(texts(&running[0]), ["read README.md"]);
    assert!(
        running[0].started.is_some(),
        "and it is counting, because it has not said it is done"
    );

    renderer
        .event(&tool_finished("call-1", "read", false))
        .expect("finished");
    let entries = renderer.transcript.entries();
    assert_eq!(
        entries.len(),
        1,
        "the ending edits the row, it does not add one: {:?}",
        entries.iter().map(texts).collect::<Vec<_>>()
    );
    let done = texts(&entries[0]);
    assert!(done[0].contains("read"), "{done:?}");
    assert!(done[0].contains("ms"), "the ending says how long: {done:?}");
    assert!(
        entries[0].started.is_none(),
        "and the clock stopped with it"
    );
}

#[test]
fn a_call_that_failed_keeps_one_row_and_reads_as_a_failure() {
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer
        .event(&read_started("call-1", "missing.md"))
        .expect("started");
    renderer
        .event(&tool_finished("call-1", "read", true))
        .expect("finished");
    let entries = renderer.transcript.entries();
    assert_eq!(entries.len(), 1, "still one row");
    assert_eq!(
        entries[0].kind,
        crate::transcript::Kind::Failure,
        "and the row itself says it went wrong"
    );
}

#[test]
fn a_quiet_run_shows_neither_the_call_nor_its_ending() {
    // `low` is the answer alone: no announcement, no ending, no row. The ending
    // edits the row the call opened, and a run that never opened one has
    // nothing to edit — which is the level working, not a call going missing.
    let pinned = Pinned::new();
    let mut renderer = renderer(&pinned);
    assert!(renderer.verbosity.is_quiet());
    renderer
        .event(&read_started("call-1", "README.md"))
        .expect("started");
    renderer
        .event(&tool_finished("call-1", "read", false))
        .expect("finished");
    assert!(
        renderer.transcript.entries().is_empty(),
        "the answer alone: {:?}",
        renderer
            .transcript
            .entries()
            .iter()
            .map(texts)
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_outcome_of_a_call_carries_its_own_ink() {
    // While a call runs its row is grey; when it ends the same row takes the
    // colour of what it became. That is what a person reads down the
    // conversation for: what worked and what did not, at a glance.
    let ink_of = |is_error: bool| {
        let pinned = Pinned::new();
        let mut renderer = visible_renderer(&pinned);
        renderer
            .event(&read_started("call-1", "README.md"))
            .expect("started");
        renderer
            .event(&tool_finished("call-1", "read", is_error))
            .expect("finished");
        renderer.transcript.entries()[0].lines[0].ink
    };
    assert_eq!(ink_of(false), Some(crate::transcript::Ink::Green));
    assert_eq!(ink_of(true), Some(crate::transcript::Ink::Red));
}

#[test]
fn a_call_that_is_still_out_is_grey_and_carries_no_outcome() {
    // The row while it runs has no ink of its own: the frame draws it in the
    // quiet ink of a thing in progress, and there is no tick or cross in the
    // text because the call has not said how it went.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer
        .event(&read_started("call-1", "README.md"))
        .expect("started");
    let entry = &renderer.transcript.entries()[0];
    assert_eq!(entry.lines[0].ink, None, "no outcome yet");
    let text = &entry.lines[0].text;
    assert!(!text.contains('\u{2713}'), "no tick: {text:?}");
    assert!(!text.contains('\u{2717}'), "no cross: {text:?}");
    assert!(entry.started.is_some(), "and it is counting");
}

/// A delta event, as the loop hands it to the renderer.
fn delta_event(delta: Delta) -> ServerEvent {
    ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::Delta { turn: 1, delta },
    }
}

#[test]
fn an_answer_that_is_interleaved_with_thinking_is_not_split() {
    // A model that reasons emits `Reasoning` and `Text` in turn, and every
    // change of block writes the blank row that separates two blocks. The row
    // was written after the entry for the new text had been made, so it landed
    // inside the answer instead of between the two: the first word of an answer
    // ended up alone on its row with a gap under it, and the rest started again
    // below — the answer looked like it had been cut in half.
    //
    // The block change is what makes the row, so it has to happen before the
    // entry exists. What is asserted here is the shape of the whole
    // conversation: one entry per block, in the order they arrived.
    let pinned = Pinned::new();
    let mut renderer = reasoning_renderer(&pinned);
    renderer.mark_working();
    for (kind, text) in [
        ("r", "Pienso un momento. "),
        ("t", "Ant"),
        ("t", "es de borrar"),
        ("t", " tengo que resolver"),
        ("r", " una duda"),
        ("t", " una duda critica"),
        ("t", ": hay 105 requests"),
        ("r", " mas"),
        ("t", " con nuestro north_session"),
        ("t", " pero otra key."),
    ] {
        let delta = if kind == "r" {
            Delta::Reasoning {
                index: 0,
                text: text.to_owned(),
            }
        } else {
            Delta::Text {
                index: 0,
                text: text.to_owned(),
            }
        };
        renderer.event(&delta_event(delta)).expect("event");
    }
    // The answers, in order and with the blank rows taken out: the text reads
    // as one answer and the first word is not on a row of its own.
    let answers: Vec<Vec<String>> = renderer
        .transcript
        .entries()
        .iter()
        .filter(|entry| entry.kind == crate::transcript::Kind::Answer)
        .map(texts)
        .collect();
    assert!(
        !answers
            .iter()
            .any(|rows| rows.first().is_some_and(|row| row == "Ant")),
        "the first word is not alone on its row: {answers:?}"
    );
    let whole: String = answers.into_iter().flatten().collect();
    assert!(
        whole.contains("Antes de borrar tengo que resolver"),
        "the answer reads as one: {whole:?}"
    );
    // And a blank row is written where a block gave way to another, which is
    // what separates a thought from the answer it leads to.
    let blanks = renderer
        .transcript
        .entries()
        .iter()
        .filter(|entry| entry.kind == crate::transcript::Kind::Notice && texts(entry) == [""])
        .count();
    assert!(blanks > 0, "the blocks are still separated");
}
