//! Settings keeps its section order and scrolls when it overflows.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::InputEvent;

use crate::ui::{build_tree, sync_views};
use crate::ui_model::{Tab, UiModel};

fn render_settings(height: u16) -> (Buffer, Rect) {
    let mut model = UiModel::detached();
    model.tab = Tab::Settings;
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    sync_views(&mut tree, &model);
    let area = Rect::new(0, 0, 80, height);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    (buf, area)
}

fn row_text(buf: &Buffer, area: Rect, y: u16) -> String {
    (0..area.width)
        .map(|x| buf[(x, y)].symbol())
        .collect::<String>()
}

fn heading_y(buf: &Buffer, area: Rect, heading: &str) -> Option<u16> {
    (0..area.height).find(|y| row_text(buf, area, *y).contains(heading))
}

#[test]
fn preferences_stay_between_plugins_and_agents() {
    let (buf, area) = render_settings(100);
    let plugins = heading_y(&buf, area, "plugins");
    let preferences = heading_y(&buf, area, "preferences");
    let agents = heading_y(&buf, area, "agents");

    assert!(plugins.is_some_and(|y| preferences.is_some_and(|p| y < p)));
    assert!(preferences.is_some_and(|y| agents.is_some_and(|a| y < a)));
}

#[test]
fn overflowing_settings_draw_a_scrollbar() {
    let (buf, area) = render_settings(8);
    let x = area.width.saturating_sub(1);
    let glyphs = (0..area.height)
        .map(|y| buf[(x, y)].symbol())
        .collect::<String>();

    assert!(glyphs.contains('░') || glyphs.contains('█'));
}

#[test]
fn page_down_moves_the_settings_content() {
    let mut model = UiModel::detached();
    model.tab = Tab::Settings;
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    sync_views(&mut tree, &model);
    let area = Rect::new(0, 0, 80, 8);
    let mut before = Buffer::empty(area);
    tree.render(area, &mut before, &theme);

    assert!(tree.focus("settings"));
    let key = InputEvent::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::PageDown,
        crossterm::event::KeyModifiers::NONE,
    ));
    tree.handle_event(&key);

    let mut after = Buffer::empty(area);
    tree.render(area, &mut after, &theme);
    let before_text = (0..area.height)
        .map(|y| row_text(&before, area, y))
        .collect::<String>();
    let after_text = (0..area.height)
        .map(|y| row_text(&after, area, y))
        .collect::<String>();

    assert_ne!(before_text, after_text);
}
