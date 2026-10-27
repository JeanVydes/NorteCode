//! Markdown and code highlighting, as runs a frame can draw.
//!
//! A model answers in markdown, and the client printed it plainly: `**bold**`
//! arrived with its asterisks, a fence arrived as three backticks, and a list
//! arrived as a column of dashes. The rendering was never the conversation's
//! business — what arrived is text, and how it is drawn is the view's — so this
//! module is called by the view and not by the renderer.
//!
//! # Why runs and not escapes
//!
//! The same reason the transcript's rows carry ink: a frame draws *spans*, and a
//! string that carries its own SGR codes is a string whose codes are shown as
//! the characters they are. A [`Run`] is text and the emphasis it is written
//! in, and the view is what turns that into a colour.
//!
//! # What it does not do
//!
//! It is a terminal's markdown, not a browser's. There are no tables, no
//! footnotes, no HTML and no nested emphasis inside a code span, because a
//! terminal has no room for the first three and the fourth is where every
//! hand-written markdown renderer starts to be wrong. A construct it does not
//! know is printed as it arrived, which is what a reader can still read.

use std::collections::BTreeSet;

/// The emphasis one run of text is written in.
///
/// It is an emphasis and not a colour: a heading and a bold word are the same
/// ink and different shapes, and the view decides what a shape looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Emphasis {
    /// Ordinary text.
    Plain,
    /// `**bold**` and `__bold__`.
    Bold,
    /// `*italic*` and `_italic_`.
    Italic,
    /// A `` `code span` ``.
    Code,
    /// A heading, without its hashes.
    Heading,
    /// The bullet that leads a list item.
    Bullet,
    /// A quoted line, without its `>`.
    Quote,
    /// Something inside a fenced block.
    Fence,
    /// A link's target, which is shown but is not what the link reads as.
    Link,
}

/// One run of text in one emphasis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    /// What it says.
    pub text: String,
    /// How it is written.
    pub emphasis: Emphasis,
}

impl Run {
    /// A run of plain text.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            emphasis: Emphasis::Plain,
        }
    }

    /// A run in an emphasis.
    #[must_use]
    pub fn styled(emphasis: Emphasis, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            emphasis,
        }
    }
}

/// One line of rendered markdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rendered {
    /// The runs the line is made of, in order.
    pub runs: Vec<Run>,
    /// How far the line is indented, in columns.
    ///
    /// It is separate from the runs because a wrapped continuation has to line
    /// up under the text it belongs to: a list item whose second row starts at
    /// the margin reads as a second item.
    pub indent: usize,
}

impl Rendered {
    /// The line's text with no emphasis.
    ///
    /// It is what a test asserts on: the runs are what the frame draws, and
    /// what they add up to is what the line says. The view reads the runs and
    /// not this.
    #[cfg(test)]
    #[must_use]
    pub fn text(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }
}

/// The most columns of indentation a list may build up.
///
/// A document with fifty levels of nesting is a document that would be read
/// off the right edge of the terminal: past this the indent stops growing, so
/// the text stays on screen and the nesting is wrong rather than invisible.
const MAX_INDENT: usize = 8;

/// Renders markdown as the lines a frame draws.
///
/// The whole text is taken at once and not a line at a time: a fence is state
/// that spans lines, and a renderer that saw one line would draw the code as
/// prose and the prose as code.
#[must_use]
pub(crate) fn render(text: &str) -> Vec<Rendered> {
    let mut out = Vec::new();
    // The fence that is open, if any: its backtick run and its language.
    let mut fence: Option<(String, String)> = None;

    for raw in text.split('\n') {
        // A quote is marked per line, so its depth is read before anything else
        // and the rest of the line is read as what it quotes.
        let mut line = raw;
        let mut depth = 0;
        while let Some(rest) = line.strip_prefix('>') {
            depth += 1;
            line = rest.strip_prefix(' ').unwrap_or(rest);
        }
        if let Some((marker, language)) = &fence {
            // Inside a fence nothing is markdown: a `**` in a code block is two
            // asterisks, and a `#` is a hash.
            if is_fence(line.trim(), marker) {
                out.push(Rendered {
                    runs: vec![Run::styled(Emphasis::Fence, fence_label(language))],
                    indent: depth,
                });
                fence = None;
            } else {
                out.push(Rendered {
                    runs: highlight(language, line),
                    indent: depth,
                });
            }
            continue;
        }
        if let Some((marker, language)) = open_fence(line) {
            out.push(Rendered {
                runs: vec![Run::styled(Emphasis::Fence, fence_label(&language))],
                indent: depth,
            });
            fence = Some((marker, language));
            continue;
        }
        out.push(block(line, depth));
    }
    out
}

