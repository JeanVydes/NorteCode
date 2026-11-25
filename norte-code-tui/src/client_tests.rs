//! Send failures are reported, never discarded.

use std::sync::Arc;
use std::time::Duration;

use norte_code_client::RestClient;
use norte_code_wire::SessionKey;
use tokio::sync::mpsc;

use super::{ClientEvent, StreamOutcome, drive_stream, retry_delay, send_prompt};
use futures_util::stream;

#[tokio::test]
async fn a_refused_send_reports_send_failed() {
    let api = Arc::new(RestClient::new("http://127.0.0.1:1", "token").expect("a local URL parses"));
    let (events, mut receiver) = mpsc::channel(4);
    send_prompt(
        &api,
        &events,
        &SessionKey::new("s1"),
        "hello".to_owned(),
        Vec::new(),
        "steer".to_owned(),
    )
    .await;
    assert!(matches!(
        receiver.recv().await,
        Some(ClientEvent::SendFailed { reason })
            if reason.to_string().contains("message refused")
    ));
}

#[test]
fn retry_delay_doubles_from_the_configured_base_and_stops_at_the_configured_cap() {
    let reconnect = crate::test_support::reconnect_config(700, 4_000);

    assert_eq!(retry_delay(&reconnect, 0), Duration::from_millis(700));
    assert_eq!(retry_delay(&reconnect, 1), Duration::from_millis(1_400));
    assert_eq!(retry_delay(&reconnect, 2), Duration::from_millis(2_800));
    assert_eq!(retry_delay(&reconnect, 3), Duration::from_millis(4_000));
}

#[test]
fn retry_delay_never_shifts_past_the_sixth_attempt() {
    let reconnect = crate::test_support::reconnect_config(100, 100_000);

    assert_eq!(retry_delay(&reconnect, 6), Duration::from_millis(6_400));
    assert_eq!(retry_delay(&reconnect, 20), Duration::from_millis(6_400));
}

fn stream_event(turn: u64) -> norte_code_wire::ServerEvent {
    norte_code_wire::ServerEvent {
        request_id: turn,
        session: Some(SessionKey::new("session_1")),
        kind: norte_code_wire::ServerEventKind::TurnStarted { turn },
    }
}

#[tokio::test]
async fn stream_events_flow_while_a_command_is_still_running() {
    use std::time::Duration;

    let (stream_tx, stream_rx) = mpsc::channel::<norte_code_wire::ServerEvent>(8);
    let mut stream: norte_code_client::EventStream =
        Box::pin(stream::unfold(stream_rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        }));
    let (outcome_tx, mut outcome_rx) = mpsc::channel(8);
    let (events_tx, mut events_rx) = mpsc::channel(32);

    let pump =
        tokio::spawn(
            async move { drive_stream(&mut stream, &mut outcome_rx, &events_tx, None).await },
        );

    for turn in 1..=3 {
        stream_tx
            .send(stream_event(turn))
            .await
            .expect("stream channel open");
    }

    let mut forwarded = 0;
    for _ in 0..3 {
        let received = tokio::time::timeout(Duration::from_secs(1), events_rx.recv())
            .await
            .expect("event forwarded without waiting for the command");
        assert!(matches!(received, Some(ClientEvent::Server(_))));
        forwarded += 1;
    }
    assert_eq!(forwarded, 3, "all stream events must be forwarded promptly");

    outcome_tx
        .send(StreamOutcome::Switch(Box::new(
            norte_code_wire::SessionSummary {
                session_id: "session_2".to_owned(),
                ..norte_code_wire::SessionSummary::default()
            },
        )))
        .await
        .expect("outcome channel open");

    let outcome = tokio::time::timeout(Duration::from_secs(1), pump)
        .await
        .expect("pump returns the outcome")
        .expect("pump task joined");
    assert!(matches!(outcome, StreamOutcome::Switch(_)));
}
