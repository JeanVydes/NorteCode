//! Fenced code blocks: syntax colour and a box that closes.
//!
//! A block used to be drawn as `── lang ──` above the code and nothing below it,
//! in the terminal's own colour. Three things were wrong with that: a reader
//! could not tell where the block ended, the code was not coloured, and it was
//! the brightest thing on screen because it used the terminal's default.
//!
//! The colours come from [`syntect`]'s scope data, mapped onto one palette
//! defined here rather than a bundled `.tmTheme`. A theme file would be a second
//! colour source to keep in step with [`crate::style`]; matching scopes here
//! means code and chrome are painted from one set of constants and cannot drift.
//!
//! The box is drawn like the reasoning gutter — open, rows, close — because it
//! is the same kind of thing: a section held apart from the answer. It is grey
//! and quiet where the gutter is faint, so a reader can tell code from a thought
//! at a glance.

use std::collections::HashMap;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, FontStyle, StyleModifier, Theme, ThemeItem};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::style::Style;

/// One colour of the block palette.
///
/// The palette is defined here once: the theme syntect highlights with is built
/// from these colours, and a colour that comes back is mapped to its `Ink` to be
/// painted. That round trip is what lets a terminal without truecolour get the
/// same colour at lower depth — syntect reports RGB and nothing else, so the
/// mapping back is the only way to know which fallback belongs to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ink {
    /// Unclassified code: identifiers, punctuation, whitespace. A grey well
    /// below the terminal's default, so a block recedes instead of shouting.
    Plain,
    /// The box itself.
    Border,
    /// A comment, the quietest thing in the block.
    Comment,
    /// Keyword, storage type and control flow: `fn`, `let`, `def`, `return`.
    Keyword,
    /// String and character literal.
    Literal,
    /// Number and language constant.
    Number,
    /// Function and method name.
    Function,
    /// Type, class and tag name.
    Type,
    /// Operator, escape and built-in: the connective tissue.
    Operator,
    /// A tag name or an invalid escape, the one place red belongs in code.
    Alert,
}

impl Ink {
    /// Every ink, for the theme and for the reverse lookup.
    const ALL: [Ink; 10] = [
        Self::Plain,
        Self::Border,
        Self::Comment,
        Self::Keyword,
        Self::Literal,
        Self::Number,
        Self::Function,
        Self::Type,
        Self::Operator,
        Self::Alert,
    ];

    /// The 24-bit colour.
    const fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Plain => (0x82, 0x8A, 0x99),
            Self::Border => (0x4A, 0x4E, 0x55),
            Self::Comment => (0x5C, 0x63, 0x70),
            Self::Keyword => (0xC6, 0x78, 0xDD),
            Self::Literal => (0x98, 0xC3, 0x79),
            Self::Number => (0xD1, 0x9A, 0x66),
            Self::Function => (0x61, 0xAF, 0xEF),
            Self::Type => (0xE5, 0xC0, 0x7B),
            Self::Operator => (0x56, 0xB6, 0xC2),
            Self::Alert => (0xE0, 0x6C, 0x75),
        }
    }

    /// The xterm 256-colour index, chosen for the same hue and a step darker.
    const fn ansi256(self) -> u8 {
        match self {
            Self::Plain => 245,
            Self::Border => 240,
            Self::Comment => 242,
            Self::Keyword => 176,
            Self::Literal => 107,
            Self::Number => 173,
            Self::Function => 75,
            Self::Type => 180,
            Self::Operator => 44,
            Self::Alert => 203,
        }
    }

    /// The classic 16-colour code. A theme commonly remaps these, which is why
    /// they are only the fallback for a terminal that advertises nothing better.
    const fn ansi16(self) -> &'static str {
        match self {
            Self::Plain | Self::Border | Self::Comment => "90",
            Self::Keyword => "35",
            Self::Literal => "32",
            Self::Number | Self::Type => "33",
            Self::Function => "34",
            Self::Operator => "36",
            Self::Alert => "31",
        }
    }

    /// The ink a colour came from, or [`Ink::Plain`] for one that is not ours.
    ///
    /// The comparison is exact because the colour was produced by this palette,
    /// and the fallback is honest: an unknown colour is shown plainly rather
    /// than guessed at.
    fn of((r, g, b): (u8, u8, u8)) -> Ink {
        Ink::ALL
            .into_iter()
            .find(|ink| ink.rgb() == (r, g, b))
            .unwrap_or(Ink::Plain)
    }

    /// The selectors this ink answers to, and whether it is italic.
    ///
    /// A grammar names a thing by the scope that fits it, and one ink has to
    /// cover every scope that means it: Rust marks `fn` as `storage.type` while
    /// Python marks `def` as `keyword.control`, and both are the same colour to
    /// a reader. Each selector is scored against the scope path and applied
    /// from the least to the most specific, so the most specific match wins
    /// whatever order this list is in, and the empty selector — the theme's
    /// default for every scope — is overridden by any narrower one.
    const fn selectors(self) -> &'static [(&'static str, bool)] {
        match self {
            Self::Plain => &[("", false)],
            Self::Border => &[],
            Self::Comment => &[("comment", true), ("punctuation.definition.comment", true)],
            Self::Keyword => &[
                ("keyword.control", false),
                ("storage.type", false),
                ("keyword", false),
                ("storage", false),
            ],
            Self::Literal => &[("string", false), ("constant.character", false)],
            Self::Number => &[
                ("constant.numeric", false),
                ("constant.language", false),
                ("constant.other", false),
                ("constant", false),
                ("variable.parameter", false),
            ],
            Self::Function => &[
                ("entity.name.function", false),
                ("support.function", false),
                ("entity.name.function.macro", false),
            ],
            Self::Type => &[
                ("entity.name.type", false),
                ("entity.name.class", false),
                ("entity.name.struct", false),
                ("support.class", false),
                ("support.type", false),
                ("entity.name", false),
            ],
            Self::Operator => &[
                ("keyword.operator", false),
                ("constant.character.escape", false),
                ("support", false),
            ],
            Self::Alert => &[("invalid", false), ("entity.name.tag", false)],
        }
    }
}

