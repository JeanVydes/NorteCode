use super::{
    ChatState, ConversationItem, HistoryKind, HistoryRole, MessageDelivery, TodoStatus,
    human_count, todos_from_history,
};

#[test]
fn token_counts_read_in_k_m_b() {
    assert_eq!(human_count(0), "0");
    assert_eq!(human_count(947), "947");
    assert_eq!(human_count(12_345), "12.3k");
    assert_eq!(human_count(1_250_000), "1.2m");
    assert_eq!(human_count(3_400_000_000), "3.4b");
}

#[test]
fn a_parked_question_is_not_a_running_turn() {
    // Nothing is being produced until someone answers, so Stop has
    // nothing to stop, the card offers Cancel instead.
    assert!(!ChatState::AwaitingAnswer.has_running_turn());
    assert_eq!(
        ChatState::AwaitingAnswer.label(),
        "Waiting for your answer…"
    );
}

#[test]
fn a_reconnecting_session_is_not_reported_as_busy() {
    assert!(!ChatState::Reconnecting.is_busy());
    assert_eq!(ChatState::Reconnecting.label(), "Reconnecting…");
}

#[test]
fn every_working_state_says_something() {
    for state in [
        ChatState::AwaitingAnswer,
        ChatState::LoadingHistory,
        ChatState::Sending,
        ChatState::Thinking,
        ChatState::Streaming,
        ChatState::RunningTool,
        ChatState::Compacting,
        ChatState::Reconnecting,
    ] {
        assert!(!state.label().is_empty(), "{state:?}");
    }
}

#[test]
fn restores_the_latest_valid_todo_call() {
    let item = |content: &str| ConversationItem {
        id: "todo".to_owned(),
        turn_id: None,
        role: HistoryRole::Tool,
        kind: HistoryKind::ToolCall,
        content: content.to_owned(),
        created_at: 0,
        delivery: MessageDelivery::Sent,
        tool: None,
        usage: None,
    };
    let items = vec![
        item(r#"todo {"todos":[{"content":"first","status":"pending"}]}"#),
        item(r#"todo {"todos":[{"content":"current","status":"in_progress"}]}"#),
    ];

    assert_eq!(
        todos_from_history(&items).expect("todo snapshot"),
        [super::SessionTodo {
            content: "current".to_owned(),
            status: TodoStatus::InProgress,
        }]
    );
}
