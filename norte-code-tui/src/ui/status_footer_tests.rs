//! The status strip is chat-only chrome.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use terminalui::Text;
use terminalui::components::statusbar::StatusBar;

use crate::theme::{ThemeChoice, ThemeKind};
use crate::ui::{build_tree, sync_theme, sync_views};
use crate::ui_model::{Tab, UiModel};

fn render(tab: Tab) -> (Buffer, Rect) {
    let model = UiModel::detached();
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    let mut model = model;
    model.tab = tab;
    sync_views(&mut tree, &model);
    let area = Rect::new(0, 0, 100, 30);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    (buf, area)
}

fn text(buf: &Buffer, area: Rect) -> String {
    (0..area.height)
        .flat_map(|y| (0..area.width).map(move |x| buf[(x, y)].symbol()))
        .collect()
}

fn focusable_ids(tab: Tab) -> Vec<String> {
    let model = UiModel::detached();
    let mut tree = build_tree(&model);
    let mut model = model;
    model.tab = tab;
    sync_views(&mut tree, &model);
    tree.focusable_ids()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[test]
fn chat_shows_the_reasoning_and_tab_hint() {
    let (buf, area) = render(Tab::Chat);
    let painted = text(&buf, area);
    assert!(painted.contains("reasoning"));
    assert!(painted.contains("tabs"));
    let focusable = focusable_ids(Tab::Chat);
    assert!(focusable.iter().any(|id| id == "reasoning"));
    assert!(focusable.iter().any(|id| id == "queue"));
}

#[test]
fn changing_the_theme_repaints_the_status_strip() {
    let mut model = UiModel::detached();
    model.tab = Tab::Chat;
    let dark = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    model.preferences.theme = ThemeChoice::BuiltIn(ThemeKind::ByeRetina);
    let light = model.preferences.catalog.theme(&model.preferences.theme);
    assert_ne!(dark, light);

    sync_theme(&mut tree, &model);
    sync_views(&mut tree, &model);
    let area = Rect::new(0, 0, 100, 30);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &light);

    let bar = tree.find::<StatusBar>("status").expect("status");
    assert_eq!(bar.style.background, Some(light.secondary));
    assert_eq!(bar.style.separator, Some(light.border));
    let status = bar.state.area;
    let repainted = (status.y..status.y + status.height)
        .any(|y| (status.x..status.x + status.width).any(|x| buf[(x, y)].fg == light.muted));
    assert!(repainted);
}

#[test]
fn syncing_the_theme_keeps_the_status_text_instances() {
    let mut model = UiModel::detached();
    let mut tree = build_tree(&model);
    let (hint, state) = {
        let hint = std::ptr::from_ref(
            tree.find::<super::copy::ShortcutHint>("hint")
                .expect("hint"),
        );
        let state = std::ptr::from_ref(tree.find::<Text>("state").expect("state"));
        (hint, state)
    };
    model.preferences.theme = ThemeChoice::BuiltIn(ThemeKind::ByeRetina);
    sync_theme(&mut tree, &model);
    let hint_after = std::ptr::from_ref(
        tree.find::<super::copy::ShortcutHint>("hint")
            .expect("hint"),
    );
    let state_after = std::ptr::from_ref(tree.find::<Text>("state").expect("state"));
    assert!(std::ptr::eq(hint, hint_after));
    assert!(std::ptr::eq(state, state_after));
}

#[test]
fn other_tabs_hide_the_reasoning_and_tab_hint() {
    for tab in [Tab::Sessions, Tab::Models, Tab::Telemetry, Tab::Settings] {
        let (buf, area) = render(tab);
        let painted = text(&buf, area);
        assert!(!painted.contains("reasoning"));
        assert!(!painted.contains("tabs"));
        let focusable = focusable_ids(tab);
        assert!(!focusable.iter().any(|id| id == "reasoning"));
        assert!(!focusable.iter().any(|id| id == "queue"));
    }
}