/// Renders a fenced block, with a closed box and syntax colour when the language
/// is known.
///
/// The label is the fence's own token, kept as written: a reader who typed
/// ```` ```sh ```` should see `sh`, not the syntax's full name.
#[must_use]
pub fn block(style: Style, language: &str, code: &str) -> String {
    use std::fmt::Write as _;

    // The box has no width to reach: the renderer owns the terminal's columns
    // and this module is handed the code, not the screen. So the border is the
    // shape alone — an opening corner with the language, and a closing one — and
    // the code decides how wide the box looks.
    let mut out = String::new();
    if language.is_empty() {
        let _ = writeln!(out, "  {}", paint(style, Ink::Border, "╭─"));
    } else {
        let _ = writeln!(
            out,
            "  {} {}",
            paint(style, Ink::Border, "╭─"),
            paint(style, Ink::Border, language)
        );
    }

    if code.is_empty() {
        let _ = writeln!(
            out,
            "  {} {}",
            paint(style, Ink::Border, "│"),
            paint(style, Ink::Border, "(empty)")
        );
    } else {
        for line in highlight(style, language, code).lines() {
            // Even a blank line keeps the border: it is a row of the box, not a
            // gap in it, and a gap is what made the block's end unguessable.
            let _ = writeln!(out, "  {} {line}", paint(style, Ink::Border, "│"));
        }
    }
    let _ = writeln!(out, "  {}", paint(style, Ink::Border, "╰─"));
    out
}

/// The syntax name a fence's token resolves to, for tests and diagnostics.
///
/// A fence names a language, not an extension: ```` ```rust ```` has to reach
/// the `rs` syntax, so the token and its extension are both looked for. An empty
/// or unknown token is no language, and answers `None` rather than guessing from
/// a substring — a block coloured as the wrong language is worse than a grey one.
#[cfg(test)]
#[must_use]
pub fn syntax_name(token: &str) -> Option<&'static str> {
    syntax_for(token).map(|syntax| syntax.name.as_str())
}

/// Highlights `code` in `language`, one ANSI run per scope.
///
/// An unknown language returns the code uncoloured, which the box then draws in
/// grey. Every run closes with a reset: a line that stopped mid-style would
/// bleed its colour into the border drawn after it.
fn highlight(style: Style, language: &str, code: &str) -> String {
    // A shortcut, not the guard that keeps escapes out: with `Colors::None`
    // every `paint` below already returns its text untouched. This only skips
    // running the grammars when nothing will be painted.
    if !style.paints() {
        return code.to_owned();
    }
    let mut out = String::new();
    match syntax_for(language) {
        Some(syntax) => {
            let palette = highlighter();
            let mut state = HighlightLines::new(syntax, &palette.theme);
            for line in LinesWithEndings::from(code) {
                match state.highlight_line(line, &palette.syntaxes) {
                    Ok(ranges) => {
                        for (run, text) in ranges {
                            let color = run.foreground;
                            out.push_str(&paint(style, Ink::of((color.r, color.g, color.b)), text));
                        }
                    }
                    // A syntax that fails on its own input is a plain line, not
                    // a failure of the answer rendering around it.
                    Err(_) => out.push_str(&paint(style, Ink::Plain, line)),
                }
            }
        }
        // No language, or one nothing matches: the code is still the box's
        // code, so it is drawn in the box's grey. Left unpainted it would come
        // out in the terminal's own foreground, which is what made a block the
        // brightest thing on screen.
        None => {
            for line in LinesWithEndings::from(code) {
                out.push_str(&paint(style, Ink::Plain, line));
            }
        }
    }
    // No trailing newline is trimmed here: the caller reads this with `lines()`,
    // which already ignores a final empty line, and `block` writes the newline
    // that closes each row.
    out
}

