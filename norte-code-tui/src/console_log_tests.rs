//! Shell output stays bounded without losing the command or its tail.

use terminalui::components::console::{Console, ConsoleStatus};

use super::push_line;

#[test]
fn a_flooding_command_keeps_only_its_recent_output() {
    let mut console = Console::new("cargo test");
    for index in 0..600 {
        push_line(&mut console, format!("line {index}"));
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
        push_line(&mut console, format!("line {index}"));
    }

    assert_eq!(console.status, ConsoleStatus::Exited { code: Some(0) });
    assert_eq!(console.state.frame, 3);
}
