//! The toast is transient: it paints a message and retires itself.

use std::time::Duration;

use ratatui::layout::Rect;
use terminalui::Tree;
use terminalui::components::toast::ToastKind;
use terminalui::{Theme, node};

use super::ToastHost;

#[test]
fn a_toast_paints_its_message_and_expires() {
    let theme = Theme::default();
    let mut host = ToastHost::new();
    host.show(ToastKind::Success, "job finished");
    assert!(host.is_active());

    let area = Rect::new(0, 0, 80, 24);
    let mut tree = Tree::new(node("toast", host));
    let painted = crate::test_support::rendered_in(&mut tree, area, &theme);
    assert!(painted.contains("job finished"), "toast text missing");

    tree.tick(Duration::from_secs(5));
    let host = tree.find::<ToastHost>("toast").expect("host");
    assert!(!host.is_active());
}
