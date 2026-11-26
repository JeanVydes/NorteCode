//! The context shown is the reported one, with the model window as fallback.

use norte_code_client::ServerInfo;
use norte_code_wire::{AgentRef, AgentSummary, ModelInfo};

use super::{ReasoningLevel, SendMode, UiModel};
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
fn selector_labels_round_trip_through_the_enums() {
    for level in [
        ReasoningLevel::Auto,
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
    ] {
        assert_eq!(ReasoningLevel::from_label(level.label()), Some(level));
    }
    for mode in [
        SendMode::Steer,
        SendMode::Queue,
        SendMode::Interrupt,
        SendMode::Reject,
    ] {
        assert_eq!(SendMode::from_label(mode.label()), Some(mode));
    }
    assert_eq!(ReasoningLevel::from_label("unknown"), None);
    assert_eq!(SendMode::from_label("unknown"), None);
}

#[test]
fn thinking_mode_uses_the_session_override_and_pending_draft_setting() {
    let mut model = UiModel::detached();
    model.pending_new_session_reasoning = Some(norte_code_wire::Reasoning::Budget { tokens: 4096 });
    assert_eq!(model.thinking_mode(), "budget 4096");

    model.active = Some(norte_code_wire::SessionKey::new("session-1"));
    model.active_reasoning = Some(norte_code_wire::Reasoning::Effort {
        effort: "xhigh".to_owned(),
    });
    assert_eq!(model.thinking_mode(), "xhigh");

    model.active_reasoning = None;
    assert_eq!(model.thinking_mode(), "auto");
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

#[test]
fn a_sessionless_draft_uses_the_default_agent_model_and_provider() {
    let mut model = UiModel::detached();
    model.server_info = Some(ServerInfo {
        default_agent: "norte-code".to_owned(),
        ..ServerInfo::default()
    });
    model.agents = vec![AgentSummary {
        id: "norte-code".to_owned(),
        provider: Some(AgentRef {
            id: "openrouter".to_owned(),
            version: String::new(),
        }),
        model: Some(AgentRef {
            id: "deepseek-v4".to_owned(),
            version: String::new(),
        }),
        ..AgentSummary::default()
    }];
    model.models = vec![info("openrouter", "deepseek-v4", Some(128_000))];

    assert_eq!(
        model.effective_model_selection(),
        Some(("openrouter", "deepseek-v4"))
    );
    assert_eq!(
        model.active_model_info().map(|model| model.model.as_str()),
        Some("deepseek-v4")
    );
    assert_eq!(model.effective_context().window, Some(128_000));
}
