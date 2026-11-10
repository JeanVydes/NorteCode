use dioxus::prelude::*;
use pulldown_cmark::{Options, Parser, html};

#[component]
pub fn Markdown(content: String) -> Element {
    let safe_html = render_safe_markdown(&content);

    rsx! {
        div {
            class: "markdown-body",
            dangerous_inner_html: safe_html,
        }
    }
}

pub(crate) fn render_safe_markdown(content: &str) -> String {
    let mut rendered = String::new();
    html::push_html(&mut rendered, Parser::new_ext(content, Options::all()));
    ammonia::Builder::default().clean(&rendered).to_string()
}

#[cfg(test)]
mod tests {
    use super::render_safe_markdown;

    #[test]
    fn markdown_html_is_sanitized_before_rendering() {
        let safe_html = render_safe_markdown("**safe** <script>alert('x')</script>");
        assert!(safe_html.contains("<strong>safe</strong>"));
        assert!(!safe_html.contains("<script"));
        assert!(!safe_html.contains("alert('x')"));
    }
}
