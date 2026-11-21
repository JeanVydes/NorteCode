//! The shared hints render Spanish labels with real key caps.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use terminalui::{Theme, Tree, View, node};

use super::ShortcutHint;

#[test]
fn chat_hint_shows_labels_and_key_caps() {
    let theme = Theme::default();
    let hint = ShortcutHint::chat(&theme);
    let width = hint.measure().map_or(40, |(width, _)| width);
    let area = Rect::new(0, 0, width, 1);
    let mut buf = Buffer::empty(area);
    let mut tree = Tree::new(node("hint", hint));
    tree.render(area, &mut buf, &theme);
    let text: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
    assert!(text.contains("[ Alt+0-9 ]"), "text was {text}");
    assert!(text.contains("tabs"), "text was {text}");
    assert!(text.contains("send"), "text was {text}");
    assert!(text.contains("newline"), "text was {text}");
}
