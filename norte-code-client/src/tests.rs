use raep_types::{Envelope, FrameType, StandardFrameType};
use serde_json::Value;

use norte_code_wire::ClientRequest;

use crate::decode::*;
use crate::remote::*;
use crate::sse::*;

use norte_code_wire::{ClientRequestKind, SessionKey};

/// A typed frame fixture: the envelope is built as `Envelope<Value>`
/// with a typed payload, never as raw JSON.
fn frame(kind: FrameType, session_id: &str, payload: serde_json::Value) -> Envelope<Value> {
    let mut frame = Envelope::new(kind, payload, 0);
    frame.session_id = session_id.to_owned();
    frame
}

#[test]
fn prefers_the_standard_payload_over_the_compatibility_extension() {
    let mut frame = frame(
        FrameType::Standard(StandardFrameType::TokenDelta),
        "session-1",
        serde_json::json!({"delta": "canonical", "index": 2, "channel": "text"}),
    );
    frame.turn_id = raep_types::TurnId::new("7").ok();
    frame.ext = Some(serde_json::json!({"norte_code": {
        "request_id": 0,
        "session": "session-1",
        "kind": {"event": "committed", "turn": 7, "version": 1}
    }}));
    let event = server_event_from_frame(&frame).expect("standard token delta");
    assert!(matches!(
        event.kind,
        norte_code_wire::ServerEventKind::Delta {
            turn: 7,
            delta: norte_code_wire::Delta::Text { index: 2, ref text }
        } if text == "canonical"
    ));
}

#[test]
fn rejects_an_unsupported_wire_version() {
    let mut frame = frame(
        FrameType::Standard(StandardFrameType::TokenDelta),
        "",
        serde_json::json!({"delta": "hi", "index": 0}),
    );
    frame.version = "2.0".to_owned();
    let error = server_event_from_frame(&frame).expect_err("unsupported version");
    assert!(error.to_string().contains("wire version"), "{error}");
}

#[test]
fn decodes_a_standard_tool_start() {
    let mut frame = frame(
        FrameType::Standard(StandardFrameType::ToolCallStarted),
        "session-1",
        serde_json::json!({
            "tool_call_id": "call-1",
            "tool_name": "shell",
            "arguments": {"command": "ls"},
            "requires_approval": false
        }),
    );
    frame.turn_id = raep_types::TurnId::new("3").ok();
    let event = server_event_from_frame(&frame).expect("standard tool start");
    assert!(matches!(
        event.kind,
        norte_code_wire::ServerEventKind::ToolStarted { turn: 3, ref call_id, ref tool, .. }
            if call_id == "call-1" && tool == "shell"
    ));
}

#[test]
fn classifies_sequence_steps() {
    assert_eq!(seq_step(None, 1), SeqStep::Apply);
    assert_eq!(seq_step(Some(4), 5), SeqStep::Apply);
    assert_eq!(seq_step(Some(4), 4), SeqStep::Duplicate);
    assert_eq!(seq_step(Some(4), 3), SeqStep::Duplicate);
    assert_eq!(seq_step(Some(4), 6), SeqStep::Gap { expected: 5 });
    assert_eq!(seq_step(Some(u64::MAX), u64::MAX), SeqStep::Duplicate);
}

#[test]
fn encodes_session_path_segments_without_reinterpreting_slashes() {
    assert_eq!(
        percent_encode("provider/session 1"),
        "provider%2Fsession%201"
    );
}

#[test]
fn extracts_session_from_session_scoped_requests() {
    let request = ClientRequest {
        request_id: 1,
        kind: ClientRequestKind::CancelTurn {
            session: SessionKey::new("session-1"),
        },
    };
    assert_eq!(session_from_request(&request).as_deref(), Some("session-1"));
}

#[test]
fn leaves_connection_scoped_requests_without_a_session() {
    let request = ClientRequest {
        request_id: 1,
        kind: ClientRequestKind::ListSessions,
    };
    assert_eq!(session_from_request(&request), None);
}

#[test]
fn prefixes_bearer_tokens_without_redacting_the_value() {
    let client = RemoteClient::new("http://localhost:8080", Some("secret-token"), None)
        .expect("valid bearer token");
    assert_eq!(
        client
            .bearer_token
            .as_ref()
            .and_then(|value| value.to_str().ok()),
        Some("Bearer secret-token")
    );
}

#[test]
fn decodes_canonical_service_data_without_the_compatibility_extension() {
    let frame = frame(
        FrameType::Extension("norte_code".to_owned()),
        "session-1",
        serde_json::json!({
            "event": "service_data",
            "kind": "tool_list",
            "data": [{
                "name": "read",
                "allowed": true,
                "scope": "default"
            }]
        }),
    );
    let event = server_event_from_frame(&frame).expect("canonical service event");
    assert!(matches!(
        event.kind,
        norte_code_wire::ServerEventKind::ToolList { ref tools }
            if tools[0].name == "read"
    ));
}

#[test]
fn decodes_canonical_session_stats_and_jobs() {
    let stats = frame(
        FrameType::Extension("norte_code".to_owned()),
        "session-1",
        serde_json::json!({
            "event": "session_stats",
            "input_tokens": 10,
            "output_tokens": 4,
            "cache_read_tokens": 2,
            "cache_write_tokens": 1,
            "cost_usd": 0.25,
            "requests": 2,
            "context_used": 14,
            "context_window": 100
        }),
    );
    let event = server_event_from_frame(&stats).expect("canonical stats");
    assert!(matches!(
        event.kind,
        norte_code_wire::ServerEventKind::SessionStats {
            context_used: 14,
            context_window: Some(100),
            ..
        }
    ));

    let jobs = frame(
        FrameType::Extension("norte_code".to_owned()),
        "session-1",
        serde_json::json!({
            "event": "job_list",
            "jobs": []
        }),
    );
    let event = server_event_from_frame(&jobs).expect("canonical jobs");
    assert!(matches!(
        event.kind,
        norte_code_wire::ServerEventKind::JobList { ref jobs } if jobs.is_empty()
    ));
}

