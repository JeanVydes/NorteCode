//! Painting helpers for telemetry chart slots.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::Theme;

use crate::ui::telemetry_page::{CHART_ROWS, CHART_SLOT};

#[test]
fn slot_title_paints_the_title_and_returns_the_body() {
    let theme = Theme::default();
    let area = Rect::new(0, 0, 30, CHART_ROWS);
    let mut page = Buffer::empty(area);
    let body = crate::ui::chart_renderer::paint_slot_title(&mut page, 2, 20, "cost", &theme);
    assert_eq!(body.y, 3);
    assert_eq!(body.width, 20);
    assert_eq!(body.height, CHART_SLOT - 1);
    let mut text = String::new();
    for x in 0..4 {
        text.push_str(page[(x, 2)].symbol());
    }
    assert!(text.contains("cost"), "{text}");
}

#[test]
fn time_labels_paints_every_tenth_label() {
    let theme = Theme::default();
    let area = Rect::new(0, 0, 60, 5);
    let mut page = Buffer::empty(area);
    let labels = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
        "e".to_string(),
        "f".to_string(),
    ];
    crate::ui::chart_renderer::paint_time_labels(&labels, &mut page, 1, 40, (0, 6), &theme);
    let mut painted = String::new();
    for y in 0..5 {
        for x in 0..60 {
            painted.push_str(page[(x, y)].symbol());
        }
    }
    assert!(painted.contains('a'), "{painted}");
}

#[test]
fn blit_rows_copies_inside_the_area_and_skips_outside() {
    let theme = Theme::default();
    let area = Rect::new(0, 0, 10, 2);
    let mut page = Buffer::empty(area);
    crate::ui::chart_renderer::paint_slot_title(&mut page, 0, 10, "title", &theme);
    let mut frame = Buffer::empty(area);
    crate::ui::chart_renderer::blit_rows(&page, &mut frame, area, 0);
    let mut text = String::new();
    for x in 0..5 {
        text.push_str(frame[(x, 0)].symbol());
    }
    assert!(text.contains("title"), "{text}");
    let outside = Rect::new(20, 0, 10, 2);
    crate::ui::chart_renderer::blit_rows(&page, &mut frame, outside, 0);
}
