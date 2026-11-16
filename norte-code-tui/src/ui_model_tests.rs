use norte_code_wire::{Delta, ReasoningInfo, ServerEventKind};

use super::{ReasoningLevel, ReasoningRecovery, TurnState};

#[test]
fn connecting_turn_enters_thinking_on_reasoning_delta() {
    let mut state = TurnState::Connecting;
    state.apply_event(&ServerEventKind::Delta {
        turn: 1,
        delta: Delta::Reasoning {
            index: 0,
            text: "Let me think".to_owned(),
        },
    });

    assert_eq!(state, TurnState::Thinking);
}

#[test]
fn thinking_turn_enters_streaming_on_text_delta_and_finishes_to_idle() {
    let mut state = TurnState::Thinking;
    state.apply_event(&ServerEventKind::Delta {
        turn: 1,
        delta: Delta::Text {
            index: 0,
            text: "Answer".to_owned(),
        },
    });
    assert_eq!(state, TurnState::Streaming);

    state.apply_event(&ServerEventKind::TurnFinished {
        turn: 1,
        phase: norte_code_wire::Phase::Idle,
        committed: true,
        output_schema_valid: None,
    });
    assert_eq!(state, TurnState::Idle);
}

#[test]
fn marks_a_turn_as_waiting_while_a_question_needs_an_answer() {
    let mut state = TurnState::Streaming;
    state.apply_event(&ServerEventKind::QuestionAsked {
        turn: 1,
        question: "question-1".to_owned(),
        prompt: "Choose an option".to_owned(),
        options: vec!["yes".to_owned()],
        multiple: false,
    });
    assert_eq!(state, TurnState::AwaitingAnswer);
    assert!(state.is_busy());

    state.apply_event(&ServerEventKind::QuestionAnswered {
        question: "question-1".to_owned(),
    });
    assert_eq!(state, TurnState::Thinking);
}

#[test]
fn automatic_is_always_a_supported_reasoning_choice() {
    let capabilities = ReasoningInfo::default();

    assert_eq!(
        ReasoningLevel::Automatic.incompatible_with(&capabilities),
        None
    );
}

#[test]
fn an_effort_named_by_the_model_is_allowed() {
    let capabilities = ReasoningInfo {
        supported: true,
        efforts: vec!["medium".to_owned(), "high".to_owned()],
        ..ReasoningInfo::default()
    };

    assert_eq!(ReasoningLevel::High.incompatible_with(&capabilities), None);
}

#[test]
fn an_effort_the_model_does_not_name_is_refused() {
    let capabilities = ReasoningInfo {
        supported: true,
        efforts: vec!["medium".to_owned(), "high".to_owned()],
        ..ReasoningInfo::default()
    };

    let message = ReasoningLevel::Low
        .incompatible_with(&capabilities)
        .expect("unsupported effort");

    assert!(message.contains("does not support effort `low`"));
    assert!(message.contains("medium, high"));
}

#[test]
fn a_refusal_is_lowered_at_most_once_per_effort_and_generation() {
    let mut recovery = ReasoningRecovery::default();

    assert!(recovery.claim(7, ReasoningLevel::Low));
    assert!(!recovery.claim(7, ReasoningLevel::Low));
}

#[test]
fn a_different_generation_or_effort_may_clear_once() {
    let mut recovery = ReasoningRecovery::default();

    assert!(recovery.claim(7, ReasoningLevel::Low));
    assert!(recovery.claim(8, ReasoningLevel::Low));
    assert!(recovery.claim(8, ReasoningLevel::Medium));
}

#[test]
fn a_clear_accepts_a_new_refusal() {
    let mut recovery = ReasoningRecovery::default();

    assert!(recovery.claim(7, ReasoningLevel::Low));
    recovery.clear();
    assert!(recovery.claim(7, ReasoningLevel::Low));
}

#[test]
fn a_host_reasoning_setting_restores_the_local_level() {
    assert_eq!(
        ReasoningLevel::from_wire(&norte_code_wire::Reasoning::Effort {
            effort: "high".to_owned(),
        }),
        ReasoningLevel::High
    );
    assert_eq!(
        ReasoningLevel::from_wire(&norte_code_wire::Reasoning::Budget { tokens: 128 }),
        ReasoningLevel::Automatic
    );
}