#[test]
fn serializes_remote_service_requests_with_protocol_tags() {
    let request = ClientRequest {
        request_id: 7,
        kind: ClientRequestKind::ListTelemetry { session: None },
    };
    let value = serde_json::to_value(request).expect("request serializes");
    assert_eq!(value["kind"]["type"], "list_telemetry");
}

#[test]
fn tracks_last_sequence_per_session_and_keeps_the_maximum() {
    let client = RemoteClient::new("http://localhost:8080", None, None).expect("client creates");
    assert_eq!(client.last_seq("session-1"), None);
    client.note_seq("session-1", 7);
    assert_eq!(client.last_seq("session-1"), Some(7));
    client.note_seq("session-1", 4);
    assert_eq!(
        client.last_seq("session-1"),
        Some(7),
        "an older frame must not move the cursor backwards"
    );
    client.note_seq("session-1", 9);
    assert_eq!(client.last_seq("session-1"), Some(9));
    client.note_seq("session-2", 3);
    assert_eq!(client.last_seq("session-2"), Some(3));
    assert_eq!(client.last_seq("session-1"), Some(9));
}

#[test]
fn records_sequence_from_raw_frames_only() {
    let client = RemoteClient::new("http://localhost:8080", None, None).expect("client creates");
    let mut first = frame(
        FrameType::Extension("norte_code".to_owned()),
        "session-1",
        serde_json::json!({}),
    );
    first.seq = Some(12);
    client.note_frame_seq(&first);
    assert_eq!(client.last_seq("session-1"), Some(12));
    let without_seq = frame(
        FrameType::Extension("norte_code".to_owned()),
        "session-1",
        serde_json::json!({}),
    );
    client.note_frame_seq(&without_seq);
    assert_eq!(client.last_seq("session-1"), Some(12));
    let empty_session = frame(
        FrameType::Extension("norte_code".to_owned()),
        "",
        serde_json::json!({}),
    );
    let mut empty_session = empty_session;
    empty_session.seq = Some(99);
    client.note_frame_seq(&empty_session);
    assert_eq!(client.last_seq(""), None);
}

#[test]
fn drains_sse_blocks_in_order_and_reaches_past_the_terminal() {
    // Real envelopes: version, id, timestamp, kind and payload, as the
    // server emits them through `Envelope::new`.
    let frame = |kind: &str, seq: u64, payload: &str| -> String {
        format!(
            "{{\"version\":\"1.0\",\"id\":\"frame-{seq}\",\"seq\":{seq},\"session_id\":\"session-1\",\"timestamp\":0,\"type\":\"{kind}\",\"payload\":{payload}}}"
        )
    };
    let mut buffer = concat!(
        "event: turn_state_changed\n",
        "data: ",
        "{\"session_id\":\"session-1\",\"seq\":1,\"payload\":{\"event\":\"turn_state_changed\",\"state\":\"finished\",\"turn\":1,\"phase\":\"idle\",\"committed\":true}}\n",
        "\n",
        "event: SESSION_STATS\n",
        "data: {\"session_id\":\"session-1\",\"seq\":2,\"payload\":{\"event\":\"session_stats\",\"input_tokens\":10,\"output_tokens\":4,\"cache_read_tokens\":2,\"cache_write_tokens\":1,\"cost_usd\":0.25,\"requests\":2,\"context_used\":14,\"context_window\":100}}\n",
        "\n",
        "data: {\"session_id\":\"session-1\",\"seq\":3,\"payload\":{\"event\":\"session_stats\",\"input_tokens\":11,\"output_tokens\":5,\"cache_read_tokens\":2,\"cache_write_tokens\":1,\"cost_usd\":0.26,\"requests\":3,\"context_used\":15,\"context_window\":100}}"
    )
    .to_owned()
    .into_bytes();
    // The fixtures above are canonical-only frames (no envelope `kind`):
    // they still decode through the canonical path, which is the path
    // `drain_sse_buffer` exercises.
    let _ = frame;
    let drained = drain_sse_buffer(&mut buffer);
    assert_eq!(drained.len(), 2);
    assert!(drained[0].1, "the terminal event is flagged");
    assert_eq!(drained[0].2, Some(1));
    assert!(
        !drained[1].1,
        "the event after the terminal is not terminal but still drains"
    );
    assert_eq!(drained[1].2, Some(2));
    // The partial trailing block is left for the next chunk.
    assert!(String::from_utf8_lossy(&buffer).contains("\"seq\":3"));
}

#[test]
fn keeps_a_multibyte_character_split_across_chunks() {
    let text = "data: {\"session_id\":\"s\",\"payload\":{\"event\":\"session_created\",\"title\":\"café\"}}\n\n";
    let bytes = text.as_bytes();
    let split = text.find('é').expect("accented character") + 1;
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&bytes[..split]);
    assert!(
        drain_sse_buffer(&mut buffer).is_empty(),
        "a partial block must wait for the rest"
    );
    buffer.extend_from_slice(&bytes[split..]);
    let drained = drain_sse_buffer(&mut buffer);
    assert_eq!(drained.len(), 1);
    assert!(matches!(
        drained[0].0.as_ref().unwrap().kind,
        norte_code_wire::ServerEventKind::SessionCreated { title: Some(ref title) }
            if title == "café"
    ));
}
