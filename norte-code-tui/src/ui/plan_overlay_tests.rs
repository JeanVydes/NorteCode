//! Rendering behavior of the plan preview overlay.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::PlanOverlay;
use crate::ui::{build_tree, sync_views};
use crate::ui_model::UiModel;

fn overlay_text(content: &str) -> String {
    let model = UiModel::detached();
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    tree.find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .open(norte_code_wire::PlanSummary {
            id: "plan-1".to_owned(),
            content: content.to_owned(),
            ..Default::default()
        });
    sync_views(&mut tree, &model);
    let area = Rect::new(0, 0, 80, 20);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_preview_numbers_markdown_lines_from_one() {
    let text = overlay_text("# Heading\n\nBody");

    assert!(text.contains("Heading"), "{text}");
    assert!(text.contains("Body"), "{text}");
    assert!(text.contains("   1  Heading"), "{text}");
    assert!(text.contains("   2  Body"), "{text}");
}

#[test]
fn the_preview_renders_inline_emphasis() {
    let text = overlay_text("Use **ID:** 42");

    assert!(text.contains("ID: 42"), "{text}");
    assert!(!text.contains("**"), "{text}");
}
