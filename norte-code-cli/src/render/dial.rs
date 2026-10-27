//! The footer's context dial: how full the window is and how close the
//! compaction threshold is.

use super::helpers::human_tokens;
use super::helpers::percent_label;
use super::{ReasoningInfo, Source, Style};

/// One cell of the context dial, from empty to full.
const DIAL_CELLS: [char; 4] = ['░', '▒', '▓', '█'];

/// How close to the compaction limit the context must be before the dial warns.
const DIAL_WARN_FRACTION: f64 = 0.8;

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
    // The window is over the limit once the used fraction reaches the threshold.
    let over = threshold > 0.0 && fraction >= threshold;
    let near = !over && threshold > 0.0 && fraction >= threshold * DIAL_WARN_FRACTION;
    let label = format!("ctx {} {}", percent_label(used, max), ring);
    if over || compact {
        format!("{} {}", style.red(label), style.red("⟳ compact"))
    } else if near {
        style.yellow(label)
    } else {
        style.blue(label)
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

pub(crate) fn source_label(source: Source) -> &'static str {
    match source {
        Source::Cache | Source::NotModified => "cache",
        Source::Network => "network",
        Source::StaleCache => "stale cache",
        Source::Bundled => "bundled snapshot",
    }
}
