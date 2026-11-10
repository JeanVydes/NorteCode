use super::*;

/// One rendered line as `text`, for the assertions about content.
fn texts(text: &str) -> Vec<String> {
    render(text, 80).iter().map(Rendered::text).collect()
}

#[test]
fn a_heading_loses_its_hashes_and_a_hash_does_not() {
    assert_eq!(texts("## A title"), ["A title"]);
    // A hashtag is a word: the space after the hashes is the rule.
    assert_eq!(texts("#hashtag"), ["#hashtag"]);
    // Seven hashes are not a heading, and `#` alone is not one either.
    assert_eq!(texts("####### too deep"), ["####### too deep"]);
    assert_eq!(texts("#"), ["#"]);
    // A closing run of hashes is syntax.
    assert_eq!(texts("## A title ##"), ["A title"]);
}

#[test]
fn a_list_is_drawn_with_a_bullet_and_keeps_its_number() {
    assert_eq!(texts("- one\n- two"), ["• one", "• two"]);
    assert_eq!(texts("* one\n+ two"), ["• one", "• two"]);
    // An ordered list keeps the number: which step a line is has no other
    // carrier.
    assert_eq!(texts("1. first\n2. second"), ["1. first", "2. second"]);
    assert_eq!(texts("1) first"), ["1. first"]);
    // A dash that is not a list is a dash.
    assert_eq!(texts("-nope"), ["-nope"]);
}

#[test]
fn a_fence_is_a_block_and_its_language_leads_it() {
    let lines = texts("```rust\nlet x = 1;\n```\nafter");
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines[0].contains("rust"), "the language leads: {lines:?}");
    assert_eq!(lines[1], "let x = 1;", "the code, without its fence");
    assert!(lines[2].contains("rust"), "and it closes: {lines:?}");
    assert_eq!(lines[3], "after");
}

#[test]
fn nothing_inside_a_fence_is_markdown() {
    // A `**` in a code block is two asterisks, and a `#` is a hash: a
    // renderer that read them as markdown would draw the code as prose.
    let lines = texts("```\n**not bold** and # not a title\n```");
    assert_eq!(lines[1], "**not bold** and # not a title");
}

#[test]
fn an_unclosed_fence_does_not_swallow_the_rest_of_the_document() {
    // The lines after it are code, which is what the fence said. What
    // matters is that they are drawn and not dropped.
    let lines = texts("```\ncode\nmore code");
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[1], "code");
    assert_eq!(lines[2], "more code");
}

#[test]
fn inline_emphasis_is_runs_and_not_asterisks() {
    let runs = inline("a **bold** b *italic* c `code` d");
    let bold = runs
        .iter()
        .find(|run| run.emphasis == Emphasis::Bold)
        .expect("bold");
    assert_eq!(bold.text, "bold");
    let italic = runs
        .iter()
        .find(|run| run.emphasis == Emphasis::Italic)
        .expect("italic");
    assert_eq!(italic.text, "italic");
    let code = runs
        .iter()
        .find(|run| run.emphasis == Emphasis::Code)
        .expect("code");
    assert_eq!(code.text, "code");
    assert!(
        !runs.iter().any(|run| run.text.contains('*')),
        "no marker survives: {runs:?}"
    );
}

#[test]
fn an_unclosed_marker_is_printed_as_it_arrived() {
    // A construct the scan cannot pair is text: swallowing it to the end of
    // the line would hide the asterisks and change what the line says.
    assert_eq!(inline("a * b"), vec![Run::plain("a * b")]);
    assert_eq!(inline("`unclosed"), vec![Run::plain("`unclosed")]);
    assert_eq!(inline("**unclosed"), vec![Run::plain("**unclosed")]);
    // Two markers on one line *are* a pair, which is what a multiplication
    // written in prose looks like and what every renderer does with it:
    // the emphasis is read, and the text is unchanged either way.
    let runs = inline("2 * 3 * 4");
    assert!(runs.iter().any(|run| run.emphasis == Emphasis::Italic));
    assert_eq!(
        runs.iter().map(|r| r.text.as_str()).collect::<String>(),
        "2  3  4",
        "the markers are gone and the spaces stay"
    );
}

#[test]
fn an_underscore_inside_a_word_is_a_word() {
    // `snake_case` is not italic: the convention is what every renderer
    // follows, and a name drawn in italics reads as an error.
    assert_eq!(
        inline("the snake_case name"),
        vec![Run::plain("the snake_case name")]
    );
    assert_eq!(
        inline("_italic_ at the start").first().map(|r| r.emphasis),
        Some(Emphasis::Italic)
    );
}

#[test]
fn a_link_shows_what_it_reads_as_and_where_it_goes() {
    let runs = inline("see [the guide](https://example.com/x)");
    assert!(runs.iter().any(|run| run.text == "the guide"));
    assert!(
        runs.iter()
            .any(|run| run.emphasis == Emphasis::Link && run.text.contains("example.com")),
        "{runs:?}"
    );
    // A bracket that is not a link is a bracket.
    assert_eq!(inline("[not a link]"), vec![Run::plain("[not a link]")]);
}

