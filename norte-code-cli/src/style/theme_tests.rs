use super::*;

#[test]
fn the_palette_is_orange() {
    assert_eq!(PRIMARY, (0xFF, 0x5C, 0xD6));
    assert_eq!(PRIMARY_256, 201);
}

#[test]
fn the_icon_set_is_safe_unicode() {
    assert_eq!(icon::DONE, "✓");
    assert_eq!(icon::PENDING, "○");
    assert_eq!(icon::RUNNING, "●");
    assert_eq!(icon::PROMPT, "›");
    assert_eq!(icon::MORE, "…");
    assert_eq!(icon::RULE, "─");
    assert_eq!(icon::BOX_TOP, "╭");
    assert_eq!(icon::BOX_BOTTOM, "╰");
}
