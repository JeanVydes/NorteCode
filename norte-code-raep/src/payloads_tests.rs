use norte_code_wire::ServerEventKind;

use super::*;

#[test]
fn tool_output_uses_canonical_job_payload_and_preserves_wire_event() {
    let payload = canonical_event_payload(&ServerEventKind::ToolOutput {
        turn: 3,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        stream: norte_code_wire::OutputStream::Stdout,
        text: "hello".to_owned(),
        last: true,
    });

    assert_eq!(payload["event"], "job_progress_delta");
    assert_eq!(payload["turn"], 3);
    assert_eq!(payload["call_id"], "call-1");
    assert_eq!(payload["stream"], "stdout");
    assert_eq!(payload["last"], true);
}

#[test]
fn session_forked_uses_the_raep_turn_and_source_fields() {
    let payload = canonical_event_payload(&ServerEventKind::SessionForked {
        from: norte_code_wire::SessionKey::new("source"),
        from_turn_id: Some("turn_4".to_owned()),
        title: Some("branch".to_owned()),
    });

    assert_eq!(
        payload,
        json!({
            "event": "session_forked",
            "from_session_id": "source",
            "from_turn_id": "turn_4",
            "title": "branch"
        })
    );
}

#[test]
fn standard_payloads_match_the_raep_schemas() {
    let budget = standard_event_payload(&ServerEventKind::BudgetExceeded {
        scope: norte_code_wire::BudgetScope::GlobalDaily,
        unit: norte_code_wire::BudgetUnit::Usd,
        spent: 4.5,
        limit: 5.0,
        action: norte_code_wire::BudgetAction::Stop,
    })
    .expect("standard budget payload");
    assert_eq!(
        budget,
        json!({
            "scope": "globaldaily",
            "metric": "usd",
            "used": 4.5,
            "limit": 5.0,
            "action": "stop"
        })
    );

    let tool_output = standard_event_payload(&ServerEventKind::ToolOutput {
        turn: 2,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        stream: norte_code_wire::OutputStream::Stdout,
        text: "hello".to_owned(),
        last: false,
    })
    .expect("standard tool output");
    assert_eq!(
        tool_output,
        json!({
            "tool_call_id": "call-1",
            "tool_name": "shell",
            "stream": "stdout",
            "delta": "hello",
            "index": 0
        })
    );

    let history = standard_event_payload(&ServerEventKind::MessageReplayed {
        index: 1,
        role: norte_code_wire::MessageRole::User,
        kind: norte_code_wire::ReplayedKind::Text,
        text: "hi".to_owned(),
    })
    .expect("standard history page");
    assert_eq!(
        history,
        json!({
            "items": [{
                "id": "1",
                "kind": "user",
                "content": {"role": "user", "content": "hi"},
                "created_at": 0
            }]
        })
    );

    let child = standard_event_payload(&ServerEventKind::ChildSessionUpdated {
        child_session_id: "child-1".to_owned(),
        state: norte_code_wire::ChildState::Idle,
        reason: "turn_finished".to_owned(),
        result: Some(json!({
            "status": "completed",
            "summary": r#"{"answer":42}"#,
            "output": {"answer": 42},
            "output_schema_valid": true,
        })),
        output_schema_valid: Some(true),
    })
    .expect("child update");
    assert_eq!(child["result"]["output"]["answer"], 42);
    assert_eq!(child["result"]["output_schema_valid"], true);
    assert_eq!(child["output_schema_valid"], true);

    let error = standard_event_payload(&ServerEventKind::Error {
        code: norte_code_wire::ErrorCode::Internal,
        message: "boom".to_owned(),
    })
    .expect("standard error");
    assert_eq!(error["message"], "boom");
    assert_eq!(error["retryable"], false);
    assert!(error["code"].is_string());

    let jobs = standard_event_payload(&ServerEventKind::JobList {
        jobs: vec![norte_code_wire::JobInfo {
            id: "job-1".to_owned(),
            label: "sleep 1".to_owned(),
            script: String::new(),
            state: norte_code_wire::JobState::Running,
            exit_code: None,
            timed_out: false,
            elapsed_ms: 10,
            output: String::new(),
            dropped: false,
            output_bytes_total: 0,
        }],
    })
    .expect("standard job list");
    assert_eq!(
        jobs,
        json!({
            "jobs": [{"job_id": "job-1", "command": "sleep 1", "status": "RUNNING"}]
        })
    );
}