/// Paints `text` in an ink, degrading to what the terminal can show.
fn paint(style: Style, ink: Ink, text: &str) -> String {
    let (r, g, b) = ink.rgb();
    style.paint_color((r, g, b), ink.ansi256(), ink.ansi16(), text)
}

/// The syntax set, theme and extension index, built once.
///
/// Loading the syntax set compiles 75 grammars and costs a few hundred
/// milliseconds, so it happens on the first fence that needs it and never again
/// — a run that prints no code pays nothing for it.
struct Palette {
    syntaxes: SyntaxSet,
    theme: Theme,
    by_extension: HashMap<String, usize>,
}

fn highlighter() -> &'static Palette {
    static PALETTE: OnceLock<Palette> = OnceLock::new();
    PALETTE.get_or_init(|| {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let by_extension = syntaxes
            .syntaxes()
            .iter()
            .enumerate()
            .flat_map(|(index, syntax)| {
                syntax
                    .file_extensions
                    .iter()
                    .map(move |extension| (extension.clone(), index))
            })
            .collect();
        Palette {
            syntaxes,
            theme: theme(),
            by_extension,
        }
    })
}

/// The theme syntect highlights with, built from [`Ink`].
///
/// Each selector is scored against a token's scope path and the matches are
/// applied from the least to the most specific, so a narrower selector wins
/// over a wider one whichever order they are listed in.
fn theme() -> Theme {
    let (r, g, b) = Ink::Plain.rgb();
    // No background: the terminal's own shows through, so a dark theme and a
    // light one both stay readable and the box does not become a filled slab.
    let mut theme = Theme {
        name: Some("norte-code".to_owned()),
        author: Some("Norte Code".to_owned()),
        ..Theme::default()
    };
    theme.settings.foreground = Some(Color { r, g, b, a: 0xFF });
    theme.scopes = Ink::ALL
        .into_iter()
        .flat_map(|ink| {
            ink.selectors().iter().map(move |(selector, italic)| {
                let (r, g, b) = ink.rgb();
                ThemeItem {
                    scope: selector.parse().unwrap_or_default(),
                    style: StyleModifier {
                        foreground: Some(Color { r, g, b, a: 0xFF }),
                        font_style: Some(if *italic {
                            FontStyle::ITALIC
                        } else {
                            FontStyle::empty()
                        }),
                        ..StyleModifier::default()
                    },
                }
            })
        })
        .collect();
    theme
}

/// The syntax for a fence's language token.
fn syntax_for(token: &str) -> Option<&'static SyntaxReference> {
    let palette = highlighter();
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let lower = token.to_ascii_lowercase();
    if let Some(&index) = palette.by_extension.get(&lower) {
        return palette.syntaxes.syntaxes().get(index);
    }
    // `sh`, `js`, `py` and `rust` are tokens rather than extensions, and
    // syntect's own resolution knows the aliases a plain index would miss.
    palette
        .syntaxes
        .find_syntax_by_token(&lower)
        .or_else(|| palette.syntaxes.find_syntax_by_name(token))
}

#[cfg(test)]
mod tests {
    use super::{Ink, block, syntax_name};
    use crate::style::{Colors, Style};

    /// A style with the palette forced, so a test reads colours without a tty.
    fn styled(colors: Colors) -> Style {
        Style::for_tests(colors)
    }

    #[test]
    fn a_known_language_is_highlighted_and_an_unknown_one_is_not() {
        let painted = styled(Colors::True);
        let (r, g, b) = Ink::Keyword.rgb();
        let keyword = format!("\u{1b}[38;2;{r};{g};{b}m");
        let rust = block(painted, "rust", "fn main() {}");
        assert!(rust.contains(&keyword), "the keyword is coloured: {rust:?}");
        // An unknown token is not guessed at: the code is drawn in the box's
        // grey rather than coloured as a language it is not.
        let unknown = block(painted, "not-a-language", "hello world");
        assert!(
            !unknown.contains(&keyword),
            "an unknown language is not coloured: {unknown:?}"
        );
        let (r, g, b) = Ink::Plain.rgb();
        assert!(
            unknown.contains(&format!("\u{1b}[38;2;{r};{g};{b}m")),
            "the code is still the box's grey: {unknown:?}"
        );
        assert!(unknown.contains("hello world"), "{unknown:?}");
    }

