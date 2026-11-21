//! Send failures are reported, never discarded.

use std::sync::Arc;

use norte_code_client::RestClient;
use norte_code_wire::SessionKey;
use tokio::sync::mpsc;

use super::{ClientEvent, send_prompt};

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
        Some(ClientEvent::SendFailed { reason }) if reason.contains("message refused")
    ));
}