/// The head of a code block, as the row that names it.
///
/// The three backticks are a delimiter and not something to read: what a person
/// wants from that row is which language the block is, so it is the language
/// that is drawn and the fence is not.
#[must_use]
fn fence_label(language: &str) -> String {
    if language.is_empty() {
        "┄ code ┄".to_owned()
    } else {
        format!("┄ {language} ┄")
    }
}

/// Whether a line closes the fence that is open.
///
/// The shape has to match: a ```` ``` ```` block is not closed by `~~~`, which
/// is what makes a fence inside a fence possible to write at all.
#[must_use]
fn is_fence(line: &str, marker: &str) -> bool {
    line.trim()
        .chars()
        .all(|c| c == marker.chars().next().unwrap_or('`'))
        && line.trim().len() >= marker.len()
}

/// The fence a line opens, as its marker and its language.
#[must_use]
fn open_fence(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    let marker = if trimmed.starts_with("```") {
        "```"
    } else if trimmed.starts_with("~~~") {
        "~~~"
    } else {
        return None;
    };
    let rest = trimmed.get(marker.len()..).unwrap_or("").trim();
    // A fence with no language is a fence: `render` draws a plain code box.
    Some((marker.to_owned(), rest.to_owned()))
}

/// One line that is not inside a fence.
#[must_use]
fn block(line: &str, depth: usize) -> Rendered {
    if depth > 0 {
        // The quote's own line: the marker is drawn and the rest is markdown
        // again, because a quote may hold a list or a bold word.
        let mut runs = vec![Run::styled(Emphasis::Quote, "│ ")];
        runs.extend(inline(line));
        return Rendered {
            runs,
            indent: depth,
        };
    }
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();

    // A rule: three or more of the same marker, and nothing else on the line.
    if is_rule(trimmed) {
        return Rendered {
            runs: vec![Run::styled(Emphasis::Quote, "─".repeat(40))],
            indent,
        };
    }
    // A heading, without its hashes.
    if let Some(rest) = heading(trimmed) {
        let mut runs = vec![Run::styled(Emphasis::Heading, rest)];
        // A closing run of hashes is part of the syntax and not of the title.
        if let Some(last) = runs.last_mut() {
            let text = last.text.trim_end_matches('#').trim_end().to_owned();
            last.text = text;
        }
        return Rendered { runs, indent };
    }
    // A bullet, drawn with a bullet and not with the character it was typed as.
    if let Some(rest) = bullet(trimmed) {
        let mut runs = vec![Run::styled(Emphasis::Bullet, "• ")];
        runs.extend(inline(rest));
        return Rendered {
            runs,
            indent: indent + 2,
        };
    }
    // An ordered item keeps its number: the number is information, and which
    // step of a procedure a line is has no other carrier.
    if let Some((number, rest)) = ordered(trimmed) {
        let mut runs = vec![Run::styled(Emphasis::Bullet, format!("{number} "))];
        runs.extend(inline(rest));
        let width = number.chars().count() + 1;
        return Rendered {
            runs,
            indent: indent + width,
        };
    }

    Rendered {
        runs: inline(line),
        indent: 0,
    }
}

/// Whether a line is a horizontal rule.
#[must_use]
fn is_rule(trimmed: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut count = 0;
    for character in trimmed.trim_end().chars() {
        if character == ' ' {
            continue;
        }
        if !matches!(character, '-' | '*' | '_') {
            return false;
        }
        seen.insert(character);
        count += 1;
    }
    count >= 3 && seen.len() == 1
}

