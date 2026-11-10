use super::{Colors, Style};

fn styled(colors: Colors) -> Style {
    Style { colors }
}

#[test]
fn truecolor_uses_rgb_instead_of_the_remapped_palette() {
    let style = styled(Colors::True);
    assert_eq!(style.cyan("x"), "\x1b[38;2;255;92;214mx\x1b[0m");
    assert_eq!(style.tool("x"), "\x1b[1;38;2;255;92;214mx\x1b[0m");
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
        "\x1b[38;2;255;92;214msafe\\u{1b}[2Jowned\x1b[0m"
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
