//! ANSI styling for the terminal renderer.
//!
//! Styling is opt-in by destination: it is enabled only when the stream is a
//! terminal, `NO_COLOR` is not set and `TERM` is not `dumb`. Piped output and
//! `--jsonl` stay plain, so scripts never have to strip escape sequences.
//!
//! Named colours use 24-bit RGB when the terminal advertises truecolour and
//! 256-colour indexes otherwise. The classic 16-colour codes are only the last
//! resort: themes commonly remap them all to the same foreground, which turns
//! the whole UI black and white even though the escapes are present.

use std::fmt::Display;
use std::io::IsTerminal;

/// How many colours the destination can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Colors {
    /// No escape sequences at all.
    None,
    /// The classic 16-colour palette.
    Ansi16,
    /// The xterm 256-colour palette.
    Ansi256,
    /// 24-bit RGB.
    True,
}

/// Which SGR codes to wrap text in.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    colors: Colors,
}

/// One Dark foreground colours, visible on both dark and light themes.
const GREEN: (u8, u8, u8) = (0x98, 0xC3, 0x79);
const RED: (u8, u8, u8) = (0xE0, 0x6C, 0x75);
const YELLOW: (u8, u8, u8) = (0xE5, 0xC0, 0x7B);
const CYAN: (u8, u8, u8) = (0x56, 0xB6, 0xC2);
const BLUE: (u8, u8, u8) = (0x61, 0xAF, 0xEF);
const MAGENTA: (u8, u8, u8) = (0xC6, 0x78, 0xDD);
const MUTED: (u8, u8, u8) = (0x9A, 0xA0, 0xA6);
const FAINT: (u8, u8, u8) = (0x7A, 0x78, 0x74);

impl Style {
    /// Styling that never emits escape sequences.
    #[must_use]
    pub fn plain() -> Self {
        Self {
            colors: Colors::None,
        }
    }

    /// Styling for `stderr`, where the agent's diagnostics go.
    #[must_use]
    pub fn stderr() -> Self {
        Self::for_terminal(std::io::stderr().is_terminal())
    }

    /// Styling for `stdout`, where reports go.
    #[must_use]
    pub fn stdout() -> Self {
        Self::for_terminal(std::io::stdout().is_terminal())
    }

    fn for_terminal(is_terminal: bool) -> Self {
        Self {
            colors: colors_for(is_terminal),
        }
    }

    /// Wraps `text` in the given SGR codes.
    #[must_use]
    pub fn paint(self, codes: &str, text: impl Display) -> String {
        if self.colors == Colors::None {
            text.to_string()
        } else {
            format!("\x1b[{codes}m{text}\x1b[0m")
        }
    }

    /// Paints with a 24-bit colour, falling back to a 256/16-colour code.
    fn color(self, rgb: (u8, u8, u8), ansi256: u8, ansi16: &str, text: impl Display) -> String {
        match self.colors {
            Colors::None => text.to_string(),
            Colors::True => {
                let (r, g, b) = rgb;
                format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
            }
            Colors::Ansi256 => format!("\x1b[38;5;{ansi256}m{text}\x1b[0m"),
            Colors::Ansi16 => format!("\x1b[{ansi16}m{text}\x1b[0m"),
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
        self.color(MUTED, 245, "2", text)
    }

    /// Dim italic, for the model's reasoning.
    #[must_use]
    pub fn thought(self, text: impl Display) -> String {
        match self.colors {
            Colors::None => text.to_string(),
            Colors::True => {
                let (r, g, b) = MUTED;
                format!("\x1b[38;2;{r};{g};{b};3m{text}\x1b[0m")
            }
            Colors::Ansi256 => format!("\x1b[38;5;245;3m{text}\x1b[0m"),
            Colors::Ansi16 => format!("\x1b[2;3m{text}\x1b[0m"),
        }
    }

    /// Green, for a successful action.
    #[must_use]
    pub fn green(self, text: impl Display) -> String {
        self.color(GREEN, 71, "32", text)
    }

    /// Red, for a failure.
    #[must_use]
    pub fn red(self, text: impl Display) -> String {
        self.color(RED, 203, "31", text)
    }

    /// Yellow, for something that needs attention.
    #[must_use]
    pub fn yellow(self, text: impl Display) -> String {
        self.color(YELLOW, 178, "33", text)
    }

    /// Cyan, for tools and prompts.
    #[must_use]
    pub fn cyan(self, text: impl Display) -> String {
        self.color(CYAN, 44, "36", text)
    }

    /// Blue, for usage and secondary labels.
    #[must_use]
    pub fn blue(self, text: impl Display) -> String {
        self.color(BLUE, 75, "34", text)
    }

    /// Magenta, for the model identity.
    #[must_use]
    pub fn magenta(self, text: impl Display) -> String {
        self.color(MAGENTA, 141, "35", text)
    }

    /// Muted, for rules and quiet chrome. Kept distinct from [`dim`] so a theme
    /// that renders bold/dim as plain still shows structure.
    ///
    /// [`dim`]: Style::dim
    #[must_use]
    pub fn faint(self, text: impl Display) -> String {
        self.color(FAINT, 244, "90", text)
    }

    /// Bold blue, for a tool call and its name.
    #[must_use]
    pub fn tool(self, text: impl Display) -> String {
        match self.colors {
            Colors::None => text.to_string(),
            Colors::True => {
                let (r, g, b) = BLUE;
                format!("\x1b[1;38;2;{r};{g};{b}m{text}\x1b[0m")
            }
            Colors::Ansi256 => format!("\x1b[1;38;5;75m{text}\x1b[0m"),
            Colors::Ansi16 => format!("\x1b[1;34m{text}\x1b[0m"),
        }
    }
}

fn colors_for(is_terminal: bool) -> Colors {
    if !is_terminal {
        return Colors::None;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return Colors::None;
    }
    if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return Colors::None;
    }
    if std::env::var("CLICOLOR").is_ok_and(|value| value == "0")
        && !std::env::var("CLICOLOR_FORCE").is_ok_and(|value| value == "1")
    {
        return Colors::None;
    }
    if std::env::var("COLORTERM")
        .is_ok_and(|value| value.eq_ignore_ascii_case("truecolor") || value == "24bit")
    {
        return Colors::True;
    }
    if std::env::var("TERM").is_ok_and(|term| term.contains("256color") || term.contains("direct"))
    {
        return Colors::Ansi256;
    }
    Colors::Ansi16
}

#[cfg(test)]
mod tests {
    use super::{Colors, Style};

    fn styled(colors: Colors) -> Style {
        Style { colors }
    }

    #[test]
    fn truecolor_uses_rgb_instead_of_the_remapped_palette() {
        let style = styled(Colors::True);
        assert_eq!(style.cyan("x"), "\x1b[38;2;86;182;194mx\x1b[0m");
        assert_eq!(style.tool("x"), "\x1b[1;38;2;97;175;239mx\x1b[0m");
    }

    #[test]
    fn palette_fallbacks_stay_distinct_from_the_classic_codes() {
        assert_eq!(styled(Colors::Ansi256).green("x"), "\x1b[38;5;71mx\x1b[0m");
        assert_eq!(styled(Colors::Ansi16).green("x"), "\x1b[32mx\x1b[0m");
        assert_eq!(styled(Colors::None).green("x"), "x");
    }
}