/// The text of a heading, without its hashes.
#[must_use]
fn heading(trimmed: &str) -> Option<String> {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed.get(hashes..)?;
    // `#hashtag` is a word, `# a title` is a heading: the space is the rule.
    let rest = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('\t'))?;
    Some(rest.trim().to_owned())
}

/// The text of a bullet, without its marker.
#[must_use]
fn bullet(trimmed: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest);
        }
    }
    None
}

/// An ordered item's number and its text.
#[must_use]
fn ordered(trimmed: &str) -> Option<(String, &str)> {
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = trimmed.get(digits..)?;
    for marker in [". ", ") "] {
        if let Some(text) = rest.strip_prefix(marker) {
            return Some((format!("{}.", &trimmed[..digits]), text));
        }
    }
    None
}

/// One line's inline markdown, as runs.
///
/// The scan is left to right and it never looks back: `**bold**` is opened and
/// closed in that order, and an unclosed `**` is two asterisks, which is what it
/// is. A construct the scan cannot pair is printed as it arrived rather than
/// swallowed to the end of the line.
#[must_use]
pub(crate) fn inline(text: &str) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let mut plain = String::new();

    // A run of plain text is flushed whole, so a line with no markdown in it is
    // one run and not one per character.
    macro_rules! flush {
        () => {
            if !plain.is_empty() {
                runs.push(Run::plain(std::mem::take(&mut plain)));
            }
        };
    }

    while at < chars.len() {
        let character = chars[at];
        // A code span first: inside it, markdown is not markdown.
        if character == '`' {
            let ticks = chars[at..].iter().take_while(|c| **c == '`').count();
            if let Some(end) = find_run(&chars, at + ticks, '`', ticks) {
                flush!();
                let inner: String = chars[at + ticks..end].iter().collect();
                let inner = inner.trim();
                runs.push(Run::styled(Emphasis::Code, inner));
                at = end + ticks;
                continue;
            }
        }
        if character == '*' || character == '_' {
            let doubled = chars.get(at + 1) == Some(&character);
            let marker = if doubled { 2 } else { 1 };
            let emphasis = if doubled {
                Emphasis::Bold
            } else {
                Emphasis::Italic
            };
            // `_` inside a word is a word: `snake_case` is not italic, and the
            // convention that says so is what every renderer follows.
            let inside_word = character == '_'
                && at > 0
                && chars.get(at - 1).is_some_and(|c| c.is_alphanumeric())
                && chars.get(at + marker).is_some_and(|c| c.is_alphanumeric());
            if !inside_word {
                if let Some(end) = find_emphasis(&chars, at + marker, character) {
                    flush!();
                    let inner: String = chars[at + marker..end].iter().collect();
                    runs.push(Run::styled(emphasis, inner));
                    at = end + marker;
                    continue;
                }
            }
        }
        // A link: the text is what it reads as, and the target is what it
        // points at. Both are worth showing in a terminal, where a target
        // cannot be clicked and a reader may want to copy it.
        if character == '[' {
            if let Some((text_end, target_end, target)) = link(&chars, at) {
                flush!();
                let label: String = chars[at + 1..text_end].iter().collect();
                runs.extend(inline(&label));
                runs.push(Run::styled(Emphasis::Link, format!(" ({target})")));
                at = target_end;
                continue;
            }
        }
        plain.push(character);
        at += 1;
    }
    flush!();
    if runs.is_empty() {
        runs.push(Run::plain(String::new()));
    }
    runs
}

/// The index of the closing run of a marker, if the line has one.
#[must_use]
fn find_run(chars: &[char], from: usize, marker: char, count: usize) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        if chars[at] == marker {
            let run = chars[at..].iter().take_while(|c| **c == marker).count();
            if run >= count {
                return Some(at);
            }
            at += run;
            continue;
        }
        at += 1;
    }
    None
}

