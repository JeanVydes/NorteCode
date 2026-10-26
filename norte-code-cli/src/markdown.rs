//! Markdown to ANSI rendering for the interactive prompt.
//!
//! The model's text is escaped before anything is parsed, so a control character
//! or a bidi override in it cannot reach the terminal raw; the SGR sequences
//! this module writes are the only escapes in the result.

use std::fmt::Write as _;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// The SGR sequence that ends any styling.
const RESET: &str = "\x1b[0m";
/// Bold.
const BOLD: &str = "\x1b[1m";
/// Dim, for code and metadata that should recede.
const DIM: &str = "\x1b[2m";
/// Italic.
const ITALIC: &str = "\x1b[3m";
/// Underline, for links.
const UNDERLINE: &str = "\x1b[4m";
/// Strikethrough.
const STRIKE: &str = "\x1b[9m";
/// Bold cyan, the heading colour.
const HEADING: &str = "\x1b[1;36m";
/// Characters a line may occupy before it is left as the terminal draws it.
const MAX_WIDTH: usize = 200;

/// Renders markdown text to ANSI-styled terminal output.
// One arm per pulldown event, so the length is the event vocabulary rather than
// tangled control flow.
#[allow(clippy::too_many_lines)]
pub fn render(text: &str) -> String {
    // The markup is trusted, the text inside it is not: the model writes it, so a
    // control character or a bidi override in it must not reach the terminal raw.
    let text = crate::text::terminal(text);
    let text = text.as_ref();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_GFM);

    let parser = Parser::new_ext(text, options);
    let mut out = String::new();
    // The styling in force inside a table cell. A cell is buffered and joined
    // with its neighbours, so an SGR emitted straight to `out` would land
    // outside the row: the open codes have to travel with the cell's text.
    let mut cell: Option<String> = None;
    let mut row: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut in_head = false;
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut list_depth: usize = 0;
    let mut quote_depth: usize = 0;

    for event in parser {
        match event {
            Event::Start(Tag::Strong) => push(&mut cell, &mut out, BOLD),
            Event::Start(Tag::Emphasis) => push(&mut cell, &mut out, ITALIC),
            Event::Start(Tag::Strikethrough) => push(&mut cell, &mut out, STRIKE),
            Event::Start(Tag::Link { dest_url, .. }) => {
                push(&mut cell, &mut out, UNDERLINE);
                // The destination is not decorative: a link whose target is
                // hidden is not a link a terminal reader can follow.
                push(&mut cell, &mut out, &format!("{dest_url}"));
                push(&mut cell, &mut out, " ");
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                push(&mut cell, &mut out, &format!("[image: {dest_url}]"));
            }
            Event::Start(Tag::Heading { level, .. }) => {
                // The level is the one thing the eye cannot recover from colour.
                let marks = "#".repeat(usize::from(level as u8));
                let _ = write!(out, "{HEADING}{marks} ");
            }
            Event::Start(Tag::BlockQuote(_)) => {
                quote_depth += 1;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_lang = match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                if code_lang.is_empty() {
                    let _ = writeln!(out, "{DIM}──{RESET}");
                } else if code_lang.eq_ignore_ascii_case("mermaid") {
                    // A diagram is a picture in text: the source is shown with
                    // its own label so it reads as one, not as runnable code.
                    let _ = writeln!(out, "{DIM}── mermaid diagram ──{RESET}");
                } else {
                    let _ = writeln!(out, "{DIM}── {code_lang} ──{RESET}");
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                code_lang.clear();
                end_block(&mut out);
            }
            Event::Start(Tag::List(_)) => {
                // A nested list starts inside the item above it, and the item's
                // text has no break of its own before it: without this the inner
                // bullet lands on the outer item's line.
                if list_depth > 0 {
                    out.push('\n');
                }
                list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    end_block(&mut out);
                }
            }
            Event::Start(Tag::Item) => {
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                let _ = write!(out, "{indent}• ");
            }
            Event::Start(Tag::Table(_)) => {
                rows.clear();
            }
            Event::Start(Tag::TableHead) => in_head = true,
            Event::End(TagEnd::TableHead) => {
                // pulldown ends the head without a `TableRow`, so its cells are
                // closed here or the header is dropped.
                in_head = false;
                if !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                }
            }
            Event::End(TagEnd::TableRow) => {
                if !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                }
            }
            Event::End(TagEnd::Table) => {
                render_table(&mut out, &rows);
                rows.clear();
                end_block(&mut out);
            }
            Event::Start(Tag::TableCell) => cell = Some(String::new()),
            Event::End(TagEnd::TableCell) => {
                // A header cell keeps its weight in the rendered table; the flag
                // is read where the row is drawn, so the text is stored plain and
                // the emphasis is re-applied there.
                let text = cell.take().unwrap_or_default();
                row.push(if in_head {
                    format!("\u{1}{text}")
                } else {
                    text
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                out.push_str(RESET);
                end_block(&mut out);
            }
            // The block a paragraph is. pulldown ends one here without ever
            // saying it started, and this arm is what keeps two of them apart.
            Event::End(TagEnd::Paragraph) => end_block(&mut out),
            Event::End(TagEnd::BlockQuote(_)) => {
                quote_depth = quote_depth.saturating_sub(1);
                end_block(&mut out);
            }
            Event::End(
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
            ) => {
                push(&mut cell, &mut out, RESET);
            }
            // Every run of literal text is escaped again: a `\u{1b}` in the source
            // cannot become an escape here just because it was written that way.
            Event::Text(t) => {
                let t = crate::text::terminal(&t);
                if in_code_block {
                    for line in t.lines() {
                        let _ = writeln!(out, "  {}", clip(line));
                    }
                    if t == "\n" {
                        out.push('\n');
                    }
                } else {
                    // A newline inside a cell would end the row, so it is folded
                    // away there; outside one it is kept as written.
                    push_cell_or_out(&mut cell, &mut out, &t);
                }
            }
            // Inline HTML is text the model wrote: it is shown as it came, never
            // interpreted, because this renderer has no HTML to draw with.
            Event::Html(html) | Event::InlineHtml(html) => {
                let html = crate::text::terminal(&html);
                let html = html.trim();
                if !html.is_empty() {
                    push(&mut cell, &mut out, &format!("{DIM}{html}{RESET}"));
                }
            }
            Event::Code(t) => {
                let code = format!("{DIM}{}{RESET}", crate::text::terminal(&t));
                push(&mut cell, &mut out, &code);
            }
            Event::SoftBreak => {
                if cell.is_some() {
                    push(&mut cell, &mut out, " ");
                } else {
                    out.push('\n');
                }
            }
            Event::End(TagEnd::Item) => {
                // A tight item ends on its text with nothing after it, so the
                // break is owed here; a loose one holds a paragraph that already
                // left the gap, and adding another put a blank line between every
                // pair of items.
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Event::HardBreak => out.push('\n'),
            Event::Rule => {
                let _ = writeln!(out, "{DIM}────────────────{RESET}");
                end_block(&mut out);
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x]" } else { "[ ]" };
                push(&mut cell, &mut out, mark);
            }
            Event::FootnoteReference(name) => {
                push(&mut cell, &mut out, &format!("[{name}]"));
            }
            _ => {}
        }
    }
    // The last block closed itself with a blank line; the caller adds the one
    // separator it wants, so the result ends on a single newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Ends the block being written, leaving one blank line after it.
