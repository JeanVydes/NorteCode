//! The detail panel renders the fields supplied by the host.

use ratatui::layout::Rect;
use terminalui::components::badge::BadgeVariant;
use terminalui::{Theme, Tree, node};

use super::detail_panel::{DetailPanel, Field};

#[test]
fn the_panel_paints_its_title_and_fields() {
    let mut panel = DetailPanel::new();
    panel.set(
        "model",
        vec![
            Field::new("provider", "openrouter"),
            Field::badge("key", "saved key", BadgeVariant::Success),
            Field::separator(),
            Field::new("hint", "ctrl+d to remove"),
        ],
    );

    let mut tree = Tree::new(node("detail", panel));
    let painted =
        crate::test_support::rendered_in(&mut tree, Rect::new(0, 0, 40, 10), &Theme::default());

    assert!(painted.contains("model"), "{painted}");
    assert!(painted.contains("provider"), "{painted}");
    assert!(painted.contains("openrouter"), "{painted}");
    assert!(painted.contains("saved key"), "{painted}");
    assert!(painted.contains("ctrl+d to remove"), "{painted}");
}
