//! Markdown to ANSI rendering for the interactive prompt.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// Renders markdown text to ANSI-styled terminal output.
pub fn render(text: &str) -> String {
    let parser = Parser::new(text);
    let mut out = String::new();
    let mut in_code_block = false;

    for event in parser {
        match event {
            Event::Start(Tag::Strong) => out.push_str("\x1b[1m"),
            Event::End(TagEnd::Strong) => out.push_str("\x1b[0m"),
            Event::Start(Tag::Emphasis) => out.push_str("\x1b[3m"),
            Event::End(TagEnd::Emphasis) => out.push_str("\x1b[0m"),
            // Inline code is handled by Event::Code below, not by a tag.
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
            Event::Start(Tag::Item) => out.push_str("  • "),
            Event::End(TagEnd::Item) => out.push('\n'),
            Event::Start(Tag::Link { dest_url, .. }) => {
                out.push_str(&format!("\x1b[4m"));
                let _ = dest_url;
            }
            Event::End(TagEnd::Link) => out.push_str("\x1b[0m"),
            Event::Text(t) => {
                if in_code_block {
                    for line in t.lines() {
                        out.push_str("  ");
                        out.push_str(line);
                        out.push('\n');
                    }
                } else {
                    out.push_str(&t);
                }
            }
            Event::Code(t) => {
                out.push_str(&format!("\x1b[2m{}\x1b[0m", t));
            }
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push_str("\n"),
            Event::Rule => out.push_str("\x1b[2m────────────────\x1b[0m\n"),
            _ => {}
        }
    }

    out
}