///
/// pulldown reports a block by its end and gives no start event for a paragraph
/// or a rule, so without this the next block begins on the last line of the one
/// before it: a heading ran into the paragraph above it, every paragraph into the
/// next, and a list into whatever followed. Trailing newlines are dropped first,
/// so a block that already ended the line does not leave a taller gap for it.
fn end_block(out: &mut String) {
    if out.is_empty() {
        return;
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out.push_str("\n\n");
}

/// Writes a sequence either into the cell being buffered or straight out.
fn push(cell: &mut Option<String>, out: &mut String, text: &str) {
    match cell {
        Some(cell) => cell.push_str(text),
        None => out.push_str(text),
    }
}

/// Writes text that may carry newlines, keeping the line breaks only outside a
/// cell, where a break would end the row.
fn push_cell_or_out(cell: &mut Option<String>, out: &mut String, text: &str) {
    match cell {
        Some(cell) => {
            for line in text.lines() {
                cell.push_str(line);
            }
        }
        None => out.push_str(&crate::text::terminal(text)),
    }
}

/// Clips one line so a long code line cannot wrap the whole terminal.
fn clip(line: &str) -> String {
    let visible = line.chars().count();
    if visible <= MAX_WIDTH {
        return line.to_owned();
    }
    let kept: String = line.chars().take(MAX_WIDTH.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Draws the collected rows as a bordered table with aligned columns.
///
/// A header cell is marked in the buffer by a leading `\u{1}` so the emphasis is
/// applied to the cell alone, after the widths are known.
fn render_table(out: &mut String, rows: &[Vec<String>]) {
    if rows.is_empty() {
        return;
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return;
    }
    // A wide cell would push the borders off the screen, so a column is capped
    // and its overflow is marked.
    let mut widths = vec![0usize; columns];
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            let plain = cell.strip_prefix('\u{1}').unwrap_or(cell);
            widths[index] = widths[index].max(visible_width(plain));
        }
    }
    let mut rule = String::from("─");
    for width in &widths {
        rule.push_str(&"─".repeat(width + 2));
        rule.push('┼');
    }
    rule.pop();

    for (index, row) in rows.iter().enumerate() {
        let _ = write!(out, "{DIM}│{RESET}");
        for (column, width) in widths.iter().enumerate() {
            let cell = row.get(column).map_or("", String::as_str);
            let header = cell.starts_with('\u{1}');
            let text = cell.strip_prefix('\u{1}').unwrap_or(cell);
            let text = clip_padded(text, *width);
            if header {
                let _ = write!(out, " {BOLD}{text}{RESET} {DIM}│{RESET}");
            } else {
                let _ = write!(out, " {text} {DIM}│{RESET}");
            }
        }
        out.push('\n');
        if index == 0 {
            // After the header row only; a table without one has no rule.
            let _ = writeln!(out, "{DIM}{rule}{RESET}");
        }
    }
}

/// The visible length of a cell, ignoring the SGR sequences inside it.
fn visible_width(text: &str) -> usize {
    let mut width = 0usize;
    let mut in_escape = false;
    for character in text.chars() {
        match character {
            '\x1b' => in_escape = true,
            'm' if in_escape => in_escape = false,
            _ if !in_escape => width += 1,
            _ => {}
        }
    }
    width
}

/// Pads or clips a cell to `width` visible characters.
fn clip_padded(text: &str, width: usize) -> String {
    if visible_width(text) <= width {
        let padding = width.saturating_sub(visible_width(text));
        return format!("{text}{}", " ".repeat(padding));
    }
    // Clipping styled text by visible width: take the characters that fit and
    // close whatever style was open, so the border is not coloured by a leak.
    let mut kept = String::new();
    let mut visible = 0usize;
    let mut in_escape = false;
    for character in text.chars() {
        if character == '\x1b' {
            in_escape = true;
            kept.push(character);
            continue;
        }
        if in_escape {
            kept.push(character);
            if character == 'm' {
                in_escape = false;
            }
            continue;
        }
        if visible + 1 == width {
            kept.push('…');
            break;
        }
        kept.push(character);
        visible += 1;
    }
    if kept.contains('\x1b') {
        kept.push_str(RESET);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::render;

    /// The document a block-separation test renders: every kind of block in a
    /// row, so the one after a given block lands somewhere it can be checked.
    const DOC: &str = "intro paragraph\n\n# Heading\n\nsecond paragraph\n\n- one\n- two\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nlet x = 1;\n```\n\nafter the fence\n\n---\n\nlast";

    /// The rendered text with its SGR sequences stripped, so a test reads the
    /// text rather than the colour.
    fn plain(out: &str) -> String {
        let mut text = String::new();
        let mut in_escape = false;
        for character in out.chars() {
            match character {
                '\x1b' => in_escape = true,
                'm' if in_escape => in_escape = false,
                _ if !in_escape => text.push(character),
                _ => {}
            }
        }
        text
    }

    /// The line a needle is on, with the styling stripped.
    fn line_with(out: &str, needle: &str) -> String {
        plain(out)
            .lines()
            .find(|line| line.contains(needle))
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    fn a_block_is_separated_from_the_one_after_it() {
        // The renderer handles a block by its end, and it had no arm for a
        // paragraph and no arm that closed the block before it, so `## Heading`
        // was appended to the paragraph above and read as part of that line.
        // The document is compared whole: the defect is a line that is missing
        // between two blocks, and a test that reads one line at a time by name
        // cannot see a line it never asked to be there.
        let plain = plain(&render(DOC));
        let expected = [
            "intro paragraph",
            "",
            "# Heading",
            "",
            "second paragraph",
            "",
            "• one",
            "• two",
            "",
            "quoted",
            "",
            "│ a │ b │",
            "────┼───",
            "│ 1 │ 2 │",
            "",
            "── rust ──",
            "  let x = 1;",
            "",
            "after the fence",
            "",
            "────────────────",
            "",
            "last",
            "",
        ]
        .join("\n");
        assert_eq!(plain, expected, "a block runs into the one after it");
    }

    #[test]
    fn a_tight_list_does_not_leave_a_blank_line_between_items() {
        // An item ends by writing a break, and a paragraph already wrote one:
        // two breaks between every pair of items is a list with air between the
        // rows, which is not how the source was written.
        let out = render("- one\n- two\n- three");
        let lines: Vec<&str> = out.lines().collect();
        let first = lines.iter().position(|line| line.contains("one")).unwrap();
        let second = lines.iter().position(|line| line.contains("two")).unwrap();
        assert_eq!(second - first, 1, "the items are not adjacent: {out:?}");
    }

    #[test]
    fn a_nested_item_does_not_land_on_the_outer_items_line() {
        // A nested list starts inside the item above it and the outer item has no
        // break before it, so the inner bullet was written on the outer line.
        let out = render("- one\n  - inner\n- two");
        assert_eq!(
            line_with(&out, "inner").trim(),
            "• inner",
            "the inner item shares the outer item's line: {out:?}"
        );
    }

    #[test]
    fn a_loose_list_still_separates_its_items() {
        // The opposite of the tight case: a blank line between items in the
        // source is a blank line in the output, not one dropped or tripled.
        let out = render("- one\n\n- two");
        let lines: Vec<&str> = out.lines().collect();
        let first = lines.iter().position(|line| line.contains("one")).unwrap();
        let second = lines.iter().position(|line| line.contains("two")).unwrap();
        assert_eq!(
            second - first,
            2,
            "the loose list's gap is not one line: {out:?}"
        );
    }

    #[test]
    fn bold_italic_and_strikethrough_carry_their_own_style() {
        let out = render("**bold** and *italic* and ~~gone~~");
        assert!(out.contains("\x1b[1mbold\x1b[0m"), "{out:?}");
        assert!(out.contains("\x1b[3mitalic\x1b[0m"), "{out:?}");
        assert!(out.contains("\x1b[9mgone\x1b[0m"), "{out:?}");
    }

    #[test]
    fn a_heading_is_marked_and_coloured() {
        let out = render("## Title\n\nbody");
        assert!(out.contains("\x1b[1;36m## Title\x1b[0m"), "{out:?}");
        assert!(out.contains("body"), "{out:?}");
    }

    #[test]
    fn bold_inside_a_table_cell_keeps_its_style() {
        // The cell is buffered and joined with its neighbours, so an SGR written
        // straight to the output landed outside the row and the emphasis vanished.
        let out = render("| a | b |\n|---|---|\n| **bold** | plain |");
        assert!(out.contains("\x1b[1mbold\x1b[0m"), "{out:?}");
        assert!(out.contains("plain"), "{out:?}");
    }

    #[test]
    fn a_table_has_borders_and_a_rule_under_the_header() {
        let out = render("| left | right |\n|---|---|\n| 1 | 2 |");
        assert!(out.contains('│'), "the columns are divided: {out:?}");
        assert!(out.contains('┼'), "the header rule crosses them: {out:?}");
    }

    #[test]
    fn a_table_pads_its_cells_to_the_column_width() {
        let out = render("| a | longer |\n|---|---|\n| x | y |");
        let widths: Vec<usize> = out
            .lines()
            .filter(|line| line.contains('│'))
            .map(super::visible_width)
            .collect();
        // The header and the body row, which must end on the same column; the
        // rule between them draws ┼ rather than │.
        assert_eq!(widths.len(), 2, "{widths:?}");
        assert_eq!(widths[0], widths[1], "rows differ in width: {widths:?}");
    }

    #[test]
    fn html_is_shown_as_text_and_never_interpreted() {
        let out = render("<div class=\"x\">hi</div>");
        assert!(out.contains("<div"), "{out:?}");
        assert!(out.contains("hi"), "{out:?}");
        assert!(out.contains("\x1b[2m"), "the markup is dimmed: {out:?}");
    }

    #[test]
    fn a_mermaid_block_is_labelled_as_a_diagram() {
        let out = render("```mermaid\ngraph TD;\n  A-->B;\n```");
        assert!(out.contains("mermaid diagram"), "{out:?}");
        assert!(out.contains("graph TD;"), "the source is shown: {out:?}");
    }

    #[test]
    fn a_link_shows_its_destination() {
        let out = render("[docs](https://example.com/x)");
        assert!(out.contains("https://example.com/x"), "{out:?}");
        assert!(out.contains("docs"), "{out:?}");
    }

    #[test]
    fn an_image_names_its_source() {
        let out = render("![alt](https://example.com/a.png)");
        assert!(
            out.contains("[image: https://example.com/a.png]"),
            "{out:?}"
        );
    }

    #[test]
    fn a_control_character_in_the_text_cannot_become_an_escape() {
        // The model writes the text, so an escape it spells out must stay spelled
        // out rather than reach the terminal as a real sequence.
        let out = render("safe\\u{1b}[2Jowned");
        assert!(!out.contains("\x1b[2J"), "{out:?}");
        assert!(out.contains("owned"), "{out:?}");
    }

    #[test]
    fn a_long_code_line_is_clipped() {
        let long = "x".repeat(500);
        let out = render(&format!("```\n{long}\n```"));
        assert!(out.contains('…'), "the line is marked as clipped: {out:?}");
        assert!(
            out.lines().all(|line| line.chars().count() <= 220),
            "no line runs away: {}",
            out.lines().map(|l| l.chars().count()).max().unwrap_or(0)
        );
    }

    #[test]
    fn a_task_list_keeps_its_boxes() {
        let out = render("- [x] done\n- [ ] todo");
        assert!(out.contains("[x]"), "{out:?}");
        assert!(out.contains("[ ]"), "{out:?}");
    }
}
