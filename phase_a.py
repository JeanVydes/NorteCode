"""Make the loading bar say which phase the turn is in.

The bar said `working…` from the moment the turn started until it stopped,
whether the request had been sent, the provider was thinking, or tokens were
already arriving. A slow first token therefore looked like a hang: twelve
seconds of a bar that never changed and never said how long it had been.

The bar now reads a shared phase and the instant the turn started, so it can
say `waiting for the first token · 8s` and then `streaming`.
"""

import pathlib

path = pathlib.Path("norte-code-cli/src/render/loading.rs")
text = path.read_text()


def sub(old, new, why, count=1):
    global text
    found = text.count(old)
    assert found == count, (why, found, count)
    text = text.replace(old, new)


sub(
    """use std::io::Write as _;
use std::sync::Arc;

use super::Style;""",
    """use std::io::Write as _;
use std::sync::Arc;
use std::time::Instant;

use super::Style;

/// What the turn is waiting for.
///
/// The client works this out from the events it receives: nothing is emitted
/// between the turn starting and the first delta, so the absence of a delta is
/// what "waiting for the first token" means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The request is on its way to the provider.
    Waiting,
    /// The provider is answering.
    Streaming,
}

impl Phase {
    /// The label the bar shows for this phase.
    fn label(self) -> &'static str {
        match self {
            Self::Waiting => "waiting for the first token",
            Self::Streaming => "streaming",
        }
    }
}""",
    "phase",
)

sub(
    """pub(crate) struct LoadingBar {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}""",
    """pub(crate) struct LoadingBar {
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// Which phase the bar shows. The renderer flips it when a delta lands.
    phase: Arc<std::sync::atomic::AtomicU8>,
    handle: Option<std::thread::JoinHandle<()>>,
}

/// The phase as the atomic holds it: the variant's discriminant.
fn encode(phase: Phase) -> u8 {
    match phase {
        Phase::Waiting => 0,
        Phase::Streaming => 1,
    }
}

fn decode(value: u8) -> Phase {
    if value == encode(Phase::Streaming) {
        Phase::Streaming
    } else {
        Phase::Waiting
    }
}""",
    "struct",
)

sub(
    """    pub(crate) fn start(style: Style) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut tick = 0_usize;
            while !stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                let line = format!("  {} {}", style.dim("working…"), loading_frame(style, tick));
                // `\\r` returns to the start of the bar's own line and `\\x1b[2K`
                // clears it, so a shorter frame never leaves the tail of a long
                // one behind.
                eprint!("\\r\\x1b[2K{line}");
                let _ = std::io::stderr().flush();
                std::thread::sleep(std::time::Duration::from_millis(LOADING_TICK_MS));
                tick = tick.wrapping_add(1);
            }
            // Leave the cursor at the start of a cleared line for whatever runs
            // after the bar.
            eprint!("\\r\\x1b[2K");
            let _ = std::io::stderr().flush();
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }""",
    """    pub(crate) fn start(style: Style) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let phase = Arc::new(std::sync::atomic::AtomicU8::new(encode(Phase::Waiting)));
        let stop_clone = Arc::clone(&stop);
        let phase_clone = Arc::clone(&phase);
        let started = Instant::now();
        let handle = std::thread::spawn(move || {
            let mut tick = 0_usize;
            while !stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                let phase = decode(phase_clone.load(std::sync::atomic::Ordering::Relaxed));
                // The wait is the thing worth counting: once tokens arrive the
                // rate below says more than the elapsed time does.
                let label = match phase {
                    Phase::Waiting => format!(
                        "{} · {}",
                        phase.label(),
                        duration_label(started.elapsed().as_millis() as u64)
                    ),
                    Phase::Streaming => phase.label().to_owned(),
                };
                let line = format!(
                    "  {} {}",
                    style.dim(label),
                    loading_frame(style, tick, phase)
                );
                // `\\r` returns to the start of the bar's own line and `\\x1b[2K`
                // clears it, so a shorter frame never leaves the tail of a long
                // one behind.
                eprint!("\\r\\x1b[2K{line}");
                let _ = std::io::stderr().flush();
                std::thread::sleep(std::time::Duration::from_millis(LOADING_TICK_MS));
                tick = tick.wrapping_add(1);
            }
            // Leave the cursor at the start of a cleared line for whatever runs
            // after the bar.
            eprint!("\\r\\x1b[2K");
            let _ = std::io::stderr().flush();
        });
        Self {
            stop,
            phase,
            handle: Some(handle),
        }
    }

    /// Records that the provider started answering.
    pub(crate) fn streaming(&self) {
        self.phase
            .store(encode(Phase::Streaming), std::sync::atomic::Ordering::Relaxed);
    }""",
    "start",
)

# The frame itself: a travelling highlight while waiting, a filling bar while
# answering, so the shape alone says whether anything is arriving.
sub(
    """/// One frame of the bar: a highlight that travels left to right.
///
/// The cells are painted through [`Style`], so a terminal without colour draws a
/// legible plain frame instead of leaking raw SGR codes.
pub(crate) fn loading_frame(style: Style, tick: usize) -> String {
    // `head` stays below the bar width, so `abs_diff` on `usize` cannot wrap.
    let head = tick % (LOADING_BAR_WIDTH + 4);
    let mut frame = String::with_capacity(LOADING_BAR_WIDTH * 4);
    for cell in 0..LOADING_BAR_WIDTH {
        let distance = cell.abs_diff(head);
        if distance < 2 {
            frame.push_str(&style.bold("█"));
        } else if distance < 4 {
            frame.push_str(&style.faint("▓"));
        } else {
            frame.push_str(&style.dim("░"));
        }
    }
    frame
}""",
    """/// One frame of the bar.
///
/// While the turn waits for the provider the highlight travels, which says that
/// time is passing and nothing else; once tokens arrive it fills from the left,
/// so the shape alone tells the two apart.
///
/// The cells are painted through [`Style`], so a terminal without colour draws a
/// legible plain frame instead of leaking raw SGR codes.
pub(crate) fn loading_frame(style: Style, tick: usize, phase: Phase) -> String {
    // `head` stays below the bar width, so `abs_diff` on `usize` cannot wrap.
    let head = tick % (LOADING_BAR_WIDTH + 4);
    let mut frame = String::with_capacity(LOADING_BAR_WIDTH * 4);
    for cell in 0..LOADING_BAR_WIDTH {
        match phase {
            Phase::Waiting => {
                let distance = cell.abs_diff(head);
                if distance < 2 {
                    frame.push_str(&style.bold("█"));
                } else if distance < 4 {
                    frame.push_str(&style.faint("▓"));
                } else {
                    frame.push_str(&style.dim("░"));
                }
            }
            Phase::Streaming => {
                if cell <= head {
                    frame.push_str(&style.bold("█"));
                } else {
                    frame.push_str(&style.dim("░"));
                }
            }
        }
    }
    frame
}""",
    "frame",
)

# The elapsed time needs a formatter from the render helper.
sub(
    """use super::Style;""",
    """use super::Style;
use super::helpers::duration_label;""",
    "import",
)

path.write_text(text)
print("ok")
