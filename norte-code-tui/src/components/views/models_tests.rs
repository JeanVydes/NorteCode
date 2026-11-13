use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{ModelRow, ModelsView};
use crate::{
    components::{Component, ComponentAction},
    ui_model::UiModel,
};
use norte_code_wire::{ModelInfo, ReasoningInfo};

#[test]
fn provider_search_keeps_all_of_its_models_and_hides_other_providers() {
    let models = [
        model("OpenAI", "o3", "Reasoning"),
        model("OpenAI", "gpt-4.1", "GPT 4.1"),
        model("Anthropic", "claude-sonnet", "Sonnet"),
    ];
    let view = ModelsView {
        filter: "openai".to_owned(),
        ..ModelsView::default()
    };

    let rows = view.visible_rows(&models);

    assert_eq!(rows.len(), 3);
    assert!(matches!(rows[0], ModelRow::Provider { name: "OpenAI", .. }));
    assert!(matches!(rows[1], ModelRow::Model(model) if model.model == "o3"));
    assert!(matches!(rows[2], ModelRow::Model(model) if model.model == "gpt-4.1"));
}

#[test]
fn model_search_keeps_its_provider_but_only_matching_models() {
    let models = [
        model("OpenAI", "o3", "Reasoning"),
        model("OpenAI", "gpt-4.1", "GPT 4.1"),
        model("Anthropic", "claude-sonnet", "Sonnet"),
    ];
    let view = ModelsView {
        filter: "O3".to_owned(),
        ..ModelsView::default()
    };

    let rows = view.visible_rows(&models);

    assert_eq!(rows.len(), 2);
    assert!(matches!(rows[0], ModelRow::Provider { name: "OpenAI", .. }));
    assert!(matches!(rows[1], ModelRow::Model(model) if model.model == "o3"));
}

#[test]
fn unmatched_search_hides_every_provider() {
    let models = [model("OpenAI", "o3", "Reasoning")];
    let view = ModelsView {
        filter: "missing".to_owned(),
        ..ModelsView::default()
    };

    assert!(view.visible_rows(&models).is_empty());
}

#[test]
fn enter_expands_a_provider_before_selecting_its_models() {
    let model_info = model("OpenAI", "o3", "Reasoning");
    let ui_model = UiModel {
        models: vec![model_info],
        active_session: Some(norte_code_wire::SessionKey::new("session")),
        ..UiModel::default()
    };
    let mut view = ModelsView::default();

    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &ui_model);
    view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &ui_model);
    view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &ui_model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::SetModel { provider, model })
            if provider == "OpenAI" && model == "o3"
    ));
}

fn model(provider: &str, model: &str, name: &str) -> ModelInfo {
    ModelInfo {
        provider: provider.to_owned(),
        model: model.to_owned(),
        name: name.to_owned(),
        description: None,
        max_context: None,
        max_output: None,
        tools: false,
        input: Vec::new(),
        output: Vec::new(),
        reasoning: ReasoningInfo::default(),
        deprecated: false,
    }
}
