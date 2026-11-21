//! Rendering behavior of the single Markdown document renderer.

use ratatui::style::Modifier;
use terminalui::Theme;

use super::Markdown;

fn text(markdown: &Markdown) -> Vec<String> {
    markdown
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

fn span_with(markdown: &Markdown, text: &str) -> ratatui::text::Span<'static> {
    markdown
        .lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content == text)
        .unwrap_or_else(|| panic!("no span with content {text:?}"))
        .clone()
}

#[test]
fn inline_emphasis_keeps_its_style() {
    let doc = Markdown::render(
        "Use **bold** and *italic*, then **ID:** 42",
        80,
        &Theme::default(),
    );

    assert_eq!(
        text(&doc),
        vec!["Use bold and italic, then ID: 42".to_string()]
    );
    assert!(
        span_with(&doc, "bold")
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        span_with(&doc, "italic")
            .style
            .add_modifier
            .contains(Modifier::ITALIC)
    );
    assert!(
        span_with(&doc, "ID:")
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn headings_render_at_every_level() {
    let doc = Markdown::render("# One\n## Two\n###### Six", 80, &Theme::default());

    assert_eq!(text(&doc), vec!["One", "Two", "Six"]);
    for heading in ["One", "Two", "Six"] {
        assert!(
            span_with(&doc, heading)
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }
    for heading in ["One", "Two"] {
        assert!(
            span_with(&doc, heading)
                .style
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }
    assert!(
        !span_with(&doc, "Six")
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED)
    );
}

#[test]
fn tables_use_a_grid_when_wide_and_labels_when_narrow() {
    let source = "| Name | Role | Note |\n| --- | --- | --- |\n| Ana | Dev | Lead |";
    let wide = Markdown::render(source, 60, &Theme::default());
    assert!(text(&wide).iter().any(|line| line.starts_with('┌')));
    assert!(text(&wide).iter().any(|line| line.contains("Ana")));

    let narrow = Markdown::render(source, 20, &Theme::default());
    let narrow_text = text(&narrow);
    assert!(!narrow_text.iter().any(|line| line.starts_with('┌')));
    assert!(narrow_text.iter().any(|line| line.contains("Name: Ana")));
    assert!(narrow_text.iter().any(|line| line.contains("Role: Dev")));
    assert!(narrow_text.iter().any(|line| line.contains("Note: Lead")));
}

#[test]
fn link_hit_testing_uses_rendered_columns() {
    let doc = Markdown::render("[docs](https://example.com/path)", 80, &Theme::default());

    assert_eq!(text(&doc), vec!["docs"]);
    assert_eq!(doc.link_at(0, 0), Some("https://example.com/path"));
    assert_eq!(doc.link_at(0, 3), Some("https://example.com/path"));
    assert_eq!(doc.link_at(0, 4), None);
    assert_eq!(doc.link_at(1, 0), None);
}
