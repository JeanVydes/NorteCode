use super::*;

fn area(height: u16) -> Rect {
    Rect::new(0, 0, 40, height)
}

#[test]
fn the_conversation_takes_what_the_chrome_does_not() {
    let heights = Heights {
        suggestion: 1,
        rule: 1,
        status: 1,
        input: 2,
        ..Heights::default()
    };
    let areas = split(area(20), heights);
    assert_eq!(areas.conversation.height, 15);
    assert_eq!(areas.input.height, 2);
    assert_eq!(areas.status.y, 19);
    assert_eq!(areas.rule.y, 18);
    // The bands are contiguous: nothing is drawn twice and no row is lost.
    assert_eq!(areas.conversation.bottom(), areas.input.y);
    assert_eq!(areas.input.bottom(), areas.suggestion.y);
    assert_eq!(areas.suggestion.bottom(), areas.rule.y);
}

#[test]
fn a_short_terminal_gives_up_the_conversation_before_the_chrome() {
    let heights = Heights {
        status: 1,
        rule: 1,
        input: 3,
        ..Heights::default()
    };
    let areas = split(area(4), heights);
    assert_eq!(areas.conversation.height, 0);
    assert_eq!(
        areas.input.height, 3,
        "the chrome is clipped, not the frame"
    );
    assert_eq!(areas.status.height, 0, "what does not fit is dropped");
}
