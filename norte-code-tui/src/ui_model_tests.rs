//! The context shown is the reported one, with the model window as fallback.

use norte_code_wire::{AgentRef, ModelInfo};

use super::UiModel;
use crate::context::ContextUsage;

fn info(provider: &str, name: &str, max_context: Option<u64>) -> ModelInfo {
    ModelInfo {
        provider: provider.to_owned(),
        model: name.to_owned(),
        name: name.to_owned(),
        description: None,
        max_context,
        max_output: None,
        tools: true,
        input: Vec::new(),
        output: Vec::new(),
        reasoning: norte_code_wire::ReasoningInfo::default(),
        deprecated: false,
    }
}

#[test]
fn the_model_window_fills_in_for_a_fresh_session() {
    let mut model = UiModel::detached();
    model.active_model = Some(AgentRef {
        id: "big".to_owned(),
        version: String::new(),
    });
    model.active_provider = Some("nvidia".to_owned());
    model.models = vec![
        info("openrouter", "big", Some(1_000)),
        info("nvidia", "big", Some(128_000)),
    ];
    let context = model.effective_context();
    assert_eq!(context.window, Some(128_000));
    assert_eq!(context.used, 0);
}

#[test]
fn reported_stats_win_over_the_model_window() {
    let mut model = UiModel::detached();
    model.context = ContextUsage {
        used: 40,
        window: Some(200),
    };
    model.active_model = Some(AgentRef {
        id: "big".to_owned(),
        version: String::new(),
    });
    model.models = vec![info("nvidia", "big", Some(128_000))];
    assert_eq!(model.effective_context().window, Some(200));
}

#[test]
fn an_unknown_model_leaves_the_window_unknown() {
    let mut model = UiModel::detached();
    model.active_model = Some(AgentRef {
        id: "missing".to_owned(),
        version: String::new(),
    });
    model.models = vec![info("nvidia", "big", Some(128_000))];
    assert_eq!(model.effective_context().window, None);
}
