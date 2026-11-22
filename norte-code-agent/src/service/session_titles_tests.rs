use super::{SessionTitle, TITLE_MAX_OUTPUT_TOKENS};

#[test]
fn a_title_use_the_first_non_empty_line_without_decorations() {
    let title = SessionTitle::parse("\"Login redirect fix\"\nExtra line").expect("title");
    assert_eq!(title.as_str(), "Login redirect fix");
}

#[test]
fn a_title_collapses_whitespace_and_limits_length() {
    let raw = format!("  reparar   el   redirect   {}", "x".repeat(80));
    let title = SessionTitle::parse(&raw).expect("title");
    assert_eq!(title.as_str().chars().count(), 40);
    assert!(!title.as_str().contains("  "));
}

#[test]
fn an_empty_model_answer_is_rejected() {
    assert!(SessionTitle::parse("\n  ```\n").is_err());
}

#[test]
fn a_title_request_leaves_room_for_reasoning_tokens() {
    const { assert!(TITLE_MAX_OUTPUT_TOKENS >= 512) };
}
