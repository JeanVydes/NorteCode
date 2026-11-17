use norte_code_wire::{OutputStream, ToolStatus};
use ratatui::style::Color;
use serde_json::json;

use super::{ToolCallComponent, ToolFinish};
use crate::{components::palette, tools::DiffMode, ui_model::ThemeMode};

fn line_content(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn a_todo_call_shows_only_the_tool_name() {
    let arguments = json!({"todos": [{"content": "secret task", "status": "pending"}]});

    let lines = ToolCallComponent::started(
        "todo",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );

    assert_eq!(line_content(&lines[0]), "✓ todo");
    assert_eq!(lines.len(), 1, "no divider line after a tool header");
}

#[test]
fn a_write_call_names_the_target_and_the_line_count() {
    let arguments = json!({"path": "src/main.rs", "content": "one\ntwo\n"});

    let lines = ToolCallComponent::started(
        "write",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );

    assert_eq!(line_content(&lines[0]), "✎ write  src/main.rs · 2 lines");
}

#[test]
fn a_shell_log_draws_lines_with_the_footer_background() {
    let lines = ToolCallComponent::shell_output_tail(
        &[
            (OutputStream::Stdout, "one".to_owned()),
            (OutputStream::Stdout, "two".to_owned()),
        ],
        false,
        ThemeMode::Dark,
        80,
    );

    assert!(lines.iter().all(|line| {
        line.spans
            .iter()
            .all(|span| span.style.bg == Some(footer_background()))
    }));
}

#[test]
fn a_raw_args_banner_fills_the_line_without_frames() {
    let arguments = json!({"detach": true, "script": "sleep 10"});

    let lines = ToolCallComponent::started(
        "shell",
        Some(&arguments),
        None,
        DiffMode::Unified,
        40,
        true,
        ThemeMode::Dark,
    );
    let banner = lines
        .iter()
        .map(line_content)
        .find(|line| line.contains("raw args"))
        .expect("developer mode draws the raw args banner");

    assert!(!banner.contains(['┌', '│', '└', '─']));
    assert_eq!(
        line_content(
            lines
                .iter()
                .find(|line| line_content(line).contains("raw args"))
                .expect("raw args banner"),
        )
        .chars()
        .count(),
        40,
        "the banner fills the full width the way the footer does"
    );
}

#[test]
fn a_detached_shell_call_hides_output_and_says_so() {
    let lines = ToolCallComponent::finished(ToolFinish {
        name: "shell",
        status: ToolStatus::Cancelled,
        output: None,
        error: None,
        shell: true,
        theme: ThemeMode::Dark,
        width: 80,
        show_tool_output: false,
    });

    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(drawn.contains("output hidden: detached job"));
}

fn footer_background() -> Color {
    palette::surface(ThemeMode::Dark)
}

/// The blob a shell returns: one structured result with the streams inside.
const SHELL_BLOB: &str = "code: Success\nexit_code: Some(0)\nduration_ms: 8\nbytes_read: 38\nstdout:\nline one\nline two (edited)\nline three";

fn finished_shell(show_tool_output: bool) -> Vec<ratatui::text::Line<'static>> {
    ToolCallComponent::finished(ToolFinish {
        name: "shell",
        status: ToolStatus::Ok,
        output: Some(SHELL_BLOB),
        error: None,
        shell: true,
        theme: ThemeMode::Dark,
        width: 80,
        show_tool_output,
    })
}

#[test]
fn a_shell_always_shows_its_stdout() {
    let drawn = finished_shell(false)
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.contains("line one"), "the stdout is drawn:\n{drawn}");
    assert!(drawn.contains("line two (edited)"));
    assert!(
        !drawn.contains("code: Success"),
        "the blob is not shown:\n{drawn}"
    );
    assert!(!drawn.contains("exit_code"));
    assert!(!drawn.contains("bytes_read"));
}

#[test]
fn the_tool_output_adds_the_whole_blob_and_keeps_the_stdout() {
    let drawn = finished_shell(true)
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        drawn.contains("code: Success"),
        "the blob is whole:\n{drawn}"
    );
    assert!(drawn.contains("exit_code: Some(0)"));
    assert!(drawn.contains("duration_ms: 8"));
    assert!(
        drawn.contains("line one"),
        "the stdout is still apart:\n{drawn}"
    );
}

