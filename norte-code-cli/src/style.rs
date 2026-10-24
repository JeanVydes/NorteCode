//! ANSI styling for the terminal renderer.
//!
//! Styling is opt-in by destination: it is enabled only when the stream is a
//! terminal, `NO_COLOR` is not set and `TERM` is not `dumb`. Piped output and
//! `--jsonl` stay plain, so scripts never have to strip escape sequences.
//! [`Colors`] holds that decision.
//!
//! Named colours use 24-bit RGB when the terminal advertises truecolour and
//! 256-colour indexes otherwise. The classic 16-colour codes are only the last
//! resort: themes commonly remap them all to the same foreground, which turns
//! the whole UI black and white even though the escapes are present.

use std::fmt::Display;
use std::io::IsTerminal;

mod colors;

pub(crate) use colors::Colors;

/// How a styled run ends its SGR sequence.
///
/// The `ESC` is split out for the same reason as [`ansi`]: the literal must not
/// carry an escape byte that [`Style::decorate`] would then escape away.
const RESET: &str = "\u{1b}[0m";

/// Opens an SGR run with the given codes.
#[must_use]
fn ansi(codes: &str) -> String {
    format!("{}[{codes}m", '\u{1b}')
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
            colors: colors::Environment::current(is_terminal).colors(),
        }
    }

    /// Wraps `text` in the given SGR codes.
    #[must_use]
    pub fn paint(self, codes: &str, text: impl Display) -> String {
        self.decorate(codes, text)
    }

    /// Whether this style emits escape sequences at all.
    #[must_use]
    pub fn paints(self) -> bool {
        !matches!(self.colors, Colors::None)
    }

    /// Wraps text that is already escaped in the given SGR codes.
    ///
    /// [`paint`] escapes its input, so a caller that composes two styles would
    /// get the inner codes escaped and printed as `\u{{1b}}[1m` instead of
    /// styled. This is the raw form for that case: the caller guarantees the
    /// text carries no terminal control of its own.
    ///
    /// [`paint`]: Style::paint
    pub(crate) fn paint_raw(self, codes: &str, text: &str) -> String {
        match self.colors {
            Colors::None => text.to_owned(),
            _ => format!("{}{text}{RESET}", ansi(codes)),
        }
    }

    /// Escapes the text and wraps it in the given SGR codes.
    ///
    /// Every styled string passes through here, so the chrome the renderer
    /// writes is escaped exactly like the untrusted model, tool and catalogue
    /// text: one path, no exceptions to get wrong. The escape sequence this
    /// function adds is built from a `char`, so its own `ESC` is never part of
    /// a literal that the escaping could reach.
    fn decorate(self, codes: &str, text: impl Display) -> String {
        let text = text.to_string();
        let text = crate::text::terminal(&text);
        match self.colors {
            Colors::None => text.into_owned(),
            _ => format!("{}{text}{RESET}", ansi(codes)),
        }
    }

    /// Paints with a 24-bit colour, falling back to a 256/16-colour code.
    fn color(self, rgb: (u8, u8, u8), ansi256: u8, ansi16: &str, text: impl Display) -> String {
        let codes = match self.colors {
            Colors::None => return self.decorate("", text),
            Colors::True => {
                let (r, g, b) = rgb;
                format!("38;2;{r};{g};{b}")
            }
            Colors::Ansi256 => format!("38;5;{ansi256}"),
            Colors::Ansi16 => ansi16.to_owned(),
        };
        self.decorate(&codes, text)
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
        let codes = match self.colors {
            Colors::None => return self.decorate("", text),
            Colors::True => {
                let (r, g, b) = MUTED;
                format!("38;2;{r};{g};{b};3")
            }
            Colors::Ansi256 => "38;5;245;3".to_owned(),
            Colors::Ansi16 => "2;3".to_owned(),
        };
        self.decorate(&codes, text)
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
        let codes = match self.colors {
            Colors::None => return self.decorate("", text),
            Colors::True => {
                let (r, g, b) = BLUE;
                format!("1;38;2;{r};{g};{b}")
            }
            Colors::Ansi256 => "1;38;5;75".to_owned(),
            Colors::Ansi16 => "1;34".to_owned(),
        };
        self.decorate(&codes, text)
    }
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

    #[test]
    fn styled_text_cannot_inject_terminal_controls() {
        let injected = "safe\x1b[2Jowned";
        assert_eq!(
            styled(Colors::True).cyan(injected),
            "\x1b[38;2;86;182;194msafe\\u{1b}[2Jowned\x1b[0m"
        );
        assert_eq!(styled(Colors::None).dim(injected), "safe\\u{1b}[2Jowned");
    }

    #[test]
    fn the_chrome_is_escaped_like_model_text() {
        // "ú", "⏳" and "·" are not controls and must survive untouched.
        assert_eq!(
            styled(Colors::None).dim("archivó ⏳ · 1k"),
            "archivó ⏳ · 1k"
        );
        // A bidi override in a label is escaped like one in an answer.
        assert_eq!(
            styled(Colors::None).dim("a\u{202e}b"),
            "a\\u{202e}b",
            "a styled label is escaped too"
        );
    }

    #[test]
    fn plain_style_reports_that_it_paints_nothing() {
        assert!(!Style::plain().paints());
        assert!(styled(Colors::True).paints());
    }
}