/// The index of the marker that closes an emphasis opened at `from`.
///
/// A single `*` is closed by a single one: `*a*` is italic and so is `**a**`
/// bold, and letting a single close a double is what turned every bullet list
/// into one italic paragraph.
#[must_use]
fn find_emphasis(chars: &[char], from: usize, marker: char) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        if chars[at] == marker {
            let run = chars[at..].iter().take_while(|c| **c == marker).count();
            // Bold may be closed by one or two: `**a*` is what a person types
            // when they are half-way through, and reading it as bold is kinder
            // than reading it as text.
            if (1..=2).contains(&run) {
                return Some(at);
            }
            at += run;
            continue;
        }
        at += 1;
    }
    None
}

/// A link's label end, its close, and its target.
#[must_use]
fn link(chars: &[char], at: usize) -> Option<(usize, usize, String)> {
    let text_end = chars[at + 1..].iter().position(|c| *c == ']')? + at + 1;
    if chars.get(text_end + 1) != Some(&'(') {
        return None;
    }
    let target_end = chars[text_end + 2..].iter().position(|c| *c == ')')? + text_end + 2;
    let target: String = chars[text_end + 2..target_end].iter().collect();
    Some((text_end, target_end + 1, target))
}

/// Highlights one line of a fenced block, as runs.
///
/// A terminal has no room for a real parser and no need for one: what a reader
/// wants from a highlighted line is where the strings are, where the comment
/// starts and which words the language reserves. The scan is left to right and
/// never looks past the line, so a broken string colours the rest of its line
/// and nothing more — which is a wrong colour in one row instead of a wrong
/// colour for the rest of the block.
///
/// A language this does not know is drawn plainly. Guessing at a language is
/// how a shell script comes out with half its words in a keyword colour.
#[must_use]
pub(crate) fn highlight(language: &str, line: &str) -> Vec<Run> {
    let keywords = keywords(language);
    if keywords.is_empty() {
        return vec![Run::styled(Emphasis::Fence, line)];
    }
    let comment = comment_prefix(language);
    let mut runs: Vec<Run> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0;
    let mut plain = String::new();
    while at < chars.len() {
        // A comment takes the rest of the line, whatever is in it.
        if !comment.is_empty() && starts_with(&chars, at, comment) {
            if !plain.is_empty() {
                runs.push(Run::styled(Emphasis::Fence, std::mem::take(&mut plain)));
            }
            let rest: String = chars[at..].iter().collect();
            runs.push(Run::styled(Emphasis::Quote, rest));
            return runs;
        }
        let character = chars[at];
        // A string runs to its own quote, and a backslash escapes one.
        if character == '"' || character == '\'' || character == '`' {
            if let Some(end) = string_end(&chars, at, character) {
                if !plain.is_empty() {
                    runs.push(Run::styled(Emphasis::Fence, std::mem::take(&mut plain)));
                }
                let text: String = chars[at..=end].iter().collect();
                runs.push(Run::styled(Emphasis::Code, text));
                at = end + 1;
                continue;
            }
        }
        // A word, so it can be matched against the language's keywords.
        if character.is_alphabetic() || character == '_' {
            let end = chars[at..]
                .iter()
                .position(|c| !c.is_alphanumeric() && *c != '_')
                .map_or(chars.len(), |offset| at + offset);
            let word: String = chars[at..end].iter().collect();
            if keywords.contains(word.as_str()) {
                if !plain.is_empty() {
                    runs.push(Run::styled(Emphasis::Fence, std::mem::take(&mut plain)));
                }
                runs.push(Run::styled(Emphasis::Heading, word));
            } else {
                plain.push_str(&word);
            }
            at = end;
            continue;
        }
        // A number.
        if character.is_ascii_digit() {
            let end = chars[at..]
                .iter()
                .position(|c| !c.is_ascii_alphanumeric() && *c != '.' && *c != '_')
                .map_or(chars.len(), |offset| at + offset);
            if !plain.is_empty() {
                runs.push(Run::styled(Emphasis::Fence, std::mem::take(&mut plain)));
            }
            let number: String = chars[at..end].iter().collect();
            runs.push(Run::styled(Emphasis::Bold, number));
            at = end;
            continue;
        }
        plain.push(character);
        at += 1;
    }
    if !plain.is_empty() {
        runs.push(Run::styled(Emphasis::Fence, plain));
    }
    if runs.is_empty() {
        runs.push(Run::styled(Emphasis::Fence, String::new()));
    }
    runs
}

