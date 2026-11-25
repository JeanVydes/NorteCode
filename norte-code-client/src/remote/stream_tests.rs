//! The live event queue stays bounded without dropping terminal events.

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::StreamExt;
use norte_code_wire::{ServerEvent, ServerEventKind};

use super::super::mock_host;
use super::{RemoteClient, queue_event};

const MAX_PENDING_EVENTS: usize = 512;

fn event(request_id: u64) -> ServerEvent {
    ServerEvent {
        request_id,
        session: None,
        kind: ServerEventKind::SessionCreated { title: None },
    }
}

#[test]
fn a_flooded_event_queue_keeps_every_event() {
    let mut pending = VecDeque::new();
    for id in 0..MAX_PENDING_EVENTS {
        queue_event(&mut pending, event(id as u64), false);
    }

    queue_event(&mut pending, event(999), false);

    assert_eq!(pending.len(), MAX_PENDING_EVENTS + 1);
    assert_eq!(pending.front().expect("front").0.request_id, 0);
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

    assert_eq!(pending.len(), MAX_PENDING_EVENTS + 1);
    let front = pending.front().expect("front");
    assert!(front.1, "the terminal event is still first");
}

#[tokio::test]
async fn a_finished_turn_keeps_the_event_stream_open() {
    let host = mock_host::start_with_stream().await;
    let client = RemoteClient::new(host.base_url.clone(), None, None).expect("client");
    let mut events = client
        .open_events_of("session-1")
        .await
        .expect("event stream");

    let first = tokio::time::timeout(Duration::from_secs(2), events.next())
        .await
        .expect("a terminal frame arrives")
        .expect("a decoded event");
    assert!(matches!(first.kind, ServerEventKind::TurnFinished { .. }));

    assert!(
        tokio::time::timeout(Duration::from_millis(200), events.next())
            .await
            .is_err(),
        "a finished turn must not close the attachment"
    );
}

#[tokio::test]
async fn a_sequence_gap_reopens_from_the_last_contiguous_event() {
    let host = mock_host::start_with_gap().await;
    let client = RemoteClient::new(host.base_url.clone(), None, None).expect("client");
    let mut first = client
        .open_events_of("session-1")
        .await
        .expect("first stream");
    assert!(first.next().await.is_some());
    assert!(first.next().await.is_none());
    assert_eq!(client.last_seq("session-1"), Some(1));
    let mut replay = client
        .open_events_of("session-1")
        .await
        .expect("replay stream");
    assert!(replay.next().await.is_some());
    assert!(replay.next().await.is_some());
    assert_eq!(client.last_seq("session-1"), Some(3));
}

#[tokio::test]
async fn an_unrecoverable_gap_is_reported_and_the_stream_can_continue() {
    let host = mock_host::start_with_truncated_gap().await;
    let client = RemoteClient::new(host.base_url.clone(), None, None).expect("client");
    let mut first = client
        .open_events_of("session-1")
        .await
        .expect("first stream");
    assert!(first.next().await.is_some());
    assert!(first.next().await.is_none());
    let mut replay = client
        .open_events_of("session-1")
        .await
        .expect("replay stream");
    let warning = replay.next().await.expect("gap warning");
    assert!(matches!(
        warning.kind,
        ServerEventKind::Error {
            code: norte_code_wire::ErrorCode::ReplayUnavailable,
            ..
        }
    ));
    assert!(replay.next().await.is_some());
    assert_eq!(client.last_seq("session-1"), Some(3));
}
