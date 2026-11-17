//! The command menu and the values its choices offer.

use crate::components::CommandMenuAction;
use crate::components::command_menu::{budget_values, queue_values, reasoning_values};

use crate::ui_model::{ReasoningLevel, SendMode};

#[test]
fn the_queue_choice_offers_every_mode() {
    let values = queue_values(SendMode::Steer);

    assert_eq!(values.len(), 4);
    assert_eq!(
        values[0].action,
        CommandMenuAction::SetQueueMode(SendMode::Steer)
    );
    assert_eq!(
        values[3].action,
        CommandMenuAction::SetQueueMode(SendMode::Reject)
    );
    assert!(values[0].entry.current);
    assert!(!values[1].entry.current);
}

#[test]
fn the_reasoning_choice_offers_every_level() {
    let values = reasoning_values(ReasoningLevel::Automatic);

    assert_eq!(values.len(), 5);
    assert_eq!(
        values[1].action,
        CommandMenuAction::SetReasoning(ReasoningLevel::Disabled)
    );
    assert_eq!(
        values[4].action,
        CommandMenuAction::SetReasoning(ReasoningLevel::High)
    );
    assert!(values[0].entry.current);
}

#[test]
fn the_budget_choice_offers_every_action() {
    let values = budget_values(norte_code_wire::BudgetAction::Ask);

    assert_eq!(values.len(), 3);
    assert_eq!(
        values[1].action,
        CommandMenuAction::SetBudgetAction(norte_code_wire::BudgetAction::Ask)
    );
    assert!(values[1].entry.current);
}
