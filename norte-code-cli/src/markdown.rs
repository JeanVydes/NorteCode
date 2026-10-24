//! Markdown to ANSI rendering for the interactive prompt.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Renders markdown text to ANSI-styled terminal output.
pub fn render(text: &str) -> String {
    // The markup is trusted, the text inside it is not: the model writes it, so a
    // control character or a bidi override in it must not reach the terminal raw.
    let text = crate::text::terminal(text);
    let text = text.as_ref();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(text, options);
    let mut out = String::new();
    let mut in_code_block = false;
    let mut table_row: Vec<String> = Vec::new();
    let mut table_cell = String::new();
    let mut in_table_cell = false;
    let mut list_depth: usize = 0;

    for event in parser {
        match event {
            Event::Start(Tag::Strong) => out.push_str("\x1b[1m"),
            Event::End(TagEnd::Strong) => out.push_str("\x1b[0m"),
            Event::Start(Tag::Emphasis) => out.push_str("\x1b[3m"),
            Event::End(TagEnd::Emphasis) => out.push_str("\x1b[0m"),
            Event::Start(Tag::Strikethrough) => out.push_str("\x1b[9m"),
            Event::End(TagEnd::Strikethrough) => out.push_str("\x1b[0m"),
            Event::Start(Tag::Heading { .. }) => out.push_str("\x1b[1;36m"),
            Event::End(TagEnd::Heading { .. }) => out.push_str("\x1b[0m\n"),
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                let code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                    _ => String::new(),
                };
                if !code_lang.is_empty() {
                    out.push_str(&format!("\x1b[2m── {} ──\x1b[0m\n", code_lang));
                } else {
                    out.push_str("\x1b[2m──\x1b[0m\n");
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                out.push('\n');
            }
            Event::Start(Tag::List(_)) => {
                list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                out.push('\n');
            }
            Event::Start(Tag::Item) => {
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                out.push_str(&format!("{}• ", indent));
            }
            Event::End(TagEnd::Item) => out.push('\n'),
            Event::Start(Tag::Table(_)) => {
                table_row.clear();
            }
            Event::End(TagEnd::Table) => {
                out.push('\n');
            }
            Event::Start(Tag::TableHead) => {}
            Event::End(TagEnd::TableHead) => {
                // Print header row
                if !table_row.is_empty() {
                    out.push_str(&table_row.join(" | "));
                    out.push('\n');
                    out.push_str(
                        &table_row
                            .iter()
                            .map(|c| "-".repeat(c.len().max(3)))
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                    out.push('\n');
                    table_row.clear();
                }
            }
            Event::Start(Tag::TableRow) => {
                table_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                out.push_str(&table_row.join(" | "));
                out.push('\n');
                table_row.clear();
            }
            Event::Start(Tag::TableCell) => {
                in_table_cell = true;
                table_cell.clear();
            }
            Event::End(TagEnd::TableCell) => {
                in_table_cell = false;
                table_row.push(table_cell.clone());
                table_cell.clear();
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                out.push_str(&format!("\x1b[4m"));
                let _ = dest_url;
            }
            Event::End(TagEnd::Link) => out.push_str("\x1b[0m"),
            // Every run of literal text is escaped again: a `\u{1b}` in the source
            // cannot become an escape here just because it was written that way.
            Event::Text(t) => {
                let t = crate::text::terminal(&t);
                let t = t.as_ref();
                if in_code_block {
                    for line in t.lines() {
                        out.push_str("  ");
                        out.push_str(line);
                        out.push('\n');
                    }
                } else if in_table_cell {
                    table_cell.push_str(t);
                } else {
                    out.push_str(t);
                }
            }
            Event::Code(t) => {
                out.push_str("\x1b[2m");
                out.push_str(&crate::text::terminal(&t));
                out.push_str("\x1b[0m");
            }
            Event::SoftBreak => {
                if !in_table_cell {
                    out.push('\n');
                }
            }
            Event::HardBreak => out.push_str("\n"),
            Event::Rule => out.push_str("\x1b[2m────────────────\x1b[0m\n"),
            _ => {}
        }
    }

    out
}