    #[test]
    fn the_fence_token_reaches_the_syntax_by_name_or_extension() {
        // `rust` and `rs` have to land on the same syntax, and so do `sh` and
        // `bash`: a fence names a language, the syntax set indexes extensions.
        assert_eq!(syntax_name("rust"), syntax_name("rs"));
        assert_eq!(syntax_name("sh"), syntax_name("bash"));
        assert!(syntax_name("rust").is_some_and(|name| name.contains("Rust")));
        assert_eq!(syntax_name(""), None);
        assert_eq!(syntax_name("nope-nope"), None);
    }

    #[test]
    fn the_box_opens_with_the_language_and_closes_under_the_code() {
        let out = block(styled(Colors::None), "rust", "let x = 1;");
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines[0].contains('╭') && lines[0].contains("rust"),
            "the box opens with its language: {out:?}"
        );
        assert!(lines[1].contains('│'), "the code is inside: {out:?}");
        assert!(lines[1].contains("let x = 1;"), "{out:?}");
        assert!(
            lines.last().is_some_and(|line| line.contains('╰')),
            "the box closes under the code: {out:?}"
        );
    }

    #[test]
    fn a_block_without_a_language_still_has_a_closed_box() {
        let out = block(styled(Colors::None), "", "plain text");
        assert!(
            out.lines().next().is_some_and(|l| l.contains('╭')),
            "{out:?}"
        );
        assert!(
            out.lines().last().is_some_and(|l| l.contains('╰')),
            "{out:?}"
        );
        assert!(out.contains("plain text"), "{out:?}");
    }

    #[test]
    fn an_empty_block_says_so_rather_than_drawing_nothing() {
        let out = block(styled(Colors::None), "rust", "");
        assert!(out.contains("(empty)"), "{out:?}");
        assert!(
            out.lines().last().is_some_and(|l| l.contains('╰')),
            "{out:?}"
        );
    }

    #[test]
    fn every_row_of_the_box_carries_a_border() {
        // The closing row is what tells a reader where the block ends; a blank
        // line inside it is a row of the box too, not a gap in it. Both depths
        // are checked: with colour the blank row carries escapes and is never
        // the empty string a careless filter would drop, and without colour it
        // is the empty string and dropping it loses the row.
        for colors in [Colors::True, Colors::None] {
            let out = block(styled(colors), "rust", "let a = 1;\n\nlet b = 2;");
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(
                lines.len(),
                5,
                "{colors:?}: open, three rows, close: {out:?}"
            );
            for (index, line) in lines.iter().enumerate() {
                let edge = index == 0 || index + 1 == lines.len();
                assert!(
                    edge || line.contains('│'),
                    "{colors:?}: row {index} lost its border: {line:?}"
                );
            }
        }
    }

    #[test]
    fn a_blank_row_inside_a_block_keeps_its_border_without_colour() {
        // Without colour the blank row is the empty string, so it has to be
        // written as a row of the box rather than skipped: a skipped row is a
        // gap in the border, which is what left the block's end unguessable.
        let out = block(styled(Colors::None), "rust", "let a = 1;\n\nlet b = 2;");
        let rows: Vec<&str> = out.lines().collect();
        assert_eq!(rows[2], "  │ ", "the blank row kept its border: {out:?}");
    }

    #[test]
    fn a_block_with_no_colour_requested_carries_no_escape_at_all() {
        // `--no-color` and a pipe both come through `Colors::None`, and the
        // highlighting must not be the one path that still emits escapes.
        let out = block(styled(Colors::None), "rust", "fn main() { let x = 1; }");
        assert!(!out.contains('\u{1b}'), "no escapes are emitted: {out:?}");
    }

    #[test]
    fn a_terminal_below_truecolour_gets_the_palette_and_not_rgb() {
        // syntect reports RGB and nothing else, so the ink a colour came from is
        // what decides the fallback; without that mapping the block would be
        // painted in sequences a 256-colour terminal cannot show.
        let out = block(styled(Colors::Ansi256), "rust", "fn main() {}");
        assert!(out.contains("\u{1b}[38;5;176m"), "a keyword: {out:?}");
        assert!(!out.contains("38;2;"), "no RGB is emitted: {out:?}");

        let classic = block(styled(Colors::Ansi16), "rust", "fn main() {}");
        assert!(classic.contains("\u{1b}[35m"), "a keyword: {classic:?}");
        assert!(!classic.contains("38;"), "no 256-colour code: {classic:?}");
    }

    #[test]
    fn the_plain_ink_is_darker_than_the_terminal_default() {
        // The block used to be the brightest thing on screen because it was not
        // styled at all. Its grey has to sit below a default foreground, which
        // is white or near it, or the box is a slab of light.
        let (r, g, b) = Ink::Plain.rgb();
        let average = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
        assert!(average < 0xC0, "the plain ink is {average:#x}, not muted");
        let (border_r, border_g, border_b) = Ink::Border.rgb();
        let border = (u16::from(border_r) + u16::from(border_g) + u16::from(border_b)) / 3;
        assert!(border < average, "the border recedes behind the code");
    }

    #[test]
    fn an_unknown_colour_falls_back_to_plain_rather_than_being_invented() {
        assert_eq!(Ink::of((0xFF, 0x00, 0xAB)), Ink::Plain);
        assert_eq!(Ink::of(Ink::Keyword.rgb()), Ink::Keyword);
    }

    #[test]
    fn a_grammar_that_names_a_keyword_by_storage_still_gets_the_keyword_ink() {
        // Rust marks `fn` as `storage.type`, not as `keyword`: an ink answers to
        // every scope that means it, and a list with only `keyword` in it left
        // `fn`, `let` and `mut` in the running text's grey.
        let out = block(styled(Colors::True), "rust", "let x = 1;");
        let (r, g, b) = Ink::Keyword.rgb();
        assert!(
            out.contains(&format!("\u{1b}[38;2;{r};{g};{b}mlet")),
            "`let` is a keyword: {out:?}"
        );
        // `pub`, `mut` and `fn` are the same ink by the same route.
        let declaration = block(styled(Colors::True), "rust", "pub fn f() {}");
        assert!(
            declaration.contains(&format!("\u{1b}[38;2;{r};{g};{b}mfn")),
            "`fn` is a keyword: {declaration:?}"
        );
    }

    #[test]
    fn a_narrower_selector_wins_over_a_wider_one_whichever_order_they_are_in() {
        // The theme is matched by specificity, not by position in the list, and
        // this is the check that says so: the empty selector is every scope's
        // default, and it must lose to the ones written after it. If the order
        // decided, `fn` would come out in the running text's grey.
        let out = block(styled(Colors::True), "rust", "pub fn f() {}");
        let (plain_r, plain_g, plain_b) = Ink::Plain.rgb();
        let (keyword_r, keyword_g, keyword_b) = Ink::Keyword.rgb();
        assert!(
            out.contains(&format!(
                "\u{1b}[38;2;{keyword_r};{keyword_g};{keyword_b}mfn"
            )),
            "`fn` took the keyword ink, not the default: {out:?}"
        );
        assert!(
            !out.contains(&format!("\u{1b}[38;2;{plain_r};{plain_g};{plain_b}mfn")),
            "`fn` did not take the default ink: {out:?}"
        );
    }

    #[test]
    fn the_code_box_is_not_the_reasoning_gutter() {
        // The two are the same kind of thing — a section held apart from the
        // answer — and they have to be told apart at a glance: the gutter is the
        // chrome's faint and the code box is its own, darker grey.
        let (r, g, b) = Ink::Border.rgb();
        let border = format!("\u{1b}[38;2;{r};{g};{b}m");
        let out = block(styled(Colors::True), "rust", "let x = 1;");
        assert!(out.contains(&border), "the box paints its border: {out:?}");
        // `faint` is what the gutter draws with; the two must not be the same
        // colour or the box reads as a thought.
        let faint = styled(Colors::True).faint("╰");
        assert!(
            !faint.contains(&border),
            "the box's border is the gutter's colour: {faint:?}"
        );
    }

    #[test]
    fn every_ink_has_a_fallback_for_each_depth() {
        for ink in Ink::ALL {
            assert!(!ink.ansi16().is_empty(), "{ink:?} has a 16-colour code");
            assert!(ink.ansi256() >= 16, "{ink:?} has a 256-colour index");
        }
    }

    #[test]
    fn the_block_ends_on_a_single_newline() {
        // The caller writes the next block right after this string, so a second
        // trailing newline would be a blank line of its own between blocks.
        let out = block(styled(Colors::None), "rust", "let x = 1;");
        assert!(out.ends_with('\n'), "{out:?}");
        assert!(!out.ends_with("\n\n"), "{out:?}");
    }
}
