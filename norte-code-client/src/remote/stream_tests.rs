//! The live event queue stays bounded without dropping terminal events.

use std::collections::VecDeque;

use norte_code_wire::{ServerEvent, ServerEventKind};

use super::{MAX_PENDING_EVENTS, queue_event};

fn event(request_id: u64) -> ServerEvent {
    ServerEvent {
        request_id,
        session: None,
        kind: ServerEventKind::SessionCreated { title: None },
    }
}

#[test]
fn a_flooded_event_queue_drops_the_oldest_event() {
    let mut pending = VecDeque::new();
    for id in 0..MAX_PENDING_EVENTS {
        queue_event(&mut pending, event(id as u64), false);
    }

    queue_event(&mut pending, event(999), false);

    assert_eq!(pending.len(), MAX_PENDING_EVENTS);
    assert_eq!(pending.front().expect("front").0.request_id, 1);
    assert_eq!(pending.back().expect("back").0.request_id, 999);
}

#[test]
fn a_terminal_event_is_kept_when_the_queue_is_full() {
    let mut pending = VecDeque::new();
    queue_event(&mut pending, event(0), true);
    for id in 1..MAX_PENDING_EVENTS {
        queue_event(&mut pending, event(id as u64), false);
    }

    queue_event(&mut pending, event(999), false);

    assert_eq!(pending.len(), MAX_PENDING_EVENTS);
    let front = pending.front().expect("front");
    assert!(front.1, "the terminal event is still first");
}
