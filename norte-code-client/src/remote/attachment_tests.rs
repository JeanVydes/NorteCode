use std::sync::atomic::Ordering;

use raep_types::TurnId;

use super::super::mock_host;
use crate::agent::AgentClient;
use crate::remote::RemoteClient;

fn turn() -> TurnId {
    TurnId::new("turn-1").expect("turn id")
}

#[tokio::test]
async fn attaches_again_and_sends_the_command_once_more() {
    let host = mock_host::start().await;
    let client = RemoteClient::new(host.base_url.clone(), None, None).expect("client");

    client
        .cancel_turn("session-1", &turn())
        .await
        .expect("cancel turn");

    assert_eq!(host.attaches.load(Ordering::SeqCst), 2);
    assert_eq!(host.commands.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn reports_why_the_host_refused_the_command() {
    let host = mock_host::start_always_refusing().await;
    let client = RemoteClient::new(host.base_url.clone(), None, None).expect("client");

    let error = client
        .cancel_turn("session-1", &turn())
        .await
        .expect_err("the host refuses every command");

    assert!(error.is_session_not_attached());
    assert!(error.message().contains("session is not attached"));
    assert!(!error.message().contains('{'));
}
