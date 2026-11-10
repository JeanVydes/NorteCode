use super::*;

#[test]
fn a_pipe_is_not_a_terminal_to_draw_on() {
    // A test run has no terminal on either end, which is the answer this
    // exists to give: a caller that drew anyway would write escape codes
    // into whatever the process was piped to.
    assert!(
        !is_interactive(),
        "a test run has no terminal, and asking has to say so"
    );
}
