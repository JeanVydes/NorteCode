"""Cover the bar coming back after a tool, and the phase it shows."""

import pathlib

path = pathlib.Path("norte-code-cli/src/render/tests.rs")
text = path.read_text()

ANCHOR = """fn loading_frame_is_legible_without_colour() {"""
assert text.count(ANCHOR) == 1, text.count(ANCHOR)

NEW = '''#[test]
fn the_bar_comes_back_after_a_tool() {
    // The bar stops on the first answer token, which is right; a turn that
    // called a tool then goes on, and it used to do so with no signal at all.
    let pinned = Pinned::new();
    let mut renderer = visible_renderer(&pinned);
    renderer.mark_working();
    assert!(renderer.loading_bar.is_some(), "the bar starts with the turn");

    // The model answers with a call, which stops the bar like any answer.
    renderer.delta(&Delta::Text {
        index: 0,
        text: "calling a tool".to_owned(),
    });
    renderer.end_blocks();
    assert!(
        renderer.loading_bar.is_none(),
        "the answer stops the bar, as before"
    );

    // The tool finishes; the turn is not over, so the bar returns.
    renderer.tool_event(&tool_finished("call-1", "write", false));
    assert!(
        renderer.loading_bar.is_some(),
        "the bar is back for what the model says next"
    );
}

#[test]
fn the_frame_shape_tells_the_two_phases_apart() {
    // While waiting the highlight travels; while streaming it fills from the
    // left, so the shape says which one it is without reading the label.
    let waiting = loading_frame(Style::plain(), 0, Phase::Waiting);
    let streaming = loading_frame(Style::plain(), 0, Phase::Streaming);
    assert_ne!(waiting, streaming, "the two phases do not look alike");
    // Streaming is a fill: the first cell is painted and the last is not at
    // tick zero.
    assert!(streaming.starts_with('█'), "{streaming:?}");
    assert!(streaming.ends_with('░'), "{streaming:?}");
}

fn loading_frame_is_legible_without_colour() {'''

text = text.replace(ANCHOR, NEW, 1)
path.write_text(text)
print("ok")
