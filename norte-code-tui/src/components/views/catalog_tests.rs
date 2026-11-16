use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use norte_code_wire::catalog::WireSchema;

use super::CatalogView;
use crate::{
    components::{CatalogOperation, Component, ComponentAction, ViewAction},
    ui_model::UiModel,
};

fn press(view: &mut CatalogView, key: KeyCode, model: &UiModel) {
    view.handle_key(KeyEvent::new(key, KeyModifiers::NONE), model);
}

fn type_text(view: &mut CatalogView, text: &str, model: &UiModel) {
    for character in text.chars() {
        press(view, KeyCode::Char(character), model);
    }
}

#[test]
fn creates_provider_using_shared_catalogue_types() {
    let mut view = CatalogView::default();
    let model = UiModel::default();

    press(&mut view, KeyCode::Char('a'), &model);
    type_text(&mut view, "openai", &model);
    press(&mut view, KeyCode::Enter, &model);
    type_text(&mut view, "OpenAI", &model);
    press(&mut view, KeyCode::Enter, &model);
    type_text(&mut view, "https://api.openai.com/v1", &model);
    press(&mut view, KeyCode::Enter, &model);
    type_text(&mut view, "OPENAI_API_KEY", &model);
    press(&mut view, KeyCode::Enter, &model);

    let Some(ComponentAction::View(ViewAction::Catalog(operation))) = view.take_action() else {
        panic!("provider form did not submit");
    };
    match *operation {
        CatalogOperation::UpsertProvider { id, provider } => {
            assert_eq!(id, "openai");
            assert_eq!(provider.name, "OpenAI");
            assert_eq!(provider.wires[0].schema, WireSchema::OpenAI);
            assert_eq!(provider.environment_keys.len(), 1);
            assert_eq!(provider.environment_keys[0], "OPENAI_API_KEY");
        }
        _ => panic!("provider form submitted the wrong operation"),
    }
}

#[test]
fn plugin_provider_entries_are_not_removable() {
    let model = UiModel {
        catalog: norte_code_client::CatalogDetail {
            providers: vec![norte_code_wire::CatalogProviderInfo {
                provider_id: "plugin-provider".to_owned(),
                name: "Plugin provider".to_owned(),
                api: None,
                from_file: false,
                model_count: 0,
                environment_keys: vec![],
            }],
            ..norte_code_client::CatalogDetail::default()
        },
        ..UiModel::default()
    };
    let mut view = CatalogView::default();

    press(&mut view, KeyCode::Char('d'), &model);

    assert!(view.take_action().is_none());
    assert_eq!(
        view.notice.as_deref(),
        Some("Plugin catalogue entries are read-only.")
    );
}

#[test]
fn deletion_requires_confirmation() {
    let model = UiModel {
        catalog: norte_code_client::CatalogDetail {
            providers: vec![norte_code_wire::CatalogProviderInfo {
                provider_id: "openai".to_owned(),
                name: "OpenAI".to_owned(),
                api: None,
                from_file: true,
                model_count: 0,
                environment_keys: vec![],
            }],
            ..norte_code_client::CatalogDetail::default()
        },
        ..UiModel::default()
    };
    let mut view = CatalogView::default();

    press(&mut view, KeyCode::Char('d'), &model);
    assert!(view.take_action().is_none());
    press(&mut view, KeyCode::Enter, &model);

    let Some(ComponentAction::View(ViewAction::Catalog(operation))) = view.take_action() else {
        panic!("delete confirmation did not submit");
    };
    assert!(matches!(
        *operation,
        CatalogOperation::RemoveProvider(provider) if provider == "openai"
    ));
}
