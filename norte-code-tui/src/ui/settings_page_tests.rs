//! Settings keeps its section order and scrolls when it overflows.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use norte_code_client::PluginSummary;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::{InputEvent, Theme, Tree};

use super::{Focus, SettingsPage};
use crate::theme::{ThemeChoice, ThemeKind};
use crate::ui::{build_tree, refresh_views, sync_views};
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
    let x = area.width.saturating_sub(2);
    let glyphs = (0..area.height)
        .map(|y| buf[(x, y)].symbol())
        .collect::<String>();

    assert!(glyphs.contains('░') || glyphs.contains('█') || glyphs.contains('│'));
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

fn settings_tree() -> (Tree, UiModel, Theme) {
    let mut model = UiModel::detached();
    model.tab = Tab::Settings;
    model.preferences.theme = ThemeChoice::BuiltIn(ThemeKind::ByeRetina);
    model.plugins = (0..3)
        .map(|index| PluginSummary {
            name: format!("plugin-{index}"),
            enabled: true,
            loaded: true,
            ..PluginSummary::default()
        })
        .collect();
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    sync_views(&mut tree, &model);
    refresh_views(&mut tree, &model);
    assert!(tree.focus("settings"));
    (tree, model, theme)
}

#[test]
fn clicks_select_plugins_and_preferences() {
    let (mut tree, _, theme) = settings_tree();
    let area = Rect::new(0, 0, 80, 40);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);

    let (plugin_row, preference_row) = {
        let page = tree
            .find::<SettingsPage>("settings")
            .expect("settings page");
        let layout = page.layout();
        (
            page.area.y + layout.plugin_rows_y() + 2,
            page.area.y + layout.preference_rows_y() + 4,
        )
    };
    let click = |column, row| {
        InputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        })
    };

    tree.handle_event(&click(4, plugin_row));
    let page = tree
        .find::<SettingsPage>("settings")
        .expect("settings page");
    assert_eq!(page.plugin_selected, 2);
    assert_eq!(page.focus, Focus::Plugins);

    tree.handle_event(&click(4, preference_row));
    let page = tree
        .find::<SettingsPage>("settings")
        .expect("settings page");
    assert_eq!(page.pref_selected, 4);
    assert_eq!(page.focus, Focus::Preferences);
}

#[test]
fn settings_fill_unpainted_cells_with_the_selected_theme() {
    let (mut tree, _, theme) = settings_tree();
    let area = Rect::new(0, 0, 80, 40);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let page = tree
        .find::<SettingsPage>("settings")
        .expect("settings page");
    let layout = page.layout();

    let preference_gap = buf[(
        page.area.x + layout.value_x() - 1,
        page.area.y + layout.preference_rows_y() + 1,
    )]
        .bg;
    let plugin_padding = buf[(page.area.x + 70, page.area.y + layout.plugin_rows_y() + 1)].bg;

    assert_eq!(preference_gap, theme.background);
    assert_eq!(plugin_padding, theme.background);
}

#[test]
fn clear_data_requires_confirmation_and_is_listed_last() {
    let (mut tree, _, theme) = settings_tree();
    let area = Rect::new(0, 0, 80, 80);
    let mut buffer = Buffer::empty(area);
    tree.render(area, &mut buffer, &theme);
    let text = (0..area.height)
        .map(|y| row_text(&buffer, area, y))
        .collect::<String>();
    assert!(text.contains("danger zone"));
    assert!(text.contains("clear data"));

    let key = |code| {
        InputEvent::Key(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        ))
    };
    for _ in 0..super::super::config_page::Setting::ALL.len() {
        tree.handle_event(&key(crossterm::event::KeyCode::Down));
    }
    tree.handle_event(&key(crossterm::event::KeyCode::Down));
    tree.handle_event(&key(crossterm::event::KeyCode::Enter));
    assert!(
        tree.find_mut::<SettingsPage>("settings")
            .expect("settings page")
            .take_clear_data_requested()
    );
}

#[test]
fn clicking_host_data_does_not_select_a_virtual_table_row() {
    let (mut tree, _, theme) = settings_tree();
    let area = Rect::new(0, 0, 100, 40);
    tree.render(area, &mut Buffer::empty(area), &theme);
    let page = tree.find::<SettingsPage>("settings").unwrap();
    let host_y = page.area.y + page.layout().section(super::SettingsSection::Host).y + 5;
    let event = InputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: page.area.x + 4,
        row: host_y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    tree.handle_event(&event);
    assert_eq!(
        tree.find::<super::ReadOnlyTable>("host_table")
            .unwrap()
            .0
            .state
            .selected,
        0
    );
    assert!(!tree.focusable_ids().contains(&"host_table"));
}

#[test]
fn plugin_marker_shows_check_when_enabled_and_x_when_disabled() {
    let mut model = UiModel::detached();
    model.tab = Tab::Settings;
    model.plugins = vec![
        PluginSummary {
            name: "on-plugin".to_owned(),
            enabled: true,
            loaded: true,
            ..PluginSummary::default()
        },
        PluginSummary {
            name: "off-plugin".to_owned(),
            enabled: false,
            loaded: true,
            ..PluginSummary::default()
        },
    ];
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    sync_views(&mut tree, &model);
    refresh_views(&mut tree, &model);
    let area = Rect::new(0, 0, 80, 40);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let text = (0..area.height)
        .map(|y| row_text(&buf, area, y))
        .collect::<String>();
    assert!(text.contains("✓ on-plugin"), "{text}");
    assert!(text.contains("x off-plugin"), "{text}");
}
