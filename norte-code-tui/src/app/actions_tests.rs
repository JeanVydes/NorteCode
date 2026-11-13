use norte_code_wire::{
    Delta, HistoryItem, HistorySpeaker, MessageRole, ReplayedKind, ServerEvent, ServerEventKind,
};

use super::transcript_with_history;
use crate::ui_model::TranscriptItem;

#[test]
fn history_replaces_replayed_messages_without_losing_live_deltas() {
    let history = vec![HistoryItem {
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
        TranscriptItem::ServerEvent(Box::new(ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::MessageReplayed {
                index: 0,
                role: MessageRole::User,
                kind: ReplayedKind::Text,
                text: "hi".to_owned(),
            },
        })),
        TranscriptItem::ServerEvent(Box::new(ServerEvent {
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
            if matches!(event.kind, ServerEventKind::Delta {
                delta: Delta::Reasoning { ref text, .. },
                ..
            } if text == "Thinking")
    ));
}
