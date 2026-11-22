//! Shell output stays bounded without losing the command or its tail.

use norte_code_client::config::RetentionConfig;
use terminalui::components::console::{Console, ConsoleStatus};

use super::push_line;

#[test]
fn a_flooding_command_keeps_only_its_recent_output() {
    let mut console = Console::new("cargo test");
    for index in 0..600 {
        push_line(
            &mut console,
            format!("line {index}"),
            RetentionConfig::default(),
        );
    }

    assert_eq!(console.lines().len(), 400);
    assert_eq!(console.lines()[0], "line 200");
    assert_eq!(console.lines()[399], "line 599");
    assert_eq!(console.command, "cargo test");
}

#[test]
fn trimming_keeps_the_command_status_and_spinner_state() {
    let mut console = Console::new("build");
    console.status = ConsoleStatus::Exited { code: Some(0) };
    console.state.frame = 3;
    for index in 0..501 {
        push_line(
            &mut console,
            format!("line {index}"),
            RetentionConfig::default(),
        );
    }

    assert_eq!(console.status, ConsoleStatus::Exited { code: Some(0) });
    assert_eq!(console.state.frame, 3);
}

#[test]
fn configured_retention_decides_when_output_is_trimmed_and_how_much_survives() {
    let retention = RetentionConfig {
        console_max_lines: 20,
        console_keep_lines: 5,
        ..RetentionConfig::default()
    };
    let mut console = Console::new("short command");
    for index in 0..20 {
        push_line(&mut console, format!("line {index}"), retention);
    }

    assert_eq!(console.lines().len(), 5);
    assert_eq!(console.lines()[0], "line 15");
    assert_eq!(console.lines()[4], "line 19");
}
