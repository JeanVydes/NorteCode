use super::*;

#[test]
fn a_pretty_printed_call_summarises_to_its_target() {
    let call = parse_replayed_call("read {\n  \"path\": \"src/main.rs\",\n  \"limit\": 40\n}")
        .expect("parsed");
    assert_eq!(call.name, "read");
    assert_eq!(call.target.as_deref(), Some("src/main.rs"));
    assert_eq!(call.arguments["limit"], 40);
}

#[test]
fn a_call_without_arguments_is_just_a_name() {
    let call = parse_replayed_call("todo").expect("parsed");
    assert_eq!(call.name, "todo");
    assert_eq!(call.target, None);
    assert!(call.arguments.is_null());
}

#[test]
fn a_brace_in_the_name_does_not_swallow_the_line() {
    assert!(parse_replayed_call("{ \"not\": \"a name\" }").is_none());
}

#[test]
fn a_multi_line_target_is_cut_to_its_first_line() {
    let call =
        parse_replayed_call("shell {\"command\": \"cargo test\\n--workspace\"}").expect("parsed");
    assert_eq!(call.target.as_deref(), Some("cargo test"));
}
