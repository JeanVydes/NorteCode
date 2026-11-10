use super::*;

#[test]
fn keeps_requested_output_when_it_fits_context() {
    let messages = vec![Message::user("hello")];
    let tools = Vec::new();

    assert_eq!(
        context_safe_output(Some(8_192), Some(128_000), &messages, &tools),
        Some(8_192)
    );
}

#[test]
fn clamps_output_to_remaining_context() {
    let messages = vec![Message::user("x".repeat(40_000))];
    let tools = Vec::new();

    let output = context_safe_output(Some(100_000), Some(20_000), &messages, &tools)
        .expect("requested output");
    assert!(output < 100_000);
    assert!(output <= 20_000 - CONTEXT_SAFETY_MARGIN);
}

#[test]
fn leaves_unbounded_output_unset() {
    let messages = vec![Message::user("hello")];
    let tools = Vec::new();

    assert_eq!(
        context_safe_output(None, Some(128_000), &messages, &tools),
        None
    );
}
