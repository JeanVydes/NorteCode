use norte_code_wire::{
    Delta, HistoryItem, HistorySpeaker, OutputStream, ServerEvent, ServerEventKind, ToolStatus,
};

use super::{TranscriptNode, TranscriptNodeKind, TranscriptTree};
use crate::ui_model::TranscriptItem;

fn event(kind: ServerEventKind) -> TranscriptItem {
    TranscriptItem::ServerEvent(Box::new(ServerEvent {
        request_id: 1,
        session: None,
        kind,
    }))
}

fn text_delta(turn: u64, text: &str) -> TranscriptItem {
    event(ServerEventKind::Delta {
        turn,
        delta: Delta::Text {
            index: 0,
            text: text.to_owned(),
        },
    })
}

fn reasoning_delta(turn: u64, text: &str) -> TranscriptItem {
    event(ServerEventKind::Delta {
        turn,
        delta: Delta::Reasoning {
            index: 0,
            text: text.to_owned(),
        },
    })
}

fn tool_started(turn: u64, call_id: &str, name: &str) -> TranscriptItem {
    event(ServerEventKind::ToolStarted {
        turn,
        call_id: call_id.to_owned(),
        tool: name.to_owned(),
        arguments: None,
        source: None,
        origin: None,
    })
}

fn tool_output(turn: u64, call_id: &str, text: &str) -> TranscriptItem {
    event(ServerEventKind::ToolOutput {
        turn,
        call_id: call_id.to_owned(),
        tool: "shell".to_owned(),
        stream: OutputStream::Stdout,
        text: text.to_owned(),
        last: false,
    })
}

fn tool_finished(turn: u64, call_id: &str, output: Option<&str>) -> TranscriptItem {
    event(ServerEventKind::ToolFinished {
        turn,
        call_id: call_id.to_owned(),
        tool: "shell".to_owned(),
        is_error: false,
        error: None,
        output: output.map(str::to_owned),
        truncated: false,
        detached: false,
        duration_ms: 12,
    })
}

fn history(turn_id: &str, role: HistorySpeaker, kind: &str, content: &str) -> TranscriptItem {
    TranscriptItem::History(HistoryItem {
        id: format!("{turn_id}-{kind}"),
        kind: kind.to_owned(),
        content: content.to_owned(),
        created_at: 1,
        turn_id: Some(turn_id.to_owned()),
        role,
        provenance: None,
    })
}

fn kinds(tree: &[TranscriptNode]) -> Vec<String> {
    tree.iter()
        .map(|node| match node.kind() {
            TranscriptNodeKind::Turn { .. } => "turn".to_owned(),
            TranscriptNodeKind::Message { .. } => "message".to_owned(),
            TranscriptNodeKind::Tool(_) => "tool".to_owned(),
            TranscriptNodeKind::Reasoning { .. } => "reasoning".to_owned(),
            TranscriptNodeKind::Error { .. } => "error".to_owned(),
            _ => "other".to_owned(),
        })
        .collect()
}

