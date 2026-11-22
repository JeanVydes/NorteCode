//! The plugins page: every action is on screen, and a build shows its steps.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use norte_code_client::PluginSummary;
use ratatui::{Terminal, backend::TestBackend};

use super::PluginsView;
use crate::components::{Component, ComponentAction, ViewAction};
use crate::ui_model::{PluginJob, UiModel};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn model_with_plugins() -> UiModel {
    UiModel {
        plugins: vec![
            PluginSummary {
                name: "alpha".to_owned(),
                enabled: true,
                loaded: true,
                load_error: None,
                source: Some("https://example.invalid/alpha".to_owned()),
                package: None,
                status: None,
                banner: None,
                tools: Vec::new(),
            },
            PluginSummary {
                name: "beta".to_owned(),
                enabled: true,
                loaded: false,
                load_error: Some("was built against plugin interface 1".to_owned()),
                source: None,
                package: Some("beta".to_owned()),
                status: None,
                banner: None,
                tools: Vec::new(),
            },
        ],
        ..UiModel::default()
    }
}

fn drawn(view: &mut PluginsView, model: &UiModel) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).expect("terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), model))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..24 {
        for x in 0..100 {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[test]
fn the_toolbar_names_every_action() {
    let mut view = PluginsView::default();
    let model = model_with_plugins();

    let text = drawn(&mut view, &model);

    assert!(text.contains("install"), "drawn:\n{text}");
    assert!(text.contains("update"), "drawn:\n{text}");
    assert!(text.contains("update all"), "drawn:\n{text}");
    assert!(text.contains("remove"), "drawn:\n{text}");
}

#[test]
fn clicking_install_opens_the_repository_prompt() {
    let mut view = PluginsView::default();
    let model = model_with_plugins();
    let _ = drawn(&mut view, &model);

    view.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
        &model,
    );

    let text = drawn(&mut view, &model);
    assert!(text.contains("Repository URL"), "drawn:\n{text}");
}

#[test]
fn clicking_remove_asks_to_confirm() {
    let mut view = PluginsView::default();
    let model = model_with_plugins();
    let _ = drawn(&mut view, &model);

    // The row is " install  update  update all  remove ", so the remove
    // button starts right after the three before it.
    let row = drawn(&mut view, &model);
    let line = row.lines().next().expect("a toolbar");
    let column = u16::try_from(line.find("remove").expect("a remove button")).expect("column");
    view.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
        &model,
    );

    let text = drawn(&mut view, &model);
    assert!(text.contains("Remove"), "drawn:\n{text}");
}

#[test]
fn enter_toggles_the_selected_plugin() {
    let mut view = PluginsView::default();
    let model = model_with_plugins();

    view.handle_key(key(KeyCode::Enter), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::View(ViewAction::Plugin(_)))
    ));
}

#[test]
fn the_progress_bar_names_the_step_and_the_stage() {
    let mut view = PluginsView::default();
    let mut model = model_with_plugins();
    model.plugin_job = Some(PluginJob {
        operation: "install".to_owned(),
        step: 3,
        total: 4,
        stage: "building".to_owned(),
    });

    let text = drawn(&mut view, &model);

    assert!(text.contains("install"), "drawn:\n{text}");
    assert!(text.contains("3/4"), "drawn:\n{text}");
    assert!(text.contains("building"), "drawn:\n{text}");
}

#[test]
fn no_progress_bar_without_a_job() {
    let mut view = PluginsView::default();
    let model = model_with_plugins();

    let text = drawn(&mut view, &model);

    assert!(!text.contains("3/4"), "drawn:\n{text}");
}

#[test]
fn a_loaded_plugin_reads_differently_from_one_that_failed() {
    let mut view = PluginsView::default();
    let model = model_with_plugins();

    let text = drawn(&mut view, &model);

    assert!(text.contains("loaded"), "drawn:\n{text}");
    assert!(text.contains("failed"), "drawn:\n{text}");
    assert!(
        text.contains("plugin interface"),
        "the reason is shown:\n{text}"
    );
}

#[test]
fn an_enabled_plugin_the_host_did_not_load_is_not_called_failed() {
    let model = UiModel {
        plugins: vec![PluginSummary {
            name: "gamma".to_owned(),
            enabled: true,
            loaded: false,
            load_error: None,
            source: None,
            package: None,
            status: None,
            banner: None,
            tools: Vec::new(),
        }],
        ..UiModel::default()
    };
    let mut view = PluginsView::default();

    let text = drawn(&mut view, &model);

    assert!(
        text.contains("not loaded in this process"),
        "drawn:\n{text}"
    );
    assert!(!text.contains("failed"), "drawn:\n{text}");
}
