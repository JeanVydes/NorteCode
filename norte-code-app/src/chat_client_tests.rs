use super::*;
use raep_types::FrameType;
use serde_json::Value;

fn frame(kind: FrameType, payload: Value) -> Envelope<Value> {
    let mut envelope = Envelope::new(kind, payload, 1);
    envelope.session_id = "s".to_owned();
    envelope
}

#[test]
fn a_reasoning_delta_lands_on_the_reasoning_channel() {
    let decoded = decode_frame(&frame(
        FrameType::Standard(StandardFrameType::TokenDelta),
        serde_json::json!({ "delta": "thinking…", "index": 0, "channel": "reasoning" }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::Token {
            channel: TokenChannel::Reasoning,
            ..
        }
    ));
}

#[test]
fn a_delta_without_a_channel_is_text() {
    let decoded = decode_frame(&frame(
        FrameType::Standard(StandardFrameType::TokenDelta),
        serde_json::json!({ "delta": "hi", "index": 0 }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::Token {
            channel: TokenChannel::Text,
            ..
        }
    ));
}

#[test]
fn an_accepted_turn_without_a_position_starts_now() {
    let decoded = decode_frame(&frame(
        FrameType::Standard(StandardFrameType::UserInputAccepted),
        serde_json::json!({ "turn_id": "turn-1" }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::Accepted { queued: false, .. }
    ));
}

#[test]
fn an_accepted_turn_with_a_position_is_queued() {
    let decoded = decode_frame(&frame(
        FrameType::Standard(StandardFrameType::UserInputAccepted),
        serde_json::json!({ "turn_id": "turn-1", "position": 1 }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::Accepted { queued: true, .. }
    ));
}

#[test]
fn tool_argument_fragments_are_not_answer_text() {
    let decoded = decode_frame(&frame(
        FrameType::Standard(StandardFrameType::TokenDelta),
        serde_json::json!({ "delta": "{\"path\":", "index": 0, "channel": "tool_arguments" }),
    ));
    assert!(matches!(decoded, DecodedFrame::Ignored));
}

#[test]
fn a_queued_turn_state_is_reported_as_queued() {
    let decoded = decode_frame(&frame(
        FrameType::Standard(StandardFrameType::TurnStateChanged),
        serde_json::json!({ "state": "QUEUED" }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::TurnPhase(TurnPhase::Queued)
    ));
}

#[test]
fn every_terminal_state_ends_the_turn_with_its_outcome() {
    for (state, expected) in [
        ("COMPLETED", TurnOutcome::Completed),
        ("CANCELLED", TurnOutcome::Cancelled),
    ] {
        let decoded = decode_frame(&frame(
            FrameType::Standard(StandardFrameType::TurnStateChanged),
            serde_json::json!({ "state": state }),
        ));
        assert!(
            matches!(decoded, DecodedFrame::TurnEnded { outcome, .. } if outcome == expected),
            "{state}"
        );
    }
    let failed = decode_frame(&frame(
        FrameType::Standard(StandardFrameType::TurnStateChanged),
        serde_json::json!({ "state": "FAILED", "error": { "message": "provider said no" } }),
    ));
    assert!(matches!(
        failed,
        DecodedFrame::TurnEnded {
            outcome: TurnOutcome::Failed(Some(message)),
            ..
        } if message == "provider said no"
    ));
}

#[test]
fn a_compaction_reported_only_by_its_summary_still_counts() {
    let decoded = decode_frame(&frame(
        FrameType::Extension("norte_code.compaction_finished".to_owned()),
        serde_json::json!({ "error": null, "summary": "a handoff" }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::Compacted {
            compacted: true,
            ..
        }
    ));
}

#[test]
fn a_failed_compaction_is_not_reported_as_compacted() {
    let decoded = decode_frame(&frame(
        FrameType::Extension("norte_code.compaction_finished".to_owned()),
        serde_json::json!({ "error": "no model", "summary": null }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::Compacted {
            compacted: false,
            ..
        }
    ));
}

#[test]
fn an_ask_user_question_is_decoded_with_its_choices() {
    let decoded = decode_frame(&frame(
        FrameType::Extension("norte_code.question_asked".to_owned()),
        serde_json::json!({
            "turn_id": 3,
            "question": "q-1",
            "prompt": "Which color?",
            "options": ["Red", "Blue"],
            "multiple": false,
        }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::QuestionAsked {
            turn: 3,
            id,
            options,
            multiple: false,
            ..
        } if id == "q-1" && options == vec!["Red".to_owned(), "Blue".to_owned()]
    ));
}

#[test]
fn an_answered_question_names_the_question_that_closed() {
    // The extension payload spells the id `question`; the canonical form
    // of the same event spells it `approval_id`. Both must decode.
    for payload in [
        serde_json::json!({ "question": "q-1" }),
        serde_json::json!({ "approval_id": "q-1" }),
    ] {
        let decoded = decode_frame(&frame(
            FrameType::Extension("norte_code.question_answered".to_owned()),
            payload,
        ));
        assert!(matches!(
            decoded,
            DecodedFrame::QuestionAnswered { id } if id == "q-1"
        ));
    }
}

#[test]
fn a_compaction_finished_event_reports_the_summary() {
    let decoded = decode_frame(&frame(
        FrameType::Extension("norte_code.compaction_finished".to_owned()),
        serde_json::json!({ "compacted": true, "summary": "we talked about cats" }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::Compacted { compacted: true, summary: Some(summary), .. }
            if summary == "we talked about cats"
    ));
}

#[test]
fn a_child_session_start_is_typed() {
    let decoded = decode_frame(&frame(
        FrameType::Extension("norte_code.child_session_started".to_owned()),
        serde_json::json!({
            "child_session_id": "child-1",
            "agent_id": "sub",
            "task_summary": "read the docs",
        }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::ChildStarted { session_id, .. } if session_id == "child-1"
    ));
}

#[test]
fn a_forked_event_names_the_new_session() {
    let decoded = decode_frame(&frame(
        FrameType::Standard(StandardFrameType::SessionForked),
        serde_json::json!({ "from_session_id": "a", "from_turn_id": "turn_5" }),
    ));
    assert!(matches!(
        decoded,
        DecodedFrame::Forked { from_turn_id: Some(turn), .. } if turn == "turn_5"
    ));
}
