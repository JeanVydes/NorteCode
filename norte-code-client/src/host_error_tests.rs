use super::HostError;

fn envelope(payload: &serde_json::Value) -> String {
    serde_json::json!({
        "version": "1.0",
        "id": "frame-1",
        "session_id": "session-1",
        "timestamp": 0,
        "type": "ERROR",
        "payload": payload.clone(),
    })
    .to_string()
}

#[test]
fn reads_the_refusal_out_of_the_host_envelope() {
    let body = envelope(&serde_json::json!({
        "code": "INVALID_FRAME",
        "message": "invalid frame: session is not attached",
        "retryable": false,
    }));

    let error = HostError::parse(&body).expect("host refusal");

    assert_eq!(error.code, "INVALID_FRAME");
    assert_eq!(error.message, "invalid frame: session is not attached");
    assert!(!error.retryable);
}

#[test]
fn recognizes_a_session_that_is_not_attached() {
    let body = envelope(&serde_json::json!({
        "code": "INVALID_FRAME",
        "message": "invalid frame: session is not attached",
        "retryable": false,
    }));

    let error = HostError::parse(&body).expect("host refusal");

    assert!(error.is_session_not_attached());
}

#[test]
fn leaves_other_refusals_alone() {
    let body = envelope(&serde_json::json!({
        "code": "SESSION_BUSY",
        "message": "another client is already attached to this session",
        "retryable": true,
    }));

    let error = HostError::parse(&body).expect("host refusal");

    assert!(!error.is_session_not_attached());
    assert!(error.retryable);
}

#[test]
fn reports_no_refusal_for_a_body_that_carries_none() {
    assert!(HostError::parse("{\"events\": []}").is_none());
    assert!(HostError::parse("not json").is_none());
}
