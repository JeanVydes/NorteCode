//! The footer's context dial and reasoning bar: the two scales the status row
//! draws, each as a painted string for the log and as widget ink for the frame.

use super::helpers::human_tokens;
use super::helpers::percent_label;
use super::{ReasoningInfo, Style};

/// One colour of the footer's scales.
///
/// The two rows on screen at once — the context dial and the reasoning bar — and
/// the log a person scrolls back through must read from one palette. A field
/// says which colour it wants and the destination decides how to paint it: an
/// SGR string for the plain-text path, a widget style for the frame. Keeping the
/// choice in one enum is what stops the two from drifting apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ink {
    /// The quietest reading, a step below dim.
    Faint,
    /// A secondary label or a number.
    Dim,
    /// A name: a session's title, a tool.
    Cyan,
    /// Usage and secondary labels.
    Blue,
    /// A comfortable reading, and a call that finished.
    Green,
    /// Something worth watching.
    Yellow,
    /// An effort above the middle.
    Orange,
    /// The top of a scale: the model identity, `max` effort.
    Magenta,
    /// At or past a limit.
    Red,
}

impl Ink {
    /// The ink as the SGR-painted string the log is written with.
    pub(crate) fn paint(self, style: Style, text: &str) -> String {
        match self {
            Self::Faint => style.faint(text),
            Self::Dim => style.dim(text),
            Self::Cyan => style.cyan(text),
            Self::Blue => style.blue(text),
            Self::Green => style.green(text),
            Self::Yellow => style.yellow(text),
            Self::Orange => style.orange(text),
            Self::Magenta => style.magenta(text),
            Self::Red => style.red(text),
        }
    }

    /// The ink as the widget style the frame draws a span with.
    ///
    /// The same colour [`Ink::paint`] writes, so a person reading the log and a
    /// person reading the frame see the one palette.
    pub(crate) fn span(self, style: Style) -> ratatui::style::Style {
        match self {
            Self::Faint => style.faint_span(),
            Self::Dim => style.dim_span(),
            Self::Cyan => style.cyan_span(),
            Self::Blue => style.blue_span(),
            Self::Green => style.green_span(),
            Self::Yellow => style.yellow_span(),
            Self::Orange => style.orange_span(),
            Self::Magenta => style.magenta_span(),
            Self::Red => style.red_span(),
        }
    }
}

/// One cell of the context dial, from empty to full.
const DIAL_CELLS: [char; 4] = ['░', '▒', '▓', '█'];

/// Where each colour of the dial begins, as a fraction *of the threshold*.
///
/// The dial answers one question — how close is the compaction — so its scale
/// is the threshold and not the window: the same share of a small model's window
/// and a large one's means the same thing. A ring that turned red at a fixed
/// fraction of the window would warn late on a model whose threshold is low and
/// early on one whose threshold is high, which is the opposite of what a warning
/// is for.
///
/// The rungs are the ones the effort bar uses, for the same reason: a person
/// reads both rows at once and should not have to learn two scales.
const DIAL_RUNGS: [(f64, Ink); 4] = [
    // A quarter of the way to the limit: the work is still comfortable.
    (0.25, Ink::Green),
    // Halfway: worth watching.
    (0.50, Ink::Yellow),
    // Three quarters: the next turn may not fit.
    (0.75, Ink::Orange),
    // At the limit: compaction is what happens next.
    (1.00, Ink::Red),
];

/// The colour the dial has reached, or `None` below the first rung.
///
/// Below the first rung the scale has not started, and a colour there would
/// claim a warning nobody has earned; that is the quiet blue.
fn dial_ink(towards: f64) -> Option<Ink> {
    let (first, _) = DIAL_RUNGS.first().copied()?;
    if towards < first {
        return None;
    }
    let mut chosen = DIAL_RUNGS[0].1;
    for (at, ink) in DIAL_RUNGS {
        if towards >= at {
            chosen = ink;
        }
    }
    Some(chosen)
}

/// How many cells the context dial draws.
const DIAL_WIDTH: usize = 8;

/// The dial's text and how close it is to the limit, or `None` when the model's
/// window is unknown.
///
/// A missing maximum is not a full context: there is no fraction to draw, so the
/// caller shows the count alone.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn dial_shape(used: u64, max: u64, threshold: f64, compact: bool) -> (String, f64) {
    let fraction = (used as f64 / max as f64).clamp(0.0, 1.0);
    let filled_cells = fraction * DIAL_WIDTH as f64;
    let full = filled_cells as usize;
    let mut ring = String::with_capacity(DIAL_WIDTH);
    for cell in 0..DIAL_WIDTH {
        if cell < full {
            ring.push('█');
        } else if (cell as f64) < filled_cells {
            let partial = ((filled_cells - cell as f64) * 4.0) as usize;
            ring.push(DIAL_CELLS[partial.min(3)]);
        } else {
            ring.push('░');
        }
    }
    // How far towards the compaction limit the window is. With no threshold
    // there is no scale to warn on, so the dial is the plain reading of the
    // window it always was.
    let towards = if threshold > 0.0 {
        fraction / threshold
    } else {
        0.0
    };
    let label = format!("ctx {} {}", percent_label(used, max), ring);
    // At or past the limit the word joins the row: the colour says it is close
    // and the word says what close means.
    let over = threshold > 0.0 && fraction >= threshold;
    let label = if over || compact {
        format!("{label} ⟳ compact")
    } else {
        label
    };
    (label, towards)
}

