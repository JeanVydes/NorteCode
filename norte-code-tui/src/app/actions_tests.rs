use crate::ui_model::TranscriptItem;
use norte_code_wire::{Delta, HistorySpeaker, MessageRole, ReplayedKind, ServerEventKind};

use super::transcript_with_history;

fn user_item(id: &str, content: &str, turn: Option<&str>) -> norte_code_wire::HistoryItem {
    norte_code_wire::HistoryItem {
        id: id.to_owned(),
        kind: "user".to_owned(),
        content: content.to_owned(),
        created_at: 1,
        turn_id: turn.map(str::to_owned),
        role: HistorySpeaker::User,
        provenance: None,
    }
}

fn assistant_item(id: &str, content: &str, turn: Option<&str>) -> norte_code_wire::HistoryItem {
    norte_code_wire::HistoryItem {
        id: id.to_owned(),
        kind: "assistant".to_owned(),
        content: content.to_owned(),
        created_at: 2,
        turn_id: turn.map(str::to_owned),
        role: HistorySpeaker::Assistant,
        provenance: None,
    }
}

fn live_stream(items: Vec<ServerEventKind>) -> Vec<TranscriptItem> {
    items
        .into_iter()
        .map(|kind| {
            TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
                request_id: 0,
                session: None,
                kind,
            }))
        })
        .collect()
}

fn delta(text: &str, channel: u32) -> ServerEventKind {
    ServerEventKind::Delta {
        turn: 1,
        delta: Delta::Text {
            index: channel,
            text: text.to_owned(),
        },
    }
}

#[test]
fn committed_deltas_are_not_repeated_after_a_reload() {
    let history = vec![
        user_item("u1", "hola", Some("1")),
        assistant_item("a1", "hola, como te ayudo", Some("1")),
    ];
    let current = live_stream(vec![delta("hola, como te ayudo", 0)]);

    let merged = transcript_with_history(history, current, true);

    assert_eq!(merged.len(), 2);
}

#[test]
fn uncommitted_deltas_survive_a_reload() {
    let history = vec![user_item("u1", "hola", Some("1"))];
    let current = live_stream(vec![delta("token parcial", 0)]);

    let merged = transcript_with_history(history, current, true);

    assert!(merged.iter().any(|item| matches!(
        item,
        TranscriptItem::ServerEvent(event)
            if matches!(&event.kind, ServerEventKind::Delta { delta: Delta::Text { text, .. }, .. } if text == "token parcial")
    )));
}

#[test]
fn replays_are_not_repeated_after_a_reload() {
    let history = vec![
        user_item("u1", "hola", Some("1")),
        assistant_item("a1", "respuesta completa", Some("1")),
    ];
    let replayed = ServerEventKind::MessageReplayed {
        index: 1,
        role: MessageRole::Assistant,
        kind: ReplayedKind::Text,
        text: "respuesta completa".to_owned(),
    };
    let current = live_stream(vec![replayed]);

    let merged = transcript_with_history(history, current, true);

    assert_eq!(merged.len(), 2);
}

#[test]
fn live_user_messages_are_deduped_against_history() {
    let history = vec![user_item("u1", "hola", Some("1"))];
    let current = vec![TranscriptItem::UserMessage {
        text: "hola".to_owned(),
        chips: Vec::new(),
    }];

    let merged = transcript_with_history(history, current, true);

    assert_eq!(merged.len(), 1);
}

#[test]
fn history_replaces_replayed_messages_without_losing_live_deltas() {
    let history = vec![norte_code_wire::HistoryItem {
        id: "history-user".to_owned(),
        kind: "user".to_owned(),
        content: "hi".to_owned(),
        created_at: 1,
        turn_id: None,
        role: HistorySpeaker::User,
        provenance: None,
    }];
    let current = vec![
        TranscriptItem::UserMessage {
            text: "hi".to_owned(),
            chips: Vec::new(),
        },
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::MessageReplayed {
                index: 0,
                role: MessageRole::User,
                kind: ReplayedKind::Text,
                text: "hi".to_owned(),
            },
        })),
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 2,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Reasoning {
                    index: 0,
                    text: "Thinking".to_owned(),
                },
            },
        })),
    ];

    let transcript = transcript_with_history(history, current, true);

    assert_eq!(transcript.len(), 2);
    assert!(matches!(transcript[0], TranscriptItem::History(_)));
    assert!(matches!(
        &transcript[1],
        TranscriptItem::ServerEvent(event)
            if matches!(&event.kind, ServerEventKind::Delta { delta: Delta::Reasoning { text, .. }, .. } if text == "Thinking")
    ));
}

#[test]
fn committed_reasoning_is_not_repeated_after_a_reload() {
    let history = [norte_code_wire::HistoryItem {
        id: "r1".to_owned(),
        kind: "reasoning".to_owned(),
        content: "Thinking".to_owned(),
        created_at: 1,
        turn_id: Some("1".to_owned()),
        role: HistorySpeaker::SystemEvent,
        provenance: None,
    }];
    let current = live_stream(vec![ServerEventKind::Delta {
        turn: 1,
        delta: Delta::Reasoning {
            index: 0,
            text: "Thinking".to_owned(),
        },
    }]);

    let merged = transcript_with_history(history.to_vec(), current, true);

    assert_eq!(merged.len(), 1);
}
