use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::HostView;
use crate::{
    components::{Component, ComponentAction, McpState, ViewAction},
    ui_model::UiModel,
};

#[test]
fn toggles_the_selected_mcp_server() {
    let mut view = HostView::default();
    let model = UiModel {
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
    };

    for code in [KeyCode::Tab, KeyCode::Tab, KeyCode::Enter] {
        view.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &model);
    }

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::View(ViewAction::SetMcp {
            name,
            state: McpState::Inactive,
        })) if name == "filesystem"
    ));
}
