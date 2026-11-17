//! Calls whose events arrive out of order never draw an empty node.

use norte_code_wire::{Delta, OutputStream, ServerEvent, ServerEventKind};

use super::TranscriptTree;
use crate::ui_model::TranscriptItem;

fn event(kind: ServerEventKind) -> TranscriptItem {
    TranscriptItem::ServerEvent(Box::new(ServerEvent {
        request_id: 1,
        session: None,
        kind,
    }))
}

fn names(items: &[TranscriptItem]) -> Vec<String> {
    let mut found = Vec::new();
    for node in &TranscriptTree::build(items) {
        collect(std::slice::from_ref(node), &mut found);
    }
    found
}

fn collect(nodes: &[super::TranscriptNode], found: &mut Vec<String>) {
    for node in nodes {
        if let super::TranscriptNodeKind::Tool(tool) = node.kind() {
            found.push(tool.name.clone());
        }
        collect(node.children(), found);
    }
}

#[test]
fn output_that_arrives_before_its_call_is_kept_for_it() {
    let items = [
        event(ServerEventKind::ToolOutput {
            turn: 1,
            call_id: "call-1".to_owned(),
            stream: OutputStream::Stdout,
            text: "early".to_owned(),
            tool: "read".to_owned(),
            last: false,
        }),
        event(ServerEventKind::ToolStarted {
            turn: 1,
            call_id: "call-1".to_owned(),
            tool: "read".to_owned(),
            arguments: None,
            source: None,
        }),
    ];

    assert_eq!(names(&items), vec!["read".to_owned()]);
}

#[test]
fn two_calls_without_an_id_stay_two_named_nodes() {
    let items = [
        event(ServerEventKind::ToolStarted {
            turn: 1,
            call_id: String::new(),
            tool: "read".to_owned(),
            arguments: None,
            source: None,
        }),
        event(ServerEventKind::ToolFinished {
            turn: 1,
            call_id: String::new(),
            tool: "read".to_owned(),
            is_error: false,
            detached: false,
            error: None,
            output: Some("one".to_owned()),
            truncated: false,
            duration_ms: 1,
        }),
        event(ServerEventKind::ToolStarted {
            turn: 1,
            call_id: String::new(),
            tool: "grep".to_owned(),
            arguments: None,
            source: None,
        }),
    ];

    assert_eq!(names(&items), vec!["read".to_owned(), "grep".to_owned()]);
}

#[test]
fn a_tool_node_never_ends_up_without_a_name() {
    let items = [
        event(ServerEventKind::ToolOutput {
            turn: 1,
            call_id: "call-1".to_owned(),
            stream: OutputStream::Stdout,
            text: "x".to_owned(),
            tool: "read".to_owned(),
            last: false,
        }),
        event(ServerEventKind::Delta {
            turn: 1,
            delta: Delta::Text {
                index: 0,
                text: "hi".to_owned(),
            },
        }),
    ];

    assert!(names(&items).iter().all(|name| !name.is_empty()));
}
