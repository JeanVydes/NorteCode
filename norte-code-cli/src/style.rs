//! ANSI styling for the terminal renderer.
//!
//! Styling is opt-in by destination: it is enabled only when the stream is a
//! terminal, `NO_COLOR` is not set and `TERM` is not `dumb`. Piped output and
//! `--jsonl` stay plain, so scripts never have to strip escape sequences.

use std::fmt::Display;
use std::io::IsTerminal;

/// Which SGR codes to wrap text in.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    enabled: bool,
}

impl Style {
    /// Styling that never emits escape sequences.
    #[must_use]
    pub fn plain() -> Self {
        Self { enabled: false }
    }

    /// Styling for `stderr`, where the agent's diagnostics go.
    #[must_use]
    pub fn stderr() -> Self {
        Self {
            enabled: supported(std::io::stderr().is_terminal()),
        }
    }

    /// Styling for `stdout`, where reports go.
    #[must_use]
    pub fn stdout() -> Self {
        Self {
            enabled: supported(std::io::stdout().is_terminal()),
        }
    }

    /// Wraps `text` in the given SGR codes.
    #[must_use]
    pub fn paint(self, codes: &str, text: impl Display) -> String {
        if self.enabled {
            format!("\x1b[{codes}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// Bold, for names and headings.
    #[must_use]
    pub fn bold(self, text: impl Display) -> String {
        self.paint("1", text)
    }

    /// Dim, for secondary detail.
    #[must_use]
    pub fn dim(self, text: impl Display) -> String {
        self.paint("2", text)
    }

    /// Dim italic, for the model's reasoning.
    #[must_use]
    pub fn thought(self, text: impl Display) -> String {
        self.paint("2;3", text)
    }

    /// Green, for a successful action.
    #[must_use]
    pub fn green(self, text: impl Display) -> String {
        self.paint("32", text)
    }

    /// Red, for a failure.
    #[must_use]
    pub fn red(self, text: impl Display) -> String {
        self.paint("31", text)
    }

    /// Yellow, for something that needs attention.
    #[must_use]
    pub fn yellow(self, text: impl Display) -> String {
        self.paint("33", text)
    }

    /// Cyan, for tools and prompts.
    #[must_use]
    pub fn cyan(self, text: impl Display) -> String {
        self.paint("36", text)
    }

    /// Blue, for usage and secondary labels.
    #[must_use]
    pub fn blue(self, text: impl Display) -> String {
        self.paint("34", text)
    }

    /// Magenta, for the model identity.
    #[must_use]
    pub fn magenta(self, text: impl Display) -> String {
        self.paint("35", text)
    }

    /// Bright black, for rules and quiet chrome. Kept distinct from [`dim`] so
    /// a theme that renders bold/dim as plain still shows structure.
    ///
    /// [`dim`]: Style::dim
    #[must_use]
    pub fn faint(self, text: impl Display) -> String {
        self.paint("90", text)
    }

    /// Bold cyan, for a tool name.
    #[must_use]
    pub fn tool(self, text: impl Display) -> String {
        self.paint("1;36", text)
    }
}

fn supported(is_terminal: bool) -> bool {
    if !is_terminal {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return false;
    }
    if std::env::var("CLICOLOR").is_ok_and(|value| value == "0") {
        return false;
    }
    true
}