#[test]
fn a_quote_keeps_its_marker_and_a_rule_is_drawn() {
    let lines = texts("> quoted\nafter");
    assert!(lines[0].starts_with('│'), "{}", lines[0]);
    assert!(lines[0].contains("quoted"));
    assert_eq!(lines[1], "after");
    // A rule, however it was typed, is a line.
    for rule in ["---", "***", "___", "- - -"] {
        let lines = texts(rule);
        assert!(lines[0].contains('─'), "{rule} -> {lines:?}");
    }
}

#[test]
fn a_struck_run_loses_its_tildes_and_a_lone_one_stays() {
    let runs = inline("a ~~gone~~ b");
    let struck = runs
        .iter()
        .find(|run| run.emphasis == Emphasis::Strike)
        .expect("struck");
    assert_eq!(struck.text, "gone");
    assert!(
        !runs.iter().any(|run| run.text.contains('~')),
        "no tilde survives: {runs:?}"
    );
    // A single tilde is a tilde: `~5` is not an unclosed strike.
    assert_eq!(inline("about ~5 items"), vec![Run::plain("about ~5 items")]);
}

#[test]
fn an_underline_run_loses_its_tag() {
    let runs = inline("a <u>under</u> b");
    let underlined = runs
        .iter()
        .find(|run| run.emphasis == Emphasis::Underline)
        .expect("underlined");
    assert_eq!(underlined.text, "under");
    assert!(
        !runs.iter().any(|run| run.text.contains('<')),
        "the tag is gone: {runs:?}"
    );
    // A tag this does not know is left as the text it is.
    assert_eq!(inline("a <b>bold</b>"), vec![Run::plain("a <b>bold</b>")]);
}

#[test]
fn a_table_is_drawn_as_aligned_columns() {
    let lines = texts("| a | b |\n| --- | --- |\n| 1 | 22 |");
    // The header, the rule, and the body: three rows and not the pipes.
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        lines[0].contains('a') && lines[0].contains('b'),
        "{lines:?}"
    );
    assert!(
        lines[1].contains('─') && lines[1].contains('┼'),
        "{lines:?}"
    );
    assert!(
        lines[2].contains('1') && lines[2].contains("22"),
        "{lines:?}"
    );
    assert!(
        !lines[0].contains('|') || !lines[0].starts_with('|'),
        "the outer pipe is gone: {lines:?}"
    );
}

#[test]
fn a_table_column_is_as_wide_as_its_widest_cell() {
    // The body can be wider than the header, and the column grows to hold
    // it: a header-only measurement would wrap the longest cell.
    let rendered = render("| h |\n| --- |\n| a longer cell |", 80);
    let body = rendered.last().expect("body").text();
    assert!(body.contains("a longer cell"), "{body:?}");
    // The header is padded to the same column, so the rule under it lines up.
    let header = rendered.first().expect("header").text();
    assert_eq!(
        header.chars().count(),
        body.chars().count(),
        "header and body share a width"
    );
}

#[test]
fn a_line_of_prose_with_a_pipe_is_not_a_table() {
    // A table needs the separator under its header; a pipe in prose is a
    // pipe, which is what a shell command full of `|` relies on.
    assert_eq!(texts("cat a | grep b"), ["cat a | grep b"]);
}

#[test]
fn an_unknown_language_is_not_guessed_at() {
    // A wrong colour is worse than none: a language this does not know is
    // drawn plainly rather than half-highlighted.
    let runs = highlight("brainfuck", "let x = \"a\"; // c");
    assert_eq!(runs.len(), 1, "one plain run: {runs:?}");
    assert_eq!(runs[0].emphasis, Emphasis::Fence);
    // And a language it knows is not.
    let runs = highlight("rust", "let x = 1;");
    assert!(runs.len() > 1, "rust highlights: {runs:?}");
}

#[test]
fn highlighting_finds_strings_comments_and_keywords() {
    let runs = highlight("rust", "let x = \"a\"; // note");
    let text: String = runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(text, "let x = \"a\"; // note", "nothing is lost");
    assert!(
        runs.iter()
            .any(|r| r.emphasis == Emphasis::Code && r.text == "\"a\""),
        "the string is its own run: {runs:?}"
    );
    assert!(
        runs.iter().any(|r| r.emphasis == Emphasis::Quote),
        "the comment is its own run: {runs:?}"
    );
    assert!(
        runs.iter().any(|r| r.text == "let"),
        "the keyword is kept whole: {runs:?}"
    );
}

#[test]
fn a_string_cannot_swallow_the_line_it_does_not_close() {
    // An unclosed quote colours the rest of its own line and nothing more:
    // a wrong colour in one row, not in the rest of the block.
    let runs = highlight("rust", "let s = \"never closed");
    let text: String = runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(text, "let s = \"never closed");
}
