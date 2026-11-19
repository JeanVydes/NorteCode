use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::HostView;
use crate::{
    components::{Component, ComponentAction, McpState, ViewAction},
    ui_model::UiModel,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn model_with_mcp() -> UiModel {
    UiModel {
        mcp: Some(norte_code_client::McpCatalog {
            servers: vec![norte_code_wire::McpInfo {
                name: "filesystem".to_owned(),
                command: "mcp-filesystem".to_owned(),
                tools: vec!["read".to_owned()],
                filtered: Vec::new(),
                active: true,
            }],
            failures: Vec::new(),
        }),
        ..UiModel::default()
    }
}

#[test]
fn toggles_the_selected_mcp_server() {
    let mut view = HostView::default();
    let model = model_with_mcp();

    for code in [KeyCode::Char(']'), KeyCode::Char(']'), KeyCode::Enter] {
        view.handle_key(key(code), &model);
    }

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::View(ViewAction::SetMcp {
            name,
            state: McpState::Inactive,
        })) if name == "filesystem"
    ));
}

#[test]
fn tab_does_not_move_between_sections() {
    let mut view = HostView::default();
    let model = model_with_mcp();

    view.handle_key(key(KeyCode::Tab), &model);
    view.handle_key(key(KeyCode::Enter), &model);

    assert!(view.take_action().is_none());
}

#[test]
fn the_bracket_keys_walk_the_sections() {
    let mut view = HostView::default();
    let model = UiModel::default();

    view.handle_key(key(KeyCode::Char(']')), &model);
    view.handle_key(key(KeyCode::Char('[')), &model);

    assert!(view.take_action().is_none());
}

#[test]
fn clicking_a_section_tab_opens_it() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut view = HostView::default();
    let model = model_with_mcp();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), &model))
        .expect("draw");

    // The MCP tab is the third label on the first row.
    view.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 14,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
        &model,
    );
    view.handle_key(key(KeyCode::Enter), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::View(ViewAction::SetMcp { .. }))
    ));
}
