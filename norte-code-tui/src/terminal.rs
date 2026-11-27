//! Terminal setup and teardown.
//!
//! Enters raw mode, the alternate screen, and pushes the key-event
//! enhancement flags the composer needs to tell a newline apart from a send,
//! then restores everything on drop.

use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

const ENABLE_MOUSE_MOTION: &[u8] = b"\x1b[?1003h";
const DISABLE_MOUSE_MOTION: &[u8] = b"\x1b[?1003l";

pub(crate) struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
    pub(crate) graphics: Option<crate::ui::chip_preview::KittyPlacement>,
}

/// Set by the signal watcher; the run loop polls it to leave cleanly.
pub(crate) static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Set once the terminal is restored; the restore is idempotent.
static RESTORED: AtomicBool = AtomicBool::new(false);

/// Spawn a thread that flips [`SHUTDOWN`] when the terminal dies.
///
/// A TUI that is killed by the terminal closing (SIGHUP) or by something
/// external (SIGTERM, SIGINT) would otherwise leave the terminal in raw mode
/// with the kitty keyboard flags on, and every later keystroke arrives as
/// escape noise.
pub(crate) fn watch_exit_signals() {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
    use signal_hook::iterator::Signals;

    std::thread::spawn(move || {
        let mut signals = match Signals::new([SIGHUP, SIGINT, SIGQUIT, SIGTERM]) {
            Ok(signals) => signals,
            Err(error) => {
                eprintln!("norte-code: warning: cannot watch exit signals: {error}");
                restore_terminal();
                SHUTDOWN.store(true, Ordering::Relaxed);
                return;
            }
        };
        let _ = signals.forever().next();
        restore_terminal();
        SHUTDOWN.store(true, Ordering::Relaxed);
    });
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
        if let Err(error) = enter_screen(&mut stdout) {
            restore_terminal();
            return Err(error);
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                restore_terminal();
                return Err(error);
            }
        };
        Ok(Self {
            terminal,
            active: true,
            graphics: (std::env::var("TERM").as_deref() == Ok("xterm-kitty")
                && std::env::var_os("ZELLIJ").is_none()
                && std::env::var_os("TMUX").is_none())
            .then(Default::default),
        })
    }

    /// The terminal, for frame drawing.
    pub(crate) fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    /// How to restore. Never panics, never fails silently.
    pub(crate) fn leave(mut self) {
        self.active = false;
        restore_terminal();
    }
}

fn enter_screen(output: &mut impl Write) -> io::Result<()> {
    // Keyboard enhancement state belongs to the active terminal screen.
    execute!(
        output,
        EnterAlternateScreen,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        ),
        EnableMouseCapture,
        crossterm::event::EnableBracketedPaste
    )?;
    output.write_all(ENABLE_MOUSE_MOTION)
}

fn leave_screen(output: &mut impl Write) -> io::Result<()> {
    output.write_all(DISABLE_MOUSE_MOTION)?;
    execute!(
        output,
        crossterm::style::ResetColor,
        crossterm::cursor::Show,
        crossterm::event::DisableBracketedPaste,
        DisableMouseCapture,
        PopKeyboardEnhancementFlags,
        LeaveAlternateScreen
    )
}

/// Give the terminal back completely; idempotent.
///
/// Resets the text attributes, shows the cursor, drops the mouse and the
/// alternate screen, pops the kitty enhancement flags, and re-enables
/// canonical mode. Writes to the controlling terminal, not just stdout, so
/// a redirect cannot strand escape bytes. Anything that fails is reported
/// to stderr once the shell can see it.
pub(crate) fn restore_terminal() {
    if RESTORED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut errors: Vec<String> = Vec::new();
    let mut out: Box<dyn io::Write> = match std::fs::OpenOptions::new().write(true).open("/dev/tty")
    {
        Ok(file) => Box::new(file),
        Err(_) => Box::new(io::stdout()),
    };
    if let Err(error) = leave_screen(&mut out) {
        errors.push(format!("the escape sequence restore failed: {error}"));
    }
    if let Err(error) = disable_raw_mode() {
        errors.push(format!("the tty mode restore failed: {error}"));
    }
    for error in errors {
        eprintln!("norte-code: warning: {error}");
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.active {
            restore_terminal();
        }
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
