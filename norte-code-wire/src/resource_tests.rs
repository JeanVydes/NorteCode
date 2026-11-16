use super::{
    ChildSessionSummary, JobStatus, JobSummary, PlanSummary, SessionStatus, SessionSummary,
};

/// The spellings are what the host writes and every client decodes: a
/// change here is a protocol change, not a refactor.
#[test]
fn job_status_spellings_are_the_hosts() {
    assert_eq!(
        serde_json::to_string(&JobStatus::Running).unwrap(),
        "\"RUNNING\""
    );
    assert_eq!(
        serde_json::to_string(&JobStatus::TimedOut).unwrap(),
        "\"TIMED_OUT\""
    );
    let decoded: JobStatus = serde_json::from_str("\"KILLED\"").unwrap();
    assert_eq!(decoded, JobStatus::Killed);
    assert!(decoded != JobStatus::Running && !decoded.is_running());
}

#[test]
fn a_session_status_a_client_predates_reads_as_unknown() {
    let decoded: SessionStatus = serde_json::from_str("\"ARCHIVED\"").unwrap();
    assert_eq!(decoded, SessionStatus::Unknown);
    assert_eq!(SessionStatus::Active.label(), "Available");
}

#[test]
fn a_host_session_summary_decodes_without_its_optional_fields() {
    let summary: SessionSummary = serde_json::from_str(
        r#"{"session_id":"s1","title":"One","status":"ACTIVE","title_source":"user",
            "labels":{},"pinned":false,"archived":false,
            "agent":{"id":"norte-code","version":"0.1.0"},
            "created_at":1,"updated_at":2,"last_activity_at":3,
            "turn_count":4,"has_pending":false}"#,
    )
    .unwrap();
    assert_eq!(summary.status, SessionStatus::Active);
    assert_eq!(summary.agent.unwrap().id, "norte-code");
    assert!(summary.model.is_none() && summary.provider.is_none());
}

#[test]
fn a_job_summary_decodes_the_fields_the_host_lists() {
    let job: JobSummary = serde_json::from_str(
        r#"{"job_id":"j1","command":"cargo test","status":"COMPLETED",
            "exit_code":0,"output_bytes_total":12,"output_truncated":false,
            "duration_ms":900}"#,
    )
    .unwrap();
    assert_eq!(job.status.label(), "Completed");
    assert_eq!(job.exit_code, Some(0));
    assert!(!job.status.is_running());
}

#[test]
fn a_plan_summary_carries_the_shared_status() {
    let plan: PlanSummary =
        serde_json::from_str(r##"{"id":"p1","content":"# Plan","status":"pending"}"##).unwrap();
    assert_eq!(plan.status.label(), "awaiting approval");
}

#[test]
fn a_child_list_entry_decodes_from_the_hosts_fields() {
    let child: ChildSessionSummary =
        serde_json::from_str(r#"{"session_id":"c1","title":null,"state":"RUNNING"}"#).unwrap();
    assert!(child.state.is_working());
    assert!(child.agent.is_none() && child.task.is_none());
}
