use ratatui::{style::Modifier, text::Line};
use unicode_width::UnicodeWidthStr;

use super::render;

#[test]
fn renders_heading_inline_styles_links_and_html_underline() {
    let lines = render(
        "# Heading\n\n**bold** *italic* ~~strike~~ <u>under</u> [link](https://example.test)",
        80,
    );

    assert!(text(&lines).contains("Heading"));
    assert!(has_modifier(&lines, "Heading", Modifier::BOLD));
    assert!(has_modifier(&lines, "bold", Modifier::BOLD));
    assert!(has_modifier(&lines, "italic", Modifier::ITALIC));
    assert!(has_modifier(&lines, "strike", Modifier::CROSSED_OUT));
    assert!(has_modifier(&lines, "under", Modifier::UNDERLINED));
    assert!(has_modifier(&lines, "link", Modifier::UNDERLINED));
}

#[test]
fn renders_markdown_inside_responsive_tables() {
    let markdown = "| Name | Details |\n| --- | --- |\n| Item | **bold** and *italic* |";
    let wide = render(markdown, 60);
    let narrow = render(markdown, 10);

    assert!(text(&wide).contains("┌"));
    assert!(text(&narrow).contains("Details:"));
    assert!(has_modifier(&narrow, "bold", Modifier::BOLD));
    assert!(wide.iter().all(|line| line_width(line) <= 60));
    assert!(narrow.iter().all(|line| line_width(line) <= 10));
}

fn text(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_width(line: &Line<'static>) -> usize {
    UnicodeWidthStr::width(
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .as_str(),
    )
}

fn has_modifier(lines: &[Line<'static>], text: &str, modifier: Modifier) -> bool {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .filter(|span| span.style.add_modifier.contains(modifier))
        .map(|span| span.content.as_ref())
        .collect::<String>()
        .contains(text)
}
