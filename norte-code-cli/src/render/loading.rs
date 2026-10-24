//! The animated bar that owns the line under the user's message while a
//! turn is in flight.

use std::io::Write as _;
use std::sync::Arc;

use super::Style;

/// An animated loading bar printed below the user's message.
///
/// The bar owns the line under the user's message while a turn is in flight: it
/// writes one carriage-returned line on stderr and clears it on stop. Because
/// the reasoning gutter and the diagnostics share that stream, every writer
/// stops the bar before it emits anything, so the two never interleave.
pub(crate) struct LoadingBar {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl LoadingBar {
    /// Animates a bar on its own thread until [`LoadingBar::stop`] is called.
    ///
    /// The thread repaints a single line in place. It is written through
    /// [`Style`], so a terminal without colour shows the frames plain instead of
    /// leaking raw SGR codes.
    pub(crate) fn start(style: Style) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut tick = 0_usize;
            while !stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                let line = format!("  {} {}", style.dim("working…"), loading_frame(style, tick));
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
            handle: Some(handle),
        }
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

/// One frame of the bar: a highlight that travels left to right.
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
}
