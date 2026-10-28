//! The footer's context dial: how full the window is and how close the
//! compaction threshold is.

use super::helpers::human_tokens;
use super::helpers::percent_label;
use super::{ReasoningInfo, Style};

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
const DIAL_RUNGS: [(f64, Rung); 4] = [
    // A quarter of the way to the limit: the work is still comfortable.
    (0.25, Rung::Green),
    // Halfway: worth watching.
    (0.50, Rung::Yellow),
    // Three quarters: the next turn may not fit.
    (0.75, Rung::Orange),
    // At the limit: compaction is what happens next.
    (1.00, Rung::Red),
];

/// One colour of the dial's scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rung {
    Green,
    Yellow,
    Orange,
    Red,
}

/// How many cells the context dial draws.
const DIAL_WIDTH: usize = 8;

/// A context-usage dial for the footer.
///
/// It draws a fixed-width ring whose filled share is how much of the model's
/// context window the last request used, with the number beside it. The ring is
/// blue well below the compaction threshold, amber near it, and red at or past
/// it, where `compact` is added. `used` is the last request's input tokens.
///
/// When the model's window is unknown the ring is all-empty and only the raw
/// token count is shown: a missing maximum is not a full context.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub(crate) fn context_dial(
    style: Style,
    used: u64,
    max: Option<u64>,
    threshold: f64,
    compact: bool,
) -> String {
    let Some(max) = max.filter(|max| *max > 0) else {
        return style.dim(format!("ctx {}", human_tokens(used)));
    };
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
    paint(style, &label, towards, threshold > 0.0)
}

/// The colour the dial is written in, by how far towards the limit it is.
///
/// Below the first rung it is the blue the row always used, and above the last
/// it stays red: a dial past its limit has nothing further to say.
#[must_use]
fn paint(style: Style, label: &str, towards: f64, scaled: bool) -> String {
    if !scaled {
        return style.blue(label);
    }
    // Below the first rung the row is the blue it always was: the scale has not
    // started, and a colour there would claim a warning nobody has earned.
    let Some((first, first_rung)) = DIAL_RUNGS.first().copied() else {
        return style.blue(label);
    };
    if towards < first {
        return style.blue(label);
    }
    // Otherwise the rung the reading has reached, and no further.
    let mut chosen = first_rung;
    for (at, rung) in DIAL_RUNGS {
        if towards >= at {
            chosen = rung;
        }
    }
    match chosen {
        Rung::Green => style.green(label),
        Rung::Yellow => style.yellow(label),
        Rung::Orange => style.orange(label),
        Rung::Red => style.red(label),
    }
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

/// How much of the bar an effort fills, from none to all of it.
///
/// The scale is the one the settings are ordered by, and every level is a step
/// of its own so two of them never draw the same bar: a control that cannot be
/// told apart from the one below it is not showing anything. `off` is empty and
/// grey; `max` is full and purple, and the colour says which level it is even
/// when the bar is too short to count the cells.
fn effort_fraction(effort: &str) -> (usize, u8) {
    match effort {
        // off is the empty bar: nothing is filled, and grey is what "nothing"
        // looks like. It is not a low effort, it is the absence of one.
        "off" | "none" | "disabled" => (0, 0),
        // The lowest efforts are "almost nothing, but not nothing", which is why
        // they fill one cell rather than none.
        "minimal" | "low" => (1, 1),
        "medium" => (2, 2),
        "high" => (3, 3),
        "xhigh" => (4, 4),
        "max" => (REASONING_CELLS, 5),
        // `auto` and a token budget are not rungs: they are a setting of another
        // kind, and a bar would claim an effort nobody chose. Seven is the
        // colour the caller paints them, and the label carries the meaning.
        _ => (0, 7),
    }
}

/// The ink a reasoning setting is written in, as a widget style.
///
/// It is the bar's own colour, exposed for the rows that name a setting without
/// drawing one: the suggestion above the input says what Tab would write —
/// `xhigh — setting` — and it means the same thing the bar does, so it reads in
/// the same colour. A person learning the scale from one row and reading the
/// other should not have to learn it twice.
///
/// The rungs are the ones the bar draws, in the order they cost: `off` is grey
/// because it is the absence of an effort, the four middle rungs go green,
/// yellow, orange and red, and `max` is the top of the scale.
#[must_use]
pub(crate) fn effort_span(style: Style, effort: &str) -> ratatui::style::Style {
    match effort_fraction(effort).1 {
        0 => style.faint_span(),
        1 => style.green_span(),
        2 => style.yellow_span(),
        3 => style.orange_span(),
        4 => style.red_span(),
        5 => style.magenta_span(),
        // `auto` and a token budget, which are a setting of another kind.
        _ => style.blue_span(),
    }
}

/// The reasoning effort as a bar, coloured by how much is being asked.
///
/// The footer used to say `think high`, which is a word a person has to map to
/// an order in their head. A bar says where the setting sits on the scale
/// without spelling the scale out, and the colour repeats it for the settings
/// whose names — `xhigh`, `max` — are the ones that cost the most.
///
/// `auto` and a token budget draw an empty bar in their own colour: neither is
/// a rung of the ladder, so neither claims one.
pub(crate) fn reasoning_bar(style: Style, label: &str) -> String {
    let (filled, colour) = effort_fraction(label);
    let text = format!(
        "{}{} {label}",
        "█".repeat(filled),
        "·".repeat(REASONING_CELLS.saturating_sub(filled))
    );
    match colour {
        // `off`: the bar is empty and grey, which is what "no effort" looks
        // like. It is the one level whose colour is not a colour.
        0 => style.faint(text),
        1 => style.green(text),
        2 => style.yellow(text),
        3 => style.orange(text),
        4 => style.red(text),
        5 => style.magenta(text),
        // `auto` and a token budget: a setting of another kind, so the bar stays
        // empty and unnamed rather than claiming an effort, and the label after
        // it is the setting itself.
        _ => style.blue(text),
    }
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
        let code = |used: u64| {
            context_dial(styled(), used, Some(1_000), 0.5, false)
                .split('m')
                .next()
                .unwrap_or_default()
                .to_owned()
        };
        // Below the first rung the row is the blue it always was.
        assert_eq!(code(50), code(10), "the scale has not started");
        // And each rung after it is a colour of its own, in the order they cost.
        let rungs = [code(200), code(300), code(400), code(500)];
        for rung in &rungs {
            assert!(!rung.is_empty(), "every rung paints");
        }
        let mut seen = vec![code(50)];
        seen.extend(rungs);
        let count = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), count, "no two rungs share a colour: {seen:?}");
        // At the limit the word joins the row, so the colour is not the only
        // thing saying it.
        assert!(context_dial(styled(), 500, Some(1_000), 0.5, false).contains("compact"));
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
        assert_ne!(
            loose, tight,
            "the threshold is what the colour is read from"
        );
        // And the limit itself is the threshold: past it, the word that says
        // what happens next is on the row.
        let past = context_dial(styled(), 460, Some(1_000), 0.45, false);
        assert!(!loose.contains("compact"), "not close under the loose one");
        assert!(past.contains("compact"), "past the tight one: {past:?}");
    }

    #[test]
    fn a_window_with_no_maximum_is_not_a_full_one() {
        // A missing maximum is not a full context: there is no fraction to
        // draw, so the dial shows the count and claims nothing.
        let text = context_dial(styled(), 123, None, 0.5, false);
        assert!(text.contains("123"), "{text:?}");
        assert!(!text.contains('\u{2588}'), "nothing is claimed: {text:?}");
        assert!(!text.contains("compact"), "{text:?}");
    }
}
