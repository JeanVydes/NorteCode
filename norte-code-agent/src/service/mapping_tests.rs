use super::shorten_tool_output;

#[test]
fn tool_output_is_cut_at_the_configured_character_limit() {
    let text = "a".repeat(20);
    let (shortened, truncated) = shorten_tool_output(text, 5);
    assert_eq!(shortened, "aaaaa\n… [truncated]");
    assert!(truncated);
}

#[test]
fn tool_output_within_the_configured_limit_is_untouched() {
    let (shortened, truncated) = shorten_tool_output("short".to_owned(), 5);
    assert_eq!(shortened, "short");
    assert!(!truncated);
}
