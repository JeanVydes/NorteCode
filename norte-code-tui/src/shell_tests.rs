//! Shell call arguments.

use super::ShellArgs;

fn args(json: serde_json::Value) -> ShellArgs {
    serde_json::from_value(json).expect("shell args")
}

#[test]
fn a_script_is_the_command() {
    let arguments = args(serde_json::json!({ "script": "echo hi", "command": "other" }));
    assert_eq!(arguments.command(), Some("echo hi"));
}

#[test]
fn command_and_cmd_are_fallbacks() {
    let arguments = args(serde_json::json!({ "cmd": "  ls -la  " }));
    assert_eq!(arguments.command(), Some("ls -la"));
    let arguments = args(serde_json::json!({ "command": "pwd" }));
    assert_eq!(arguments.command(), Some("pwd"));
}

#[test]
fn a_call_without_a_command_has_nothing_to_show() {
    let arguments = args(serde_json::json!({ "timeout": 1000 }));
    assert_eq!(arguments.command(), None);
}
