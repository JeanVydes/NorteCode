use std::sync::atomic::Ordering;

use super::super::mock_host;
use crate::agent::AgentClient;
use crate::remote::RemoteClient;

#[tokio::test]
async fn lists_the_jobs_the_host_knows() {
    let host = mock_host::start_with_jobs().await;
    let client = RemoteClient::new(host.base_url.clone(), None, None).expect("client");

    let jobs = client.list_jobs("session-1").await.expect("job list");

    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, "job-1");
    assert_eq!(jobs[0].label, "cargo build");
    assert_eq!(jobs[0].state, norte_code_wire::JobState::Running);
}

#[tokio::test]
async fn stops_a_job_and_reports_how_it_ended() {
    let host = mock_host::start_with_jobs().await;
    let client = RemoteClient::new(host.base_url.clone(), None, None).expect("client");

    let job = client
        .stop_job("session-1", "job-1")
        .await
        .expect("stopped job");

    assert_eq!(job.id, "job-1");
    assert_eq!(job.state, norte_code_wire::JobState::Cancelled);
    assert_eq!(job.elapsed_ms, 1_200);
}

#[tokio::test]
async fn the_session_is_attached_before_the_jobs_are_asked_for() {
    let host = mock_host::start_with_jobs().await;
    let client = RemoteClient::new(host.base_url.clone(), None, None).expect("client");

    client.list_jobs("session-1").await.expect("job list");

    assert_eq!(host.attaches.load(Ordering::SeqCst), 1);
}