#[test]
fn an_edit_shows_its_result_text_as_the_tool_wrote_it() {
    let lines = ToolCallComponent::finished(ToolFinish {
        name: "edit",
        status: ToolStatus::Ok,
        output: Some(
            "edited /tmp/a.txt: 1 edit(s) applied (3 lines, 38 bytes)\n  edit 0: exact at lines 2-2",
        ),
        error: None,
        shell: false,
        theme: ThemeMode::Dark,
        width: 80,
        show_tool_output: true,
    });
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.contains("edited /tmp/a.txt: 1 edit(s) applied"));
    assert!(drawn.contains("  edit 0: exact at lines 2-2"));
    assert!(
        !drawn.contains("out "),
        "the result is not labelled:\n{drawn}"
    );
}

#[test]
fn a_spawned_agent_shows_its_agent_and_title() {
    let arguments = serde_json::json!({
        "prompt": "do the thing",
        "agent_id": "reviewer",
        "title": "Review the diff",
        "detach": false,
        "lifetime": "turn"
    });
    let lines = ToolCallComponent::started(
        "spawn_agent",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.contains("reviewer"), "drawn:\n{drawn}");
    assert!(drawn.contains("Review the diff"), "drawn:\n{drawn}");
}

#[test]
fn a_spawn_without_a_title_shows_the_start_of_the_prompt() {
    let prompt = "a".repeat(400);
    let arguments = serde_json::json!({
        "prompt": prompt,
        "detach": false,
        "lifetime": "turn"
    });
    let lines = ToolCallComponent::started(
        "spawn_agent",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.contains("..."), "a long prompt is cut:\n{drawn}");
}

#[test]
fn a_message_to_an_agent_shows_what_was_sent() {
    let arguments = serde_json::json!({
        "child_session_id": "abcdef1234567890",
        "content": "please also check the tests",
        "delivery": "steer"
    });
    let lines = ToolCallComponent::started(
        "message_agent",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.contains("abcdef12"), "drawn:\n{drawn}");
    assert!(
        drawn.contains("please also check the tests"),
        "drawn:\n{drawn}"
    );
}

#[test]
fn a_message_sent_as_content_parts_still_shows_its_text() {
    let arguments = serde_json::json!({
        "child_session_id": "abcdef1234567890",
        "content": [{"type": "text", "text": "part one"}, {"type": "text", "text": "part two"}]
    });
    let lines = ToolCallComponent::started(
        "message_agent",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.contains("part one"), "drawn:\n{drawn}");
    assert!(drawn.contains("part two"), "drawn:\n{drawn}");
}

#[test]
fn a_long_message_is_cut_to_one_readable_line() {
    let arguments = serde_json::json!({
        "child_session_id": "abcdef1234567890",
        "content": "word ".repeat(200)
    });
    let lines = ToolCallComponent::started(
        "message_agent",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.ends_with("..."), "the message is cut:\n{drawn}");
    assert!(drawn.len() < 200);
}

#[test]
fn a_wait_names_what_it_waits_for() {
    let arguments = serde_json::json!({
        "child_session_id": "abcdef1234567890",
        "until": "finished",
        "timeout_s": 120
    });
    let lines = ToolCallComponent::started(
        "wait_agent",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.contains("until finished"), "drawn:\n{drawn}");
    assert!(drawn.contains("within 120s"), "drawn:\n{drawn}");
}

#[test]
fn a_wait_defaults_are_shown() {
    let arguments = serde_json::json!({"child_session_id": "abcdef1234567890"});
    let lines = ToolCallComponent::started(
        "wait_agent",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.contains("until idle"), "drawn:\n{drawn}");
    assert!(drawn.contains("within 300s"), "drawn:\n{drawn}");
}

#[test]
fn a_stop_names_the_child_and_the_mode() {
    let arguments = serde_json::json!({
        "child_session_id": "abcdef1234567890",
        "mode": "kill"
    });
    let lines = ToolCallComponent::started(
        "stop_agent",
        Some(&arguments),
        None,
        DiffMode::Unified,
        80,
        false,
        ThemeMode::Dark,
    );
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(drawn.contains("kill"), "drawn:\n{drawn}");
}