/// The context dial: its text and the ink it is drawn in.
///
/// The ring's filled share is how much of the model's window the last request
/// used, with the number beside it. The colour is the dial's scale — quiet blue
/// well below the compaction threshold, green, yellow and orange as it
/// approaches, red at or past it, where `compact` joins the row. `used` is the
/// last request's input tokens. With no window there is no scale, so the count
/// is shown alone in the quiet reading.
pub(crate) fn context_dial(
    style: Style,
    used: u64,
    max: Option<u64>,
    threshold: f64,
    compact: bool,
) -> (Ink, String) {
    let Some(max) = max.filter(|max| *max > 0) else {
        return (Ink::Dim, format!("ctx {}", human_tokens(used)));
    };
    let (label, towards) = dial_shape(used, max, threshold, compact);
    let ink = dial_ink(towards).unwrap_or(Ink::Blue);
    let _ = style;
    (ink, label)
}

/// The input-to-output ratio, the number a cost conversation usually asks
/// about. A turn with no output is unbounded rather than zero.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn token_ratio(input: u64, output: u64) -> String {
    if input == 0 && output == 0 {
        return "–".to_owned();
    }
    if output == 0 {
        return "∞".to_owned();
    }
    let ratio = input as f64 / output as f64;
    if ratio >= 10.0 {
        format!("{ratio:.0}:1")
    } else {
        format!("{ratio:.1}:1")
    }
}

pub(crate) fn reasoning_label(reasoning: &ReasoningInfo) -> String {
    if !reasoning.supported {
        return "-".to_owned();
    }
    if reasoning.efforts.is_empty() {
        return if reasoning.can_disable {
            "auto/off".to_owned()
        } else {
            "auto".to_owned()
        };
    }
    reasoning.efforts.join(",")
}

/// How many cells the reasoning bar draws.
const REASONING_CELLS: usize = 4;

/// The cells an effort fills and the ink it is drawn in.
///
/// The scale is the one the settings are ordered by, and every level is a step
/// of its own so two of them never draw the same bar: a control that cannot be
/// told apart from the one below it is not showing anything. `off` is empty and
/// grey; `max` is full and magenta, and the colour says which level it is even
/// when the bar is too short to count the cells.
fn effort_ink(effort: &str) -> (usize, Ink) {
    match effort {
        // off is the empty bar: nothing is filled, and grey is what "nothing"
        // looks like. It is not a low effort, it is the absence of one.
        "off" | "none" | "disabled" => (0, Ink::Faint),
        // The lowest efforts are "almost nothing, but not nothing", which is why
        // they fill one cell rather than none.
        "minimal" | "low" => (1, Ink::Green),
        "medium" => (2, Ink::Yellow),
        "high" => (3, Ink::Orange),
        "xhigh" => (4, Ink::Red),
        "max" => (REASONING_CELLS, Ink::Magenta),
        // `auto` and a token budget are not rungs: they are a setting of another
        // kind, and a bar would claim an effort nobody chose. The label carries
        // the meaning and the blue says it is not on the ladder.
        _ => (0, Ink::Blue),
    }
}

/// The ink a reasoning setting is written in.
///
/// It is the bar's own colour, exposed for the rows that name a setting without
/// drawing one: the suggestion above the input says what Tab would write —
/// `xhigh — setting` — and it means the same thing the bar does, so it reads in
/// the same colour. A person learning the scale from one row and reading the
/// other should not have to learn it twice.
#[must_use]
pub(crate) fn effort_span(style: Style, effort: &str) -> ratatui::style::Style {
    effort_ink(effort).1.span(style)
}

/// The reasoning effort as a bar: its text and the ink it is drawn in.
///
/// The footer used to say `think high`, which is a word a person has to map to
/// an order in their head. A bar says where the setting sits on the scale
/// without spelling the scale out, and the colour repeats it for the settings
/// whose names — `xhigh`, `max` — are the ones that cost the most.
///
/// `auto` and a token budget draw an empty bar in their own colour: neither is
/// a rung of the ladder, so neither claims one.
pub(crate) fn reasoning_bar(label: &str) -> (Ink, String) {
    let (filled, ink) = effort_ink(label);
    let text = format!(
        "{}{} {label}",
        "█".repeat(filled),
        "·".repeat(REASONING_CELLS.saturating_sub(filled))
    );
    (ink, text)
}

#[cfg(test)]
mod tests {
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
}