fn children_kinds(tree: &[TranscriptNode]) -> Vec<String> {
    tree.first()
        .map(|turn| {
            turn.children()
                .iter()
                .map(|child| match child.kind() {
                    TranscriptNodeKind::Message { role, .. } => format!("message:{role:?}"),
                    TranscriptNodeKind::Tool(_) => "tool".to_owned(),
                    TranscriptNodeKind::Reasoning { .. } => "reasoning".to_owned(),
                    _ => "other".to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_turn_holds_its_reasoning_its_message_and_its_tool() {
    let items = [
        reasoning_delta(1, "thinking"),
        text_delta(1, "hello"),
        tool_started(1, "call-1", "shell"),
        tool_finished(1, "call-1", Some("done")),
    ];

    let tree = TranscriptTree::build(&items);

    assert_eq!(kinds(&tree), vec!["turn"]);
    assert_eq!(
        children_kinds(&tree),
        vec!["reasoning", "message:Assistant", "tool"]
    );
}

#[test]
fn two_calls_of_a_turn_are_two_nodes() {
    let items = [
        tool_started(1, "call-1", "shell"),
        tool_started(1, "call-2", "shell"),
        tool_finished(1, "call-2", Some("second")),
    ];

    let tree = TranscriptTree::build(&items);

    assert_eq!(children_kinds(&tree), vec!["tool", "tool"]);
}

#[test]
fn the_chunks_of_a_call_land_in_its_own_tool() {
    let items = [
        tool_started(1, "call-1", "shell"),
        tool_output(1, "call-1", "one\n"),
        tool_started(1, "call-2", "shell"),
        tool_output(1, "call-2", "two\n"),
    ];

    let tree = TranscriptTree::build(&items);
    let tools = tree
        .first()
        .map(|turn| {
            turn.children()
                .iter()
                .filter_map(|child| match child.kind() {
                    TranscriptNodeKind::Tool(tool) => Some(tool.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    assert_eq!(tools.len(), 2);
    assert_eq!(
        tools[0].chunks,
        vec![(OutputStream::Stdout, "one".to_owned())]
    );
    assert_eq!(
        tools[1].chunks,
        vec![(OutputStream::Stdout, "two".to_owned())]
    );
}

#[test]
fn the_history_of_a_turn_and_its_live_events_land_in_the_same_turn() {
    let items = [
        history("turn_1", HistorySpeaker::User, "user", "hola"),
        text_delta(1, "buenos dias"),
    ];

    let tree = TranscriptTree::build(&items);

    assert_eq!(kinds(&tree), vec!["turn"]);
    assert_eq!(
        children_kinds(&tree),
        vec!["message:User", "message:Assistant"]
    );
}

#[test]
fn the_message_the_reader_sent_waits_for_the_turn_it_belongs_to() {
    let items = [
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "hola".to_owned(),
            chips: Vec::new(),
        },
        text_delta(1, "buenos dias"),
    ];

    let tree = TranscriptTree::build(&items);

    assert_eq!(kinds(&tree), vec!["turn"]);
    assert_eq!(
        children_kinds(&tree),
        vec!["message:User", "message:Assistant"]
    );
}

#[test]
fn a_message_with_no_turn_of_its_own_still_has_a_home() {
    let items = [TranscriptItem::UserMessage {
        queued: false,
        turn_id: None,
        text: "hola".to_owned(),
        chips: Vec::new(),
    }];

    let tree = TranscriptTree::build(&items);

    assert_eq!(kinds(&tree), vec!["turn"]);
    assert_eq!(children_kinds(&tree), vec!["message:User"]);
}

#[test]
fn a_finished_call_carries_its_status_and_its_result() {
    let items = [
        tool_started(1, "call-1", "shell"),
        tool_finished(1, "call-1", Some("the result")),
    ];

    let tree = TranscriptTree::build(&items);
    let tool = tree
        .first()
        .and_then(|turn| turn.children().first())
        .and_then(|child| match child.kind() {
            TranscriptNodeKind::Tool(tool) => Some(tool),
            _ => None,
        });

    let tool = tool.expect("the tool node");
    assert_eq!(tool.status, Some(ToolStatus::Ok));
    assert_eq!(tool.duration_ms, Some(12));
    assert_eq!(tool.output.as_deref(), Some("the result"));
}

#[test]
fn a_history_turn_and_a_live_turn_with_the_same_number_become_one() {
    let items = [
        history("turn_1", HistorySpeaker::User, "user", "hola"),
        history(
            "turn_1",
            HistorySpeaker::Assistant,
            "assistant",
            "hola a ti",
        ),
        text_delta(1, "sigo respondiendo"),
    ];

    let tree = TranscriptTree::build(&items);

    assert_eq!(kinds(&tree), vec!["turn"]);
    assert_eq!(
        children_kinds(&tree),
        vec!["message:User", "message:Assistant", "message:Assistant"]
    );
}

#[test]
fn a_user_message_between_two_parts_of_one_turn_does_not_split_it() {
    let items = [
        history("turn_1", HistorySpeaker::User, "user", "hola"),
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "segunda".to_owned(),
            chips: Vec::new(),
        },
        text_delta(1, "sigo respondiendo"),
    ];

    let tree = TranscriptTree::build(&items);

    assert_eq!(
        tree.iter()
            .filter(|node| matches!(node.kind(), TranscriptNodeKind::Turn { key } if *key == crate::components::views::session::transcript::TurnKey::Live(1)))
            .count(),
        1,
        "one turn, not two"
    );
}