#[test]
fn standard_token_and_tool_payloads_match_the_raep_schemas() {
    let delta = standard_event_payload(&ServerEventKind::Delta {
        turn: 2,
        delta: Delta::Text {
            index: 3,
            text: "hello".to_owned(),
        },
    })
    .expect("standard token delta");
    assert_eq!(
        delta,
        json!({"delta": "hello", "index": 3, "channel": "text"})
    );

    let finished = standard_event_payload(&ServerEventKind::TurnFinished {
        turn: 2,
        phase: norte_code_wire::Phase::Idle,
        committed: true,
        output_schema_valid: None,
    })
    .expect("standard turn state");
    assert_eq!(
        finished,
        json!({"state": "COMPLETED", "stop_reason": "end_turn"})
    );
    assert_eq!(
        standard_event_payload(&ServerEventKind::TurnStarted { turn: 2 }).expect("turn start"),
        json!({"state": "CALLING_PROVIDER"})
    );
    let failed = standard_event_payload(&ServerEventKind::TurnStopped {
        reason: "provider_failure".to_owned(),
    })
    .expect("failed turn");
    assert_eq!(failed["state"], "FAILED");
    assert_eq!(failed["stop_reason"], "error");
    assert_eq!(failed["error"]["code"], "INTERNAL");

    let started = standard_event_payload(&ServerEventKind::ToolStarted {
        turn: 2,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        arguments: Some(json!({"command": "ls"})),
        source: None,
    })
    .expect("standard tool start");
    assert_eq!(
        started,
        json!({
            "tool_call_id": "call-1",
            "tool_name": "shell",
            "arguments": {"command": "ls"},
            "requires_approval": false,
            "executor": "server",
            "attempt": 1
        })
    );

    let tool_finished = standard_event_payload(&ServerEventKind::ToolFinished {
        turn: 2,
        call_id: "call-1".to_owned(),
        tool: "shell".to_owned(),
        is_error: false,
        error: None,
        output: Some("done".to_owned()),
        truncated: false,
        detached: false,
        duration_ms: 12,
    })
    .expect("standard tool finish");
    assert_eq!(
        tool_finished,
        json!({
            "tool_call_id": "call-1",
            "tool_name": "shell",
            "status": "OK",
            "duration_ms": 12,
            "result_summary": "done",
            "result_truncated": false
        })
    );
}

#[test]
fn detached_job_lifecycle_payloads_match_the_raep_schemas() {
    let started = standard_event_payload(&ServerEventKind::JobStarted {
        job_id: "job-1".to_owned(),
        tool_call_id: "call-1".to_owned(),
        command: "make".to_owned(),
        started_at: 1_700_000_000_000,
        limits: norte_code_wire::JobLimits {
            timeout_s: Some(120),
            max_output_bytes: Some(1_048_576),
        },
    })
    .expect("job started");
    assert_eq!(started["job_id"], "job-1");
    assert_eq!(started["tool_call_id"], "call-1");
    assert_eq!(started["command"], "make");
    assert_eq!(started["detached"], true);
    assert_eq!(started["started_at"], 1_700_000_000_000_u64);
    assert_eq!(
        started["limits"],
        json!({"timeout_s": 120, "max_output_bytes": 1_048_576})
    );

    let progress = standard_event_payload(&ServerEventKind::JobProgressDelta {
        job_id: "job-1".to_owned(),
        stream: norte_code_wire::OutputStream::Stderr,
        delta: "warning\n".to_owned(),
        index: 17,
    })
    .expect("job progress");
    assert_eq!(
        progress,
        json!({"stream": "stderr", "delta": "warning\n", "index": 17})
    );

    let finished = standard_event_payload(&ServerEventKind::JobFinished {
        job: JobInfo {
            id: "job-1".to_owned(),
            label: "make".to_owned(),
            script: "make".to_owned(),
            state: norte_code_wire::JobState::Finished,
            exit_code: Some(0),
            timed_out: false,
            elapsed_ms: 21,
            output: "done\n".to_owned(),
            dropped: false,
            output_bytes_total: 5,
        },
        triggered_new_turn: true,
        new_turn_id: Some(12),
        injected_into_turn_id: None,
    })
    .expect("job finished");
    assert_eq!(finished["status"], "COMPLETED");
    assert_eq!(finished["exit_code"], 0);
    assert_eq!(finished["summary"], "make");
    assert_eq!(finished["output_snapshot"], "done\n");
    assert_eq!(finished["triggered_new_turn"], true);
    assert_eq!(finished["new_turn_id"], "12");
    assert!(finished["injected_into_turn_id"].is_null());
}
