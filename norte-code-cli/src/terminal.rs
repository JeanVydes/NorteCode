//! The terminal: who owns it, how a frame is drawn into it, and what it is
//! restored to.
//!
//! One owner. Before this, four things wrote escapes to stderr — the editor,
//! the renderer, the approval panel and the loading bar — and each kept its own
//! idea of where the cursor was. That is where the bugs came from, and it is
//! what a frame does away with: widgets describe what they want, the layout
//! decides where it goes, and one place writes the result.
//!
//! # The buffer
//!
//! ratatui draws into a [`Buffer`] of cells and diffs it against the previous
//! frame, so nothing is written that did not change. That is what makes a tick
//! cheap: a spinner moving one cell rewrites one cell, not the screen.
//!
//! # The guard
//!
//! [`Guard`] puts the terminal back on drop: raw mode off, cursor shown,
//! bracketed paste off. A panic, an early return and an ordinary exit all end
//! up in the same place, which matters because a terminal left in raw mode with
//! no cursor is the one failure a person cannot fix without closing the window.
//!
//! # Sizing
//!
//! Nothing here holds a width or a height. The size is read from the terminal
//! when asked and handed to the layout, so a resize is one more frame and not a
//! migration: the layout is what decides, and it decides from what the terminal
//! says at that moment.
//!
//! [`Buffer`]: ratatui::buffer::Buffer

use std::io::{self, IsTerminal, Write};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

/// A frame, as the drawing functions see it.
pub(crate) type Draw<'a> = Frame<'a>;

/// Whether this process has a terminal to draw on.
///
/// Both ends are asked: stdin because that is where the keys come from, stdout
/// because that is where the frames go. Either being a pipe means there is
/// nobody at the other end to read a screen, and the caller prints lines
/// instead of drawing one.
#[must_use]
pub(crate) fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Owns the terminal and gives it back on drop.
pub(crate) struct Guard {
    /// The screen, taken back on the way out so its own restore runs while the
    /// terminal is still in raw mode.
    screen: Option<Terminal<CrosstermBackend<io::Stdout>>>,
}

impl Guard {
    /// Takes the terminal over.
    ///
    /// The alternate screen is entered, and the scrollback the terminal had is
    /// therefore put away while the session runs. That is the trade a
    /// full-screen UI makes, and the client keeps its own transcript: scrolling
    /// back through the conversation is a view over the events this process
    /// already holds, not a bet on what the terminal still remembers.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal refuses raw mode or the alternate
    /// screen, and leaves nothing changed when it does.
    pub(crate) fn enter() -> Result<Self, String> {
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen).map_err(|error| error.to_string())?;
        // Raw mode is what turns a keystroke into an event instead of a line,
        // and it is what clears `ISIG`: without that the terminal turns Ctrl-C
        // into a signal and the editor never sees the key.
        if let Err(error) = enable_raw_mode() {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            return Err(format!(
                "could not switch the terminal to raw mode: {error}"
            ));
        }
        // Bracketed paste makes a pasted block one event instead of a burst of
        // keystrokes: without it a pasted paragraph arrives as typing and its
        // newlines submit it line by line.
        //
        // The keyboard protocol is what makes a modified Enter distinguishable:
        // a terminal that does not report it sends the same carriage return for
        // Enter and Shift+Enter, so the newline branch never fires and a draft
        // can never be more than one line. A terminal that does not support the
        // sequence ignores it, and the pop on the way out is then a no-op.
        let _ = execute!(
            out,
            EnableBracketedPaste,
            // Mouse capture is what makes the wheel scroll the conversation. A
            // terminal without it sends nothing for a wheel event at all, so
            // there is no fallback to keep: the keys are the other way to read
            // what scrolled past.
            EnableMouseCapture,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        let screen = Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                // Fullscreen uses the whole alternate screen.
                viewport: Viewport::Fullscreen,
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            screen: Some(screen),
        })
    }

    /// Draws one frame.
    ///
    /// Called once per change, never in a loop: ratatui diffs against the
    /// previous frame, so a call with nothing changed writes nothing, and a
    /// caller spinning on it would burn a core to produce empty output.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal is gone, which means the process is
    /// ending anyway.
    pub(crate) fn draw(&mut self, body: impl FnOnce(&mut Draw<'_>)) -> Result<(), String> {
        let Some(screen) = self.screen.as_mut() else {
            return Ok(());
        };
        screen
            .draw(|frame| body(frame))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Puts text on the clipboard of the terminal the client is running in.
///
/// It is OSC 52: the sequence carries the bytes and the terminal writes them to
/// whatever clipboard the person is actually using. That is what makes it work
/// over ssh and inside tmux, where a program on this side has no reach into the
/// other one — and it is why there is no clipboard crate here: the only
/// clipboard that matters is the one belonging to the terminal a person is
/// looking at, and only they can write to it.
///
/// The payload is bounded by what a terminal accepts. What a person copies out
/// of a conversation is a paragraph, not a megabyte.
///
/// # Errors
///
/// Returns an error when the terminal refuses the write.
pub(crate) fn copy(text: &str) -> Result<(), String> {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = io::stdout();
    // `c` is the clipboard; `p` would be the primary selection.
    write!(out, "\x1b]52;c;{encoded}\x07").map_err(|error| error.to_string())?;
    out.flush().map_err(|error| error.to_string())
}

impl Drop for Guard {
    fn drop(&mut self) {
        // The screen goes first: its own restore runs while the terminal is
        // still in raw mode and the cursor is still hidden.
        drop(self.screen.take());
        let mut out = io::stdout();
        let _ = execute!(
            out,
            PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let _ = out.flush();
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
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
}
