//! Terminal setup and teardown.
//!
//! Enters raw mode, the alternate screen, and pushes the key-event
//! enhancement flags the composer needs to tell a newline apart from a send,
//! then restores everything on drop.

use std::io::{self, Stdout};

use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

pub(crate) struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
}

impl TerminalSession {
    /// Enter raw mode, alt screen and mouse capture.
    ///
    /// # Errors
    /// Returns the error the terminal reported; on failure everything is
    /// already restored.
    pub(crate) fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        ) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
            let _ = disable_raw_mode();
            return Err(error);
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut stdout = io::stdout();
                let _ = execute!(stdout, PopKeyboardEnhancementFlags, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(error);
            }
        };
        Ok(Self {
            terminal,
            active: true,
        })
    }

    /// The terminal, for frame drawing.
    pub(crate) fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    /// How to restore. Never panics, never fails silently.
    pub(crate) fn leave(mut self) {
        self.active = false;
        self.restore();
    }

    fn restore(&mut self) {
        let _ = execute!(
            io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.active {
            self.restore();
        }
    }
}
