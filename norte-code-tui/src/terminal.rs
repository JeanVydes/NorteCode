//! Terminal setup and teardown.
//!
//! Enters raw mode, the alternate screen, and pushes the key-event
//! enhancement flags the composer needs to tell a newline apart from a send,
//! then restores everything on drop.

use std::io::{self, Stdout};
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

pub(crate) struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
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
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async {
            let Ok(mut interrupt) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            else {
                return;
            };
            let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            else {
                return;
            };
            let Ok(mut hangup) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            else {
                return;
            };
            let Ok(mut quit) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::quit())
            else {
                return;
            };
            tokio::select! {
                _ = interrupt.recv() => {}
                _ = terminate.recv() => {}
                _ = hangup.recv() => {}
                _ = quit.recv() => {}
            }
            restore_terminal();
            SHUTDOWN.store(true, Ordering::Relaxed);
        });
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
        if let Err(error) = execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        ) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
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
    if let Err(error) = execute!(
        out,
        crossterm::style::ResetColor,
        crossterm::cursor::Show,
        DisableMouseCapture,
        LeaveAlternateScreen,
        PopKeyboardEnhancementFlags,
    ) {
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
