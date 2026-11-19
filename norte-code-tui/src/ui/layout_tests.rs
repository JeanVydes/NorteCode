//! Rendered position of the rail at each preference value.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::build_tree;
use crate::ui::RailPosition;
use crate::ui_model::UiModel;

const WORD: &[char] = &['S', 'E', 'S', 'S', 'I', 'O', 'N'];

fn session_origin(position: RailPosition) -> Option<(u16, u16)> {
    let mut model = UiModel::detached();
    model.preferences.rail_position = position;
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    let area = Rect::new(0, 0, 100, 40);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    for y in 0..area.height {
        for x in 0..area.width {
            if WORD.iter().enumerate().all(|(i, ch)| {
                let offset = u16::try_from(i).unwrap_or(u16::MAX);
                buf[(x.saturating_add(offset), y)].symbol() == ch.to_string()
            }) {
                return Some((x, y));
            }
        }
    }
    None
}

#[test]
fn hidden_rail_draws_no_session_header() {
    assert!(session_origin(RailPosition::Hide).is_none());
}

#[test]
fn left_rail_draws_the_session_header_on_the_left() {
    let (x, _) = session_origin(RailPosition::Left).expect("SESSION");
    assert!(x < 28, "SESSION at {x}");
}

#[test]
fn right_rail_draws_the_session_header_on_the_right() {
    let (x, _) = session_origin(RailPosition::Right).expect("SESSION");
    assert!(x >= 72, "SESSION at {x}");
}

#[test]
fn top_rail_draws_the_session_header_above_the_stream() {
    let (_, y) = session_origin(RailPosition::Top).expect("SESSION");
    assert!(y < 9, "SESSION at {y}");
}

#[test]
fn bottom_rail_draws_the_session_header_below_the_stream() {
    let (_, y) = session_origin(RailPosition::Bottom).expect("SESSION");
    assert!(y >= 24, "SESSION at {y}");
}
