//! The animated bar that owns the line under the user's message while a
//! turn is in flight.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Instant;

use super::Style;
use crate::duration_label;

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
}

/// An animated loading bar printed below the user's message.
///
/// The bar owns the line under the user's message while a turn is in flight: it
/// writes one carriage-returned line on stderr and clears it on stop. Because
/// the reasoning gutter and the diagnostics share that stream, every writer
/// stops the bar before it emits anything, so the two never interleave.
pub(crate) struct LoadingBar {
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
}

impl LoadingBar {
    /// Animates a bar on its own thread until [`LoadingBar::stop`] is called.
    ///
    /// The thread repaints a single line in place. It is written through
    /// [`Style`], so a terminal without colour shows the frames plain instead of
    /// leaking raw SGR codes.
    pub(crate) fn start(style: Style) -> Self {
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
                // rate below says more than the elapsed time does. A wait fits
                // in a u64 by a wide margin; milliseconds of it would take half
                // a billion years to overflow.
                #[allow(clippy::cast_possible_truncation)]
                let waited = started.elapsed().as_millis() as u64;
                let label = match phase {
                    Phase::Waiting => {
                        format!("{} · {}", phase.label(), duration_label(waited))
                    }
                    Phase::Streaming => phase.label().to_owned(),
                };
                let line = format!(
                    "  {} {}",
                    style.dim(label),
                    loading_frame(style, tick, phase)
                );
                // `\r` returns to the start of the bar's own line and `\x1b[2K`
                // clears it, so a shorter frame never leaves the tail of a long
                // one behind.
                eprint!("\r\x1b[2K{line}");
                let _ = std::io::stderr().flush();
                std::thread::sleep(std::time::Duration::from_millis(LOADING_TICK_MS));
                tick = tick.wrapping_add(1);
            }
            // Leave the cursor at the start of a cleared line for whatever runs
            // after the bar.
            eprint!("\r\x1b[2K");
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
        self.phase.store(
            encode(Phase::Streaming),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Stops the animation and joins its thread, clearing the bar's line.
    pub(crate) fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LoadingBar {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Width of the loading bar, in cells, not counting the label.
pub(crate) const LOADING_BAR_WIDTH: usize = 24;

/// How often the loading bar advances, in milliseconds.
const LOADING_TICK_MS: u64 = 90;

/// One frame of the bar.
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
}