/// Whether a marker starts at this position.
#[must_use]
fn starts_with(chars: &[char], at: usize, marker: &str) -> bool {
    marker
        .chars()
        .zip(chars.iter().skip(at))
        .all(|(want, have)| want == *have)
        && chars.len() >= at + marker.chars().count()
}

/// The index of the quote that closes a string, honouring `\`.
#[must_use]
fn string_end(chars: &[char], from: usize, quote: char) -> Option<usize> {
    let mut at = from + 1;
    while at < chars.len() {
        match chars[at] {
            '\\' => at += 2,
            character if character == quote => return Some(at),
            _ => at += 1,
        }
    }
    None
}

/// The comment marker of a language, when it has one.
#[must_use]
fn comment_prefix(language: &str) -> &'static str {
    match language {
        // Python and shell comment with a hash; toml, yaml and json have no
        // comment in the shape this highlights.
        "python" | "py" | "bash" | "sh" | "shell" | "zsh" | "yaml" | "yml" | "toml" => "#",
        _ => "//",
    }
}

/// The words a language reserves, as the names it is written under.
///
/// A language that is not here is highlighted as plain text on purpose: a
/// wrong colour is worse than none, and a list nobody maintains is a list that
/// colours last year's syntax.
#[must_use]
// It is one arm per language and one word per line: the length is the table,
// and splitting it into functions would put the reader three jumps from the
// list they came to read.
#[allow(clippy::too_many_lines)]
fn keywords(language: &str) -> BTreeSet<&'static str> {
    let list: &[&str] = match language {
        "rust" | "rs" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        "python" | "py" => &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
            "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return",
            "True", "try", "while", "with", "yield",
        ],
        "javascript" | "js" | "typescript" | "ts" | "tsx" | "jsx" => &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "from",
            "function",
            "if",
            "import",
            "in",
            "instanceof",
            "let",
            "new",
            "null",
            "of",
            "return",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "typeof",
            "undefined",
            "var",
            "void",
            "while",
            "yield",
        ],
        "bash" | "sh" | "shell" | "zsh" => &[
            "case", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in",
            "local", "return", "then", "until", "while",
        ],
        // The data formats share their three literals and nothing else: there
        // is no keyword in a JSON document.
        "json" | "toml" | "yaml" | "yml" => &["false", "null", "true"],
        "go" => &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "type",
            "var",
        ],
        "sql" => &[
            "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE",
            "CREATE", "TABLE", "INDEX", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "GROUP",
            "BY", "ORDER", "HAVING", "LIMIT", "AND", "OR", "NOT", "NULL",
        ],
        "c" | "h" | "cpp" | "cc" | "hpp" | "java" | "csharp" | "cs" => &[
            "auto",
            "break",
            "case",
            "char",
            "class",
            "const",
            "continue",
            "default",
            "do",
            "double",
            "else",
            "enum",
            "extern",
            "false",
            "float",
            "for",
            "if",
            "int",
            "long",
            "namespace",
            "new",
            "nullptr",
            "private",
            "protected",
            "public",
            "return",
            "short",
            "signed",
            "sizeof",
            "static",
            "struct",
            "switch",
            "template",
            "this",
            "throw",
            "true",
            "try",
            "typedef",
            "unsigned",
            "using",
            "virtual",
            "void",
            "while",
        ],
        _ => &[],
    };
    list.iter().copied().collect()
}

/// The longest indentation a rendered line may carry, in columns.
///
/// It is [`MAX_INDENT`] exposed for the view, which is what has to leave room
/// for it before wrapping.
#[must_use]
pub(crate) fn max_indent() -> usize {
    MAX_INDENT
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One rendered line as `text`, for the assertions about content.
    fn texts(text: &str) -> Vec<String> {
        render(text).iter().map(Rendered::text).collect()
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
}
