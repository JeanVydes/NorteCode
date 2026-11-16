use norte_code_wire::QueueMode;

use super::{ComponentAction, HeaderModal};
use crate::ui_model::ReasoningLevel;

fn accepts_all() -> norte_code_wire::ReasoningInfo {
    norte_code_wire::ReasoningInfo {
        supported: true,
        can_disable: true,
        efforts: vec!["low".to_owned(), "medium".to_owned(), "high".to_owned()],
        supports_budget: false,
        max_budget: None,
    }
}

#[test]
fn the_queue_menu_offers_the_three_modes() {
    let selected = HeaderModal::Queue.options();
    assert_eq!(selected, &["reject", "queue", "interrupt"]);
    assert!(matches!(
        HeaderModal::Queue.action(0, selected),
        Some(ComponentAction::SetQueueMode(QueueMode::Reject))
    ));
    assert!(matches!(
        HeaderModal::Queue.action(2, selected),
        Some(ComponentAction::SetQueueMode(QueueMode::Interrupt))
    ));
    assert!(HeaderModal::Queue.action(3, selected).is_none());
}

#[test]
fn the_reasoning_menu_covers_every_level() {
    let selected = HeaderModal::Reasoning.options();
    assert_eq!(selected, &["auto", "off", "low", "medium", "high"]);
    assert!(matches!(
        HeaderModal::Reasoning.action(0, selected),
        Some(ComponentAction::SetReasoning(ReasoningLevel::Automatic))
    ));
    assert!(matches!(
        HeaderModal::Reasoning.action(4, selected),
        Some(ComponentAction::SetReasoning(ReasoningLevel::High))
    ));
    assert!(HeaderModal::Reasoning.action(5, selected).is_none());
}

#[test]
fn the_reasoning_menu_offers_only_what_the_model_accepts() {
    let mut info = accepts_all();
    info.efforts = vec!["high".to_owned()];
    let selected = HeaderModal::Reasoning.options_for(Some(&info));
    assert_eq!(selected, ["auto", "off", "high"]);
    assert!(matches!(
        HeaderModal::Reasoning.action(2, &selected),
        Some(ComponentAction::SetReasoning(ReasoningLevel::High))
    ));
    assert!(matches!(
        HeaderModal::Reasoning.action(0, &selected),
        Some(ComponentAction::SetReasoning(ReasoningLevel::Automatic))
    ));
    assert!(matches!(
        HeaderModal::Reasoning.action(1, &selected),
        Some(ComponentAction::SetReasoning(ReasoningLevel::Disabled))
    ));
    assert!(HeaderModal::Reasoning.action(3, &selected).is_none());
}

#[test]
fn the_reasoning_menu_hides_off_when_the_model_cannot_disable() {
    let mut info = accepts_all();
    info.can_disable = false;
    let selected = HeaderModal::Reasoning.options_for(Some(&info));
    assert_eq!(selected, ["auto", "low", "medium", "high"]);
}

#[test]
fn the_reasoning_menu_offers_nothing_but_auto_when_reasoning_is_unsupported() {
    let unsupported = norte_code_wire::ReasoningInfo {
        supported: false,
        can_disable: true,
        efforts: Vec::new(),
        supports_budget: false,
        max_budget: None,
    };
    let selected = HeaderModal::Reasoning.options_for(Some(&unsupported));
    assert_eq!(selected, ["auto", "off"]);
}

#[test]
fn the_menus_carry_a_title() {
    use super::HeaderModal::*;
    assert_eq!(Queue.title(), "queue mode");
    assert_eq!(Reasoning.title(), "reasoning effort");
}
