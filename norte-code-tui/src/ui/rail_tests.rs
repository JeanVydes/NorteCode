//! The rail renders its context gauge and compact button.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::build_tree;
use crate::context::ContextUsage;
use crate::ui::RailPosition;
use crate::ui_model::UiModel;

fn rail_text(usage: ContextUsage) -> String {
    let mut model = UiModel::detached();
    model.context = usage;
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    let area = Rect::new(0, 0, 100, 40);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let mut out = String::new();
    for y in 1..16 {
        for x in 0..28 {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_rail_shows_context_usage_and_the_compact_button() {
    let text = rail_text(ContextUsage {
        used: 64,
        window: Some(128),
    });
    assert!(text.contains("CONTEXT"), "{text}");
    assert!(text.contains("64/128"), "{text}");
    assert!(text.contains("compact 115"), "{text}");
    assert!(text.contains('█') || text.contains('░'), "{text}");
    assert!(text.contains("compact"), "{text}");
}

#[test]
fn a_session_without_a_window_says_so() {
    let text = rail_text(ContextUsage::default());
    assert!(text.contains("no context reported"), "{text}");
}

#[test]
fn the_compact_button_takes_a_focus_slot() {
    let model = UiModel::detached();
    let tree = build_tree(&model);
    assert!(tree.focusable_ids().contains(&"compact"));
}

#[test]
fn hiding_the_rail_removes_its_ink_but_not_its_focus_slot() {
    let mut model = UiModel::detached();
    model.preferences.rail_position = RailPosition::Hide;
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    let area = Rect::new(0, 0, 100, 40);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    assert_eq!(tree.focused_id(), Some("composer"));
    let mut painted = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            painted.push_str(buf[(x, y)].symbol());
        }
    }
    assert!(!painted.contains("CONTEXT"), "the hidden rail still drew");
    assert!(tree.focusable_ids().contains(&"compact"));
}
