//! What the provider and model picker shows under a filter.

use norte_code_wire::ModelInfo;

use super::ModelsView;

fn model(provider: &str, id: &str) -> ModelInfo {
    ModelInfo {
        provider: provider.to_owned(),
        model: id.to_owned(),
        name: id.to_owned(),
        description: None,
        max_context: None,
        max_output: None,
        tools: true,
        input: Vec::new(),
        output: Vec::new(),
        reasoning: norte_code_wire::ReasoningInfo::default(),
        deprecated: false,
    }
}

fn catalog() -> Vec<ModelInfo> {
    vec![
        model("openai", "gpt-4o"),
        model("openai", "gpt-6"),
        model("openai", "o3"),
        model("anthropic", "claude-sonnet"),
        model("anthropic", "gpt-clone"),
    ]
}

#[test]
fn a_matching_model_does_not_keep_its_whole_provider() {
    let mut view = ModelsView::default();
    view.filter = "gpt-6".to_owned();

    let rows = view.visible_rows_for(&catalog());

    assert!(rows.iter().any(|row| row.contains("openai")));
    assert!(rows.iter().any(|row| row.contains("gpt-6")));
    assert!(
        !rows.iter().any(|row| row.contains("gpt-4o")),
        "a sibling model that does not match must not be listed"
    );
    assert!(
        !rows.iter().any(|row| row.contains("claude-sonnet")),
        "a provider with no match must not be listed"
    );
}

#[test]
fn a_matching_provider_shows_all_of_its_models() {
    let mut view = ModelsView::default();
    view.filter = "anthropic".to_owned();

    let rows = view.visible_rows_for(&catalog());

    assert!(rows.iter().any(|row| row.contains("claude-sonnet")));
    assert!(rows.iter().any(|row| row.contains("gpt-clone")));
    assert!(!rows.iter().any(|row| row.contains("openai")));
}

#[test]
fn an_empty_filter_shows_every_provider_and_no_model() {
    let view = ModelsView::default();

    let rows = view.visible_rows_for(&catalog());

    assert_eq!(rows.len(), 2);
}

#[test]
fn a_filter_that_matches_nothing_shows_nothing() {
    let mut view = ModelsView::default();
    view.filter = "zzz".to_owned();

    assert!(view.visible_rows_for(&catalog()).is_empty());
}
