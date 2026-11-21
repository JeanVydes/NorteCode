//! The rail renders its context gauge and compact button.

use crossterm::event::{Event, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{Rail, build_tree};
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
    for y in 1..19 {
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
    assert!(text.contains("64 / 128"), "{text}");
    assert!(text.contains("compact at 115"), "{text}");
    assert!(text.contains('█') || text.contains('░'), "{text}");
    assert!(text.contains("compact"), "{text}");
}

#[test]
fn a_session_without_a_window_marks_it_unknown() {
    let text = rail_text(ContextUsage::default());
    assert!(text.contains("0 / ?"), "{text}");
    assert!(text.contains("compact at ?"), "{text}");
}

#[test]
fn the_compact_button_takes_a_focus_slot() {
    let model = UiModel::detached();
    let tree = build_tree(&model);
    assert!(tree.focusable_ids().contains(&"compact"));
}

#[test]
fn arriving_plans_are_visible_and_keep_their_click_rows() {
    let mut model = UiModel::detached();
    model.plans = vec![norte_code_wire::PlanSummary {
        id: "plan-1".to_owned(),
        title: Some("A plan".to_owned()),
        ..Default::default()
    }];
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    tree.find_mut::<Rail>("rail")
        .expect("rail in tree")
        .set_plans(model.plans.clone());
    let area = Rect::new(0, 0, 100, 20);

    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let text = painted_text(&buf, area);
    assert!(text.contains("PLANS"), "{text}");
    assert!(text.contains("A plan"), "{text}");

    let Some(row) = text.lines().position(|line| line.contains("A plan")) else {
        panic!("the plan row is not visible: {text}");
    };
    let row = u16::try_from(row).expect("the plan row fits the terminal");
    let _ = tree.handle_event(&mouse(
        MouseEventKind::Down(crossterm::event::MouseButton::Left),
        10,
        row,
    ));
    let rail = tree.find_mut::<Rail>("rail").expect("rail in tree");
    assert_eq!(rail.take_plan_request().as_deref(), Some("plan-1"));
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> terminalui::InputEvent {
    terminalui::InputEvent::from(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }))
}

fn painted_text(buf: &Buffer, area: Rect) -> String {
    (area.y..area.y + area.height)
        .map(|y| {
            (area.x..area.x + 28)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn rail_labels_round_trip_through_the_enum() {
    for position in [
        RailPosition::Hide,
        RailPosition::Left,
        RailPosition::Right,
        RailPosition::Top,
        RailPosition::Bottom,
    ] {
        assert_eq!(RailPosition::from_label(position.label()), Some(position));
    }
    assert_eq!(RailPosition::from_label("center"), None);
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
