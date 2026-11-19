//! Geometry of the chat layout at each rail position.

use ratatui::layout::Rect;

use super::ChatLayout;
use crate::ui::RailPosition;

fn area() -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 40,
    }
}

fn split(position: RailPosition) -> (Option<Rect>, Rect) {
    ChatLayout::new(position).split(area())
}

#[test]
fn hidden_rail_gives_the_stream_everything() {
    let (rail, stream) = split(RailPosition::Hide);
    let rail = rail.expect("rail still renders off-screen for focus");
    assert_eq!((rail.width, rail.height), (0, 0));
    assert_eq!(stream, area());
}

#[test]
fn left_rail_precedes_the_stream() {
    let (rail, stream) = split(RailPosition::Left);
    let rail = rail.expect("rail");
    assert_eq!((rail.x, rail.width, rail.height), (0, 28, 40));
    assert_eq!((stream.x, stream.width), (28, 72));
}

#[test]
fn right_rail_follows_the_stream() {
    let (rail, stream) = split(RailPosition::Right);
    let rail = rail.expect("rail");
    assert_eq!((rail.x, rail.width), (72, 28));
    assert_eq!((stream.x, stream.width), (0, 72));
}

#[test]
fn top_rail_sits_above_the_stream() {
    let (rail, stream) = split(RailPosition::Top);
    let rail = rail.expect("rail");
    assert_eq!((rail.y, rail.height, rail.width), (0, 9, 100));
    assert_eq!((stream.y, stream.height), (9, 31));
}

#[test]
fn bottom_rail_sits_below_the_stream() {
    let (rail, stream) = split(RailPosition::Bottom);
    let rail = rail.expect("rail");
    assert_eq!((rail.y, rail.height), (31, 9));
    assert_eq!((stream.y, stream.height), (0, 31));
}

#[test]
fn a_narrow_area_never_underflows() {
    let layout = ChatLayout::new(RailPosition::Right);
    let (rail, stream) = layout.split(Rect {
        width: 4,
        height: 2,
        ..area()
    });
    assert_eq!(rail.expect("rail").width, 4);
    assert_eq!(stream.width, 0);
}
