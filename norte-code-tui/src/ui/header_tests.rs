//! The header reports running jobs as a spinner next to the turn state.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::Header;
use crate::ui::build_tree;
use crate::ui_model::UiModel;

fn header_row(jobs: usize) -> (Buffer, Rect) {
    let model = UiModel::detached();
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    tree.find_mut::<Header>("header")
        .expect("header")
        .set_jobs(jobs);
    let area = Rect::new(0, 0, 160, 30);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    (buf, area)
}

#[test]
fn running_jobs_add_activity_next_to_the_turn_state() {
    let (idle, area) = header_row(0);
    let (busy, _) = header_row(2);

    let changed: Vec<u16> = (0..area.width)
        .filter(|x| idle[(*x, 0)].symbol() != busy[(*x, 0)].symbol())
        .collect();
    assert!(!changed.is_empty(), "the spinner must paint something");
    assert!(
        changed.iter().all(|x| *x >= 100),
        "job activity belongs next to the state, not over the tabs"
    );
    let painted: String = changed.iter().map(|x| busy[(*x, 0)].symbol()).collect();
    assert!(painted.contains('2'));
}

#[test]
fn many_jobs_are_capped_for_narrow_header_space() {
    let (busy, area) = header_row(150);
    let painted = (0..area.width)
        .map(|x| busy[(x, 0)].symbol())
        .collect::<String>();
    assert!(painted.contains("99+"));
}
