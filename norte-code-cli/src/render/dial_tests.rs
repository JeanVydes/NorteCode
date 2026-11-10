use super::*;
use crate::style::Colors;

/// A style that paints, so a test can tell which rung was chosen from the
/// colour that came out.
fn styled() -> Style {
    Style::for_tests(Colors::True)
}

#[test]
fn the_dial_turns_a_rung_at_a_time_towards_the_limit() {
    // The scale is the threshold and not the window: what is being warned
    // about is how close the compaction is, and the same share of a small
    // model's window and a large one's means the same thing. Under a 0.5
    // threshold, a quarter of the way there is 12.5% of the window.
    let ink = |used: u64| context_dial(styled(), used, Some(1_000), 0.5, false).0;
    // Below the first rung the row is the quiet blue it always was.
    assert_eq!(ink(50), Ink::Blue, "the scale has not started");
    // Each rung after it is a colour of its own, in the order they cost.
    assert_eq!(ink(200), Ink::Green);
    assert_eq!(ink(300), Ink::Yellow);
    assert_eq!(ink(400), Ink::Orange);
    assert_eq!(ink(500), Ink::Red);
    // At the limit the word joins the row, so the colour is not the only
    // thing saying it.
    assert!(
        context_dial(styled(), 500, Some(1_000), 0.5, false)
            .1
            .contains("compact")
    );
}

#[test]
fn a_threshold_that_moved_moves_the_colour() {
    // The bug this exists for: the threshold was read once at startup, so a
    // value written into the config and picked up by `/refresh` changed how
    // the session compacts without changing the colour the bar warns in.
    // The same usage is a different colour under a different limit.
    let used = 400;
    let loose = context_dial(styled(), used, Some(1_000), 0.9, false);
    let tight = context_dial(styled(), used, Some(1_000), 0.45, false);
    assert_ne!(loose.0, tight.0, "the threshold is the colour's scale");
    // And the limit itself is the threshold: past it, the word that says
    // what happens next is on the row.
    let past = context_dial(styled(), 460, Some(1_000), 0.45, false);
    assert!(
        !loose.1.contains("compact"),
        "not close under the loose one"
    );
    assert!(past.1.contains("compact"), "past the tight one: {past:?}");
}

#[test]
fn a_window_with_no_maximum_is_not_a_full_one() {
    // A missing maximum is not a full context: there is no fraction to
    // draw, so the dial shows the count and claims nothing.
    let (ink, text) = context_dial(styled(), 123, None, 0.5, false);
    assert_eq!(ink, Ink::Dim, "the count is the quiet reading");
    assert!(text.contains("123"), "{text:?}");
    assert!(!text.contains('\u{2588}'), "nothing is claimed: {text:?}");
    assert!(!text.contains("compact"), "{text:?}");
}

#[test]
fn every_effort_has_its_own_rung_and_ink() {
    // Each level is a step of its own, in the order they cost: a control
    // that cannot be told apart from the one below it shows nothing.
    let pairs = [
        ("off", 0, Ink::Faint),
        ("minimal", 1, Ink::Green),
        ("low", 1, Ink::Green),
        ("medium", 2, Ink::Yellow),
        ("high", 3, Ink::Orange),
        ("xhigh", 4, Ink::Red),
        ("max", REASONING_CELLS, Ink::Magenta),
        // `auto` and a token budget are a setting of another kind.
        ("auto", 0, Ink::Blue),
    ];
    for (label, filled, ink) in pairs {
        let (got_ink, text) = reasoning_bar(label);
        assert_eq!(got_ink, ink, "`{label}` is {ink:?}");
        assert!(
            text.starts_with(&"█".repeat(filled)),
            "`{label}` fills {filled}: {text:?}"
        );
    }
    // The scale's middle rungs are four colours of their own.
    let rungs = ["low", "medium", "high", "xhigh", "max"];
    let inks: Vec<Ink> = rungs.iter().map(|label| reasoning_bar(label).0).collect();
    let mut seen = inks.clone();
    seen.dedup();
    assert_eq!(seen.len(), inks.len(), "no two rungs share a colour");
}
