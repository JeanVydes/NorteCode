//! Geometry and drag math shared by the transcript and chart rails.

use super::Scrollbar;

#[test]
fn a_click_on_the_thumb_is_not_a_page_jump() {
    let bar = Scrollbar::new(80, 2, 20, 6, 4);
    assert!(bar.on_rail(80, 9));
    assert!(!bar.on_rail(79, 9));
    assert!(!bar.on_rail(80, 30));
    assert!(bar.thumb_contains(7));
    assert!(!bar.thumb_contains(3));
    assert_eq!(bar.page(3), -1);
    assert_eq!(bar.page(15), 1);
}

#[test]
fn dragging_the_thumb_maps_track_position_to_offset() {
    let bar = Scrollbar::new(80, 2, 22, 0, 2);
    assert_eq!(bar.offset_for_drag(1, 100), 0);
    assert_eq!(bar.offset_for_drag(11, 100), 50);
    assert_eq!(bar.offset_for_drag(21, 100), 100);
}

#[test]
fn a_thumb_filling_the_track_cannot_be_dragged() {
    let bar = Scrollbar::new(80, 2, 10, 0, 10);
    assert_eq!(bar.offset_for_drag(5, 40), 0);
}
