use norte_code_wire::{Delta, ServerEventKind};

use super::TurnState;

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
