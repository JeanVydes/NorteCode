//! Send failures are reported, never discarded.

use std::sync::Arc;
use std::time::Duration;

use norte_code_client::RestClient;
use norte_code_client::config::ReconnectConfig;
use norte_code_wire::SessionKey;
use tokio::sync::mpsc;

use super::{ClientEvent, retry_delay, send_prompt};

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
    let reconnect = ReconnectConfig {
        retry_base_ms: 700,
        retry_max_ms: 4_000,
        give_up_attempts: 3,
        healthy_stream_s: 10,
    };

    assert_eq!(retry_delay(&reconnect, 0), Duration::from_millis(700));
    assert_eq!(retry_delay(&reconnect, 1), Duration::from_millis(1_400));
    assert_eq!(retry_delay(&reconnect, 2), Duration::from_millis(2_800));
    assert_eq!(retry_delay(&reconnect, 3), Duration::from_millis(4_000));
}

#[test]
fn retry_delay_never_shifts_past_the_sixth_attempt() {
    let reconnect = ReconnectConfig {
        retry_base_ms: 100,
        retry_max_ms: 100_000,
        give_up_attempts: 3,
        healthy_stream_s: 10,
    };

    assert_eq!(retry_delay(&reconnect, 6), Duration::from_millis(6_400));
    assert_eq!(retry_delay(&reconnect, 20), Duration::from_millis(6_400));
}
