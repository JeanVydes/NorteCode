use super::render_safe_markdown;

#[test]
fn markdown_html_is_sanitized_before_rendering() {
    let safe_html = render_safe_markdown("**safe** <script>alert('x')</script>");
    assert!(safe_html.contains("<strong>safe</strong>"));
    assert!(!safe_html.contains("<script"));
    assert!(!safe_html.contains("alert('x')"));
}
