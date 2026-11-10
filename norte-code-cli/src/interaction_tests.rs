use super::{parse_budget_limit, parse_chat_command};

#[test]
fn budget_limit_rejects_non_positive_values() {
    assert!(parse_budget_limit(super::BudgetUnit::Tokens, "0").is_err());
}

#[test]
fn unknown_chat_text_remains_a_message() {
    assert!(parse_chat_command("hello").is_none());
}
