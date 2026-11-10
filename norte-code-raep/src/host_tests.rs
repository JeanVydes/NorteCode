use super::*;
use norte_code_wire::ServerEvent;
use raep_types::{SessionAttach, TurnId};

use raep_types::FrameId;
use std::collections::{HashMap, HashSet};

use axum::{body::Body, http::Request};
use tower::ServiceExt;

#[test]
fn plugin_update_summary_reports_partial_failures() {
    let result = summarize_plugin_updates(vec![
        ("good".to_owned(), Ok("good".to_owned())),
        ("bad".to_owned(), Err("build failed".to_owned())),
    ]);

    assert_eq!(
        result,
        Err("Plugin update failed: bad: build failed. Updated successfully: good.".to_owned())
    );
}

#[test]
fn plugin_update_summary_reports_successes() {
    let result = summarize_plugin_updates(vec![
        ("one".to_owned(), Ok("one".to_owned())),
        ("two".to_owned(), Ok("two".to_owned())),
    ]);

    assert_eq!(result, Ok("one, two".to_owned()));
}

#[test]
fn user_input_requires_exactly_one_nonempty_form() {
    let missing: AcceptedUserInput = serde_json::from_value(json!({})).expect("decode");
    assert!(validate_user_input(&missing).is_err());

    let both: AcceptedUserInput = serde_json::from_value(json!({
        "prompt": "hello",
        "content": [{"type": "text", "text": "also hello"}]
    }))
    .expect("decode");
    assert!(validate_user_input(&both).is_err());

    let empty: AcceptedUserInput = serde_json::from_value(json!({"prompt": "  "})).expect("decode");
    assert!(validate_user_input(&empty).is_err());

    let prompt: AcceptedUserInput =
        serde_json::from_value(json!({"prompt": "hello"})).expect("decode");
    assert!(validate_user_input(&prompt).is_ok());
}

#[test]
fn multimodal_input_accepts_only_supported_upload_parts() {
    let image: AcceptedUserInput = serde_json::from_value(json!({
        "content": [{
            "type": "image",
            "source": {"kind": "upload", "upload_id": "upl_test"},
            "mime_type": "image/png",
            "alt": "diagram"
        }]
    }))
    .expect("decode");
    assert!(validate_user_input(&image).is_ok());

    let foreign_source = serde_json::from_value::<AcceptedUserInput>(json!({
        "content": [{
            "type": "image",
            "source": {"kind": "url", "url": "https://example.test/image.png"},
            "mime_type": "image/png",
            "alt": "diagram"
        }]
    }));
    assert!(foreign_source.is_err());

    let inline = serde_json::from_value::<AcceptedUserInput>(json!({
        "content": [{
            "type": "image",
            "source": {"kind": "inline", "data": "AA=="},
            "mime_type": "image/png"
        }]
    }));
    assert!(inline.is_err());

    let video: AcceptedUserInput = serde_json::from_value(json!({
        "content": [{
            "type": "video",
            "source": {"kind": "upload", "upload_id": "upl_test"},
            "mime_type": "video/mp4",
            "duration_ms": 1200
        }]
    }))
    .expect("decode");
    assert!(validate_user_input(&video).is_ok());

    let audio: AcceptedUserInput = serde_json::from_value(json!({
        "content": [{
            "type": "audio",
            "source": {"kind": "upload", "upload_id": "upl_test"},
            "mime_type": "audio/wav",
            "duration_ms": 1200,
            "transcript": "hello"
        }]
    }))
    .expect("decode");
    assert!(validate_user_input(&audio).is_ok());

    let wrong_mime: AcceptedUserInput = serde_json::from_value(json!({
        "content": [{
            "type": "document",
            "source": {"kind": "upload", "upload_id": "upl_test"},
            "mime_type": "image/png"
        }]
    }))
    .expect("decode");
    assert!(validate_user_input(&wrong_mime).is_err());
}

#[tokio::test]
async fn upload_index_keeps_session_and_owner_boundaries() {
    let index = MemoryUploadIndex::default();
    let record = UploadRecord {
        id: "upl_test".to_owned(),
        session_id: "session-a".to_owned(),
        owner: Some("alice".to_owned()),
        blob_id: "blob".to_owned(),
        mime_type: "image/png".to_owned(),
        bytes: 3,
        sha256: "abc".to_owned(),
        expires_at: now_millis() + 60_000,
        referenced: false,
    };
    index.insert(record.clone()).await.expect("insert");
    assert_eq!(index.get("upl_test").await.expect("get"), Some(record));
    index
        .mark_referenced("session-a", Some("alice"), &["upl_test".to_owned()])
        .await
        .expect("mark referenced");
    assert!(
        index
            .get("upl_test")
            .await
            .expect("get")
            .is_some_and(|upload| upload.referenced)
    );
    assert!(
        index
            .mark_referenced("session-b", Some("alice"), &["upl_test".to_owned()],)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn generated_audio_output_is_durable_and_owner_scoped() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let blobs = Arc::new(norte::InMemoryBlobStore::new());
    let host = NorteCodeRaepHost::new(service, HostConfig::default())
        .expect("host")
        .with_artifact_store(Arc::new(BlobArtifactStore::new(
            Arc::clone(&blobs) as Arc<dyn BlobStore>
        )))
        .with_upload_index(Arc::new(MemoryUploadIndex::default()));
    host.state
        .lock()
        .await
        .session_owners
        .insert("session-a".to_owned(), "alice".to_owned());

    let media = norte_code_wire::MediaPart {
        kind: norte_code_wire::MediaKind::Audio,
        source: norte_code_wire::MediaSource::Inline(b"generated audio".to_vec()),
        mime: "audio/wav".to_owned(),
        metadata: norte_code_wire::MediaMetadata::default(),
    };
    let events = host
        .emit_media_output("session-a", 7, 0, &media)
        .await
        .expect("emit media output");

    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].kind,
        standard(StandardFrameType::OutputPartStarted)
    );
    assert_eq!(
        events[1].kind,
        standard(StandardFrameType::OutputPartFinished)
    );
    let output_id = events[1].payload["source"]["output_id"]
        .as_str()
        .expect("output id");
    let (bytes, mime_type) = host
        .get_session_output_as("session-a", output_id, Some("alice"))
        .await
        .expect("fetch output");
    assert_eq!(bytes, b"generated audio");
    assert_eq!(mime_type, "audio/wav");
    assert!(
        host.get_session_output_as("session-a", output_id, Some("bob"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn generated_output_http_download_supports_ranges_and_auth() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let blobs = Arc::new(norte::InMemoryBlobStore::new());
    let host = Arc::new(
        NorteCodeRaepHost::new(
            service,
            HostConfig {
                auth_principals: HashMap::from([
                    ("token-a".to_owned(), "alice".to_owned()),
                    ("token-b".to_owned(), "bob".to_owned()),
                ]),
                ..HostConfig::default()
            },
        )
        .expect("host")
        .with_artifact_store(Arc::new(BlobArtifactStore::new(
            Arc::clone(&blobs) as Arc<dyn BlobStore>
        )))
        .with_upload_index(Arc::new(MemoryUploadIndex::default())),
    );
    host.state
        .lock()
        .await
        .session_owners
        .insert("session-a".to_owned(), "alice".to_owned());
    let media = norte_code_wire::MediaPart {
        kind: norte_code_wire::MediaKind::Audio,
        source: norte_code_wire::MediaSource::Inline(b"generated audio".to_vec()),
        mime: "audio/wav".to_owned(),
        metadata: norte_code_wire::MediaMetadata::default(),
    };
    let events = host
        .emit_media_output("session-a", 7, 0, &media)
        .await
        .expect("emit media output");
    let output_id = events[1].payload["source"]["output_id"]
        .as_str()
        .expect("output id");
    let app = crate::http::router(host);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/sessions/session-a/outputs/{output_id}"))
                .header("authorization", "Bearer token-a")
                .header("range", "bytes=0-7")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_RANGE)
            .expect("content range"),
        "bytes 0-7/15"
    );
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("body");
    assert_eq!(&body[..], b"generate");

    let denied = app
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/sessions/session-a/outputs/{output_id}"))
                .header("authorization", "Bearer token-b")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(denied.status(), axum::http::StatusCode::NOT_FOUND);
}

fn test_job_info(
    state: norte_code_wire::JobState,
    exit_code: Option<i32>,
    timed_out: bool,
    output: &str,
    output_bytes_total: u64,
) -> norte_code_wire::JobInfo {
    norte_code_wire::JobInfo {
        id: "job-test".to_owned(),
        label: "test".to_owned(),
        script: "test".to_owned(),
        state,
        exit_code,
        timed_out,
        elapsed_ms: 10,
        output: output.to_owned(),
        dropped: output_bytes_total > output.len() as u64,
        output_bytes_total,
    }
}

#[test]
fn job_state_uses_registry_status_and_retained_output_cursor() {
    let failed = test_job_info(
        norte_code_wire::JobState::Finished,
        Some(2),
        false,
        "output",
        6,
    );
    assert_eq!(job_status(&failed), JobStatus::Failed);
    assert_eq!(
        job_output_from_index(&failed, 2).expect("retained output"),
        "tput"
    );
    assert!(matches!(
        job_output_from_index(&failed, 7),
        Err(HostError::InvalidFrame(message)) if message.contains("exceeds")
    ));

    let timed_out = test_job_info(
        norte_code_wire::JobState::Finished,
        None,
        true,
        "output",
        10,
    );
    assert_eq!(job_status(&timed_out), JobStatus::TimedOut);
    assert!(matches!(
        job_output_from_index(&timed_out, 3),
        Err(HostError::InvalidFrame(message)) if message == "JOB_OUTPUT_EXPIRED"
    ));
}

#[test]
fn history_items_keep_rewound_turn_identity() {
    let item = history_item_value(HistoryItemView {
        turn: Some(3),
        role: norte_code_wire::MessageRole::User,
        kind: norte_code_wire::ReplayedKind::Text,
        text: "old input".to_owned(),
        timestamp: 1,
        rewound: true,
        origin: None,
        max_content_bytes: None,
    });
    assert_eq!(item.turn_id.as_deref(), Some("turn_3"));
    assert!(item.rewound);
}
use norte::BlobStore;

fn replay_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("norte-code-replay-{}", FrameId::generate()))
}

#[test]
fn replay_log_round_trips_frames_after_a_sequence() {
    let dir = replay_dir();
    let log = ReplayLog::new(&dir, 64 * 1024);
    for seq in 1..=3 {
        let mut frame = Envelope::new(
            standard(StandardFrameType::TokenDelta),
            json!({"seq": seq}),
            i64::try_from(seq).expect("test sequence fits in i64"),
        );
        frame.session_id = "session-a".to_owned();
        frame.seq = Some(seq);
        log.append(&frame).expect("append");
    }
    let window = log.after("session-a", 1);
    assert!(!window.truncated);
    assert_eq!(
        window
            .frames
            .iter()
            .filter_map(|frame| frame.seq)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(log.last_seq("session-a"), Some(3));
    let other = log.after("session-b", 0);
    assert_eq!(other.frames, Vec::new());
    std::fs::remove_dir_all(dir).expect("remove replay directory");
}

#[test]
fn replay_log_trims_to_the_byte_cap_and_reports_truncation() {
    let dir = replay_dir();
    let log = ReplayLog::new(&dir, 700);
    for seq in 1..=20 {
        let mut frame = Envelope::new(
            standard(StandardFrameType::TokenDelta),
            json!({"seq": seq}),
            i64::try_from(seq).expect("test sequence fits in i64"),
        );
        frame.session_id = "session-a".to_owned();
        frame.seq = Some(seq);
        log.append(&frame).expect("append");
    }
    let window = log.after("session-a", 0);
    assert!(
        window.truncated,
        "asking from the start of a trimmed log is truncated"
    );
    let last = window.frames.last().and_then(|frame| frame.seq);
    assert_eq!(last, Some(20), "the newest frames survive");
    assert!(
        window.frames.len() < 20,
        "the cap drops older frames instead of growing"
    );
    std::fs::remove_dir_all(dir).expect("remove replay directory");
}

#[test]
fn replay_log_survives_a_reopen_with_its_sequence() {
    let dir = replay_dir();
    {
        let log = ReplayLog::new(&dir, 64 * 1024);
        let mut frame = Envelope::new(standard(StandardFrameType::TokenDelta), json!({}), 1);
        frame.session_id = "session-a".to_owned();
        frame.seq = Some(9);
        log.append(&frame).expect("append");
    }
    let reopened = ReplayLog::new(&dir, 64 * 1024);
    assert_eq!(reopened.last_seq("session-a"), Some(9));
    std::fs::remove_dir_all(dir).expect("remove replay directory");
}

#[tokio::test]
async fn service_turn_events_keep_the_client_turn_id() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
    let client_turn_id = TurnId::new("turn_from_client").expect("turn id");
    let queued_turn_id = TurnId::new("queued_turn").expect("queued turn id");
    {
        let mut state = host.state.lock().await;
        state.turns.pending_push("session", client_turn_id.clone());
        state.turns.enqueue(
            "session",
            QueuedTurn {
                turn_id: queued_turn_id.clone(),
                text: "queued prompt".to_owned(),
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                metadata: None,
                output: vec![raep_types::Modality::Text],
                response_format: None,
                replaces_turn_id: None,
            },
            false,
        );
    }
    let session = SessionKey::new("session");
    let events = vec![
        ServerEvent {
            request_id: 0,
            session: Some(session.clone()),
            kind: ServerEventKind::TurnStarted { turn: 17 },
        },
        ServerEvent {
            request_id: 0,
            session: Some(session.clone()),
            kind: ServerEventKind::ToolStarted {
                turn: 17,
                call_id: "call-1".to_owned(),
                tool: "shell".to_owned(),
                arguments: None,
                source: None,
            },
        },
        ServerEvent {
            request_id: 0,
            session: Some(session.clone()),
            kind: ServerEventKind::ToolFinished {
                turn: 17,
                call_id: "call-1".to_owned(),
                tool: "shell".to_owned(),
                is_error: false,
                error: None,
                output: Some("done".to_owned()),
                truncated: false,
                detached: false,
                duration_ms: 10,
            },
        },
        ServerEvent {
            request_id: 0,
            session: Some(session),
            kind: ServerEventKind::TurnStopped {
                reason: "cancelled".to_owned(),
            },
        },
    ];
    let frames = host
        .collect_service_events("session", Box::pin(futures_util::stream::iter(events)))
        .await
        .expect("turn events");
    assert_eq!(frames.len(), 7);
    assert_eq!(frames[0].payload["state"], "CALLING_PROVIDER");
    assert_eq!(frames[1].payload["state"], "EXECUTING_TOOL");
    assert_eq!(frames[2].kind, standard(StandardFrameType::ToolCallStarted));
    assert_eq!(
        frames[3].kind,
        standard(StandardFrameType::ToolCallFinished)
    );
    assert_eq!(frames[4].payload["state"], "CALLING_PROVIDER");
    assert_eq!(frames[5].payload["state"], "FAILED");
    assert_eq!(frames[5].payload["stop_reason"], "error");
    assert!(
        frames[..6]
            .iter()
            .all(|frame| frame.turn_id.as_ref() == Some(&client_turn_id))
    );
    assert_eq!(frames[6].turn_id.as_ref(), Some(&queued_turn_id));
    assert_eq!(frames[6].payload["state"], "FAILED");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn user_inputs_require_turn_ids_and_queue_with_a_bounded_position() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let host = NorteCodeRaepHost::new(
        service,
        HostConfig {
            max_queued_inputs: 1,
            ..HostConfig::default()
        },
    )
    .expect("host");
    let attach = Envelope::new(
        standard(StandardFrameType::SessionAttach),
        payload(SessionAttach {
            agent_id: "norte-code".to_owned(),
            agent_override: None,
            create_if_missing: Some(true),
            mode: None,
            takeover: None,
            resume_from_seq: None,
            capabilities: None,
            client_info: None,
            options: None,
        }),
        1,
    );
    let attached = host.handle(attach).await.expect("attach");
    let session_id = attached[0].session_id.clone();
    let mut missing_turn_id = Envelope::new(
        standard(StandardFrameType::UserInput),
        json!({"prompt": "must include a turn id"}),
        2,
    );
    missing_turn_id.session_id = session_id.clone();
    assert!(matches!(
        host.handle(missing_turn_id).await,
        Err(HostError::InvalidFrame(message)) if message.contains("turn_id")
    ));

    host.state
        .lock()
        .await
        .turns
        .activate(&session_id, TurnId::new("active_turn").expect("turn id"));
    let mut queued = Envelope::new(
        standard(StandardFrameType::UserInput),
        json!({"prompt": "queued input", "queue_policy": "queue"}),
        3,
    );
    queued.session_id = session_id.clone();
    queued.turn_id = Some(TurnId::new("queued_turn").expect("turn id"));
    let response = host.handle(queued).await.expect("queued input");
    assert_eq!(response.len(), 2);
    assert_eq!(
        response[0].kind,
        standard(StandardFrameType::UserInputAccepted)
    );
    assert_eq!(response[0].payload["position"], 1);
    assert_eq!(
        response[0].turn_id.as_ref().map(TurnId::as_str),
        Some("queued_turn")
    );
    assert_eq!(response[1].payload["state"], "QUEUED");

    let mut conflicting_retry = Envelope::new(
        standard(StandardFrameType::UserInput),
        json!({"prompt": "different content", "queue_policy": "queue"}),
        4,
    );
    conflicting_retry.session_id = session_id.clone();
    conflicting_retry.turn_id = Some(TurnId::new("queued_turn").expect("turn id"));
    assert!(matches!(
        host.handle(conflicting_retry).await,
        Err(HostError::IdempotencyConflict(_))
    ));

    let mut overflow = Envelope::new(
        standard(StandardFrameType::UserInput),
        json!({"prompt": "one too many", "queue_policy": "queue"}),
        5,
    );
    overflow.session_id = session_id.clone();
    overflow.turn_id = Some(TurnId::new("overflow_turn").expect("turn id"));
    assert!(matches!(
        host.handle(overflow).await,
        Err(HostError::SessionBusy)
    ));
    let mut cancel_queued = Envelope::new(standard(StandardFrameType::CancelTurn), json!({}), 6);
    cancel_queued.session_id = session_id;
    cancel_queued.turn_id = Some(TurnId::new("queued_turn").expect("turn id"));
    let cancelled = host
        .handle(cancel_queued)
        .await
        .expect("cancel queued turn");
    assert_eq!(cancelled[0].payload["state"], "CANCELLED");
    assert_eq!(cancelled[0].payload["stop_reason"], "cancelled");
}

#[tokio::test]
async fn attach_delivers_replay_between_attached_and_state() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
    let first = host
        .handle(Envelope::new(
            standard(StandardFrameType::SessionAttach),
            payload(SessionAttach {
                agent_id: "norte-code".to_owned(),
                agent_override: None,
                create_if_missing: Some(true),
                mode: None,
                takeover: None,
                resume_from_seq: None,
                capabilities: None,
                client_info: None,
                options: None,
            }),
            1,
        ))
        .await
        .expect("initial attach");
    let session_id = first[0].session_id.clone();
    let mut detach = Envelope::new(standard(StandardFrameType::SessionDetach), json!({}), 2);
    detach.session_id = session_id.clone();
    host.handle(detach).await.expect("detach");
    host.emit(
        &session_id,
        StandardFrameType::TokenDelta,
        json!({"delta": "replayed"}),
    )
    .await
    .expect("emit replay frame");

    let mut resume = Envelope::new(
        standard(StandardFrameType::SessionAttach),
        payload(SessionAttach {
            agent_id: "norte-code".to_owned(),
            agent_override: None,
            create_if_missing: Some(false),
            mode: None,
            takeover: None,
            resume_from_seq: Some(2),
            capabilities: None,
            client_info: None,
            options: None,
        }),
        3,
    );
    resume.session_id = session_id.clone();
    let resumed = host.handle(resume).await.expect("resume");
    assert_eq!(resumed.len(), 3);
    assert_eq!(
        resumed
            .iter()
            .map(|frame| frame.kind.clone())
            .collect::<Vec<_>>(),
        [
            // The missed frames come first: the attach frame that follows
            // names the window they fill.
            standard(StandardFrameType::TokenDelta),
            standard(StandardFrameType::SessionAttached),
            standard(StandardFrameType::SessionState),
        ]
    );
    let current_seq = resumed
        .iter()
        .filter_map(|frame| frame.seq)
        .max()
        .expect("resumed sequence");
    let mut future_ack = Envelope::new(
        standard(StandardFrameType::Ack),
        json!({"last_seq": current_seq + 1}),
        4,
    );
    future_ack.session_id = session_id.clone();
    assert!(matches!(
        host.handle(future_ack).await,
        Err(HostError::InvalidFrame(message)) if message.contains("exceeds")
    ));
    let mut ack = Envelope::new(
        standard(StandardFrameType::Ack),
        json!({"last_seq": current_seq}),
        5,
    );
    ack.session_id = session_id;
    host.handle(ack).await.expect("ack emitted frames");
    assert_eq!(
        resumed
            .iter()
            .filter_map(|frame| frame.seq)
            .collect::<Vec<_>>(),
        [3, 4, 5]
    );
}

#[tokio::test]
async fn connection_limit_is_enforced_without_waiting() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let host = NorteCodeRaepHost::new(
        service,
        HostConfig {
            max_connections: 1,
            ..HostConfig::default()
        },
    )
    .expect("host");
    let permit = host.try_connection().expect("first connection");
    assert!(host.try_connection().is_none());
    drop(permit);
    assert!(host.try_connection().is_some());
}

#[tokio::test]
async fn attaching_emits_schema_shaped_state_without_replaying_history() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
    let hello = host.hello();
    assert_eq!(hello.payload["limits"]["max_concurrent_jobs"], 8);
    assert_eq!(hello.payload["limits"]["job_output_retention_s"], 86_400);
    let info = host.info();
    assert_eq!(info["limits"]["max_concurrent_jobs"], 8);
    assert_eq!(info["limits"]["job_output_retention_s"], 86_400);
    let frame = Envelope::new(
        standard(StandardFrameType::SessionAttach),
        json!({"agent_id": "norte-code", "create_if_missing": true}),
        1,
    );
    let frames = host.handle(frame).await.expect("attach");
    assert_eq!(frames.len(), 2, "attach must not replay history");
    assert_eq!(frames[0].kind, standard(StandardFrameType::SessionAttached));
    assert_eq!(frames[0].payload["created"], true);
    assert_eq!(frames[0].payload["mode"], "controller");
    assert_eq!(frames[0].payload["last_seq"], 0);
    assert_eq!(frames[1].kind, standard(StandardFrameType::SessionState));
    assert_eq!(frames[1].payload["status"], "ACTIVE");
    assert!(
        frames[1].payload["history_summary"].is_array(),
        "SESSION_STATE must carry history_summary"
    );
}

#[tokio::test]
async fn attaching_existing_session_infers_its_agent_from_service() {
    let service = || {
        Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(norte_code_protocol::InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        )
    };
    let default_service = service();
    let subagent_service = service();
    let session_id = subagent_service
        .start_session(None)
        .await
        .expect("start session")
        .to_string();
    let host = NorteCodeRaepHost::with_agents(
        HashMap::from([
            ("norte-code".to_owned(), default_service),
            ("norte-code-subagent".to_owned(), subagent_service),
        ]),
        HostConfig {
            agent_id: "norte-code".to_owned(),
            ..HostConfig::default()
        },
    )
    .expect("host");
    let mut frame = Envelope::new(
        standard(StandardFrameType::SessionAttach),
        json!({"agent_id": "norte-code-subagent", "create_if_missing": false}),
        1,
    );
    frame.session_id = session_id;

    assert!(
        host.handle(frame)
            .await
            .expect("attach to existing subagent session")
            .iter()
            .any(|frame| frame.kind == standard(StandardFrameType::SessionAttached))
    );
}

#[tokio::test]
async fn history_get_uses_turn_cursor_and_rejects_bad_requests() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
    let attached = host
        .handle(Envelope::new(
            standard(StandardFrameType::SessionAttach),
            json!({"agent_id": "norte-code", "create_if_missing": true}),
            1,
        ))
        .await
        .expect("attach");
    let session_id = attached[0].session_id.clone();
    let mut request = Envelope::new(
        standard(StandardFrameType::HistoryGet),
        json!({"limit": 10}),
        2,
    );
    request.session_id = session_id.clone();
    let page = host.handle(request).await.expect("history page");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].kind, standard(StandardFrameType::HistoryPage));
    assert_eq!(page[0].payload["items"], json!([]));
    assert_eq!(page[0].payload["has_more"], false);

    let mut invalid_cursor = Envelope::new(
        standard(StandardFrameType::HistoryGet),
        json!({"before_turn_id": "not-a-turn"}),
        3,
    );
    invalid_cursor.session_id = session_id;
    assert!(matches!(
        host.handle(invalid_cursor).await,
        Err(HostError::InvalidFrame(message)) if message.contains("before_turn_id")
    ));

    let mut obsolete_cursor = Envelope::new(
        standard(StandardFrameType::HistoryGet),
        json!({"after": "1"}),
        4,
    );
    obsolete_cursor.session_id = attached[0].session_id.clone();
    assert!(host.handle(obsolete_cursor).await.is_err());

    let mut invalid_limit = Envelope::new(
        standard(StandardFrameType::HistoryGet),
        json!({"limit": 101}),
        5,
    );
    invalid_limit.session_id = attached[0].session_id.clone();
    assert!(matches!(
        host.handle(invalid_limit).await,
        Err(HostError::InvalidFrame(message)) if message.contains("limit")
    ));

    let mut unknown_turn = Envelope::new(
        standard(StandardFrameType::HistoryGet),
        json!({"before_turn_id": "turn_999"}),
        6,
    );
    unknown_turn.session_id = attached[0].session_id.clone();
    assert!(matches!(
        host.handle(unknown_turn).await,
        Err(HostError::InvalidFrame(message)) if message.contains("TURN_NOT_FOUND")
    ));
}

#[tokio::test]
async fn text_bundle_export_is_bounded_and_not_advertised_as_full_portability() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
    let info = host.info();
    let capabilities = info["capabilities"].as_array().expect("capability list");
    let capability_names = capabilities
        .iter()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    assert!(capability_names.contains("transport.ws"));
    assert!(capability_names.contains("transport.sse"));
    assert!(capability_names.contains("history"));
    assert!(capability_names.contains("X_NORTECODE_JOBS_BASIC"));
    assert!(capability_names.contains("X_NORTECODE_DELEGATION_BASIC"));
    for unsupported in [
        "jobs",
        "delegation",
        "handoff",
        "quota",
        "manage",
        "reconfigure",
        "fork",
        "compaction",
        "export",
        "import",
    ] {
        assert!(
            !capability_names.contains(unsupported),
            "unsupported capability {unsupported} must not be advertised"
        );
    }
    assert!(matches!(
        NorteCodeRaepHost::import(),
        Err(HostError::InvalidFrame(message)) if message.contains("HTTP workflow")
    ));

    let attached = host
        .handle(Envelope::new(
            standard(StandardFrameType::SessionAttach),
            json!({"agent_id": "norte-code", "create_if_missing": true}),
            1,
        ))
        .await
        .expect("attach");
    let session_id = attached[0].session_id.clone();
    let mut default_export =
        Envelope::new(standard(StandardFrameType::SessionExport), json!({}), 2);
    default_export.session_id = session_id.clone();
    assert!(matches!(
        host.handle(default_export).await,
        Err(HostError::InvalidFrame(message)) if message.contains("all false")
    ));
    let mut export = Envelope::new(
        standard(StandardFrameType::SessionExport),
        json!({"include": {
            "children": false,
            "uploads": false,
            "job_output": false,
            "outputs": false,
            "workspace": false
        }}),
        3,
    );
    export.session_id = session_id;
    let ready = host.handle_as(export, Some("local")).await.expect("export");
    assert_eq!(
        ready[0].kind,
        standard(StandardFrameType::SessionExportReady)
    );
    let export_id = ready[0].payload["export_id"].as_str().expect("export id");
    let (bytes, _) = host
        .get_export_as(export_id, "local")
        .await
        .expect("bundle");
    assert!(host.get_export_as(export_id, "other").await.is_err());
    let (manifest, files) =
        crate::bundle::read_bundle(&bytes, 1024 * 1024).expect("valid exported bundle");
    assert_eq!(manifest.sessions.len(), 1);
    assert_eq!(files.len(), 2);
}

#[tokio::test]
async fn observer_attach_can_read_history_but_cannot_control_session() {
    let service = Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let host = NorteCodeRaepHost::new(service, HostConfig::default()).expect("host");
    let attached = host
        .handle(Envelope::new(
            standard(StandardFrameType::SessionAttach),
            json!({
                "agent_id": "norte-code",
                "create_if_missing": true,
                "mode": "observer"
            }),
            1,
        ))
        .await
        .expect("observer attach");
    let session_id = attached[0].session_id.clone();
    let mut history = Envelope::new(
        standard(StandardFrameType::HistoryGet),
        json!({"limit": 10}),
        2,
    );
    history.session_id = session_id.clone();
    assert!(host.handle(history).await.is_ok());

    let mut input = Envelope::new(
        standard(StandardFrameType::UserInput),
        json!({"prompt": "must be rejected"}),
        3,
    );
    input.session_id = session_id;
    assert!(matches!(
        host.handle(input).await,
        Err(HostError::InvalidFrame(message)) if message.contains("observer")
    ));
}

#[test]
fn ordinary_tool_output_is_not_mislabeled_as_a_job() {
    let kind = ServerEventKind::ToolOutput {
        turn: 1,
        call_id: "job-1".to_owned(),
        tool: "shell".to_owned(),
        stream: norte_code_wire::OutputStream::Stdout,
        text: "hi".to_owned(),
        last: false,
    };
    assert_eq!(job_id_for_event(&kind), None);
    assert_eq!(turn_id_for_event(&kind), Some(1));
}

#[test]
fn job_list_status_filters_use_the_closed_status_enum() {
    let request: JobListRequest = serde_json::from_value(json!({
        "status": ["RUNNING", "TIMED_OUT"],
        "limit": 10
    }))
    .expect("valid job list request");
    assert_eq!(
        request.status,
        Some(vec![JobStatus::Running, JobStatus::TimedOut])
    );
    assert!(
        serde_json::from_value::<JobListRequest>(json!({
            "status": ["RUNNIG"]
        }))
        .is_err()
    );
}

#[test]
fn detached_job_events_use_standard_frames_and_job_envelope_ids() {
    let started = ServerEventKind::JobStarted {
        job_id: "job-1".to_owned(),
        tool_call_id: "call-1".to_owned(),
        command: "make".to_owned(),
        started_at: 1_700_000_000_000,
        limits: norte_code_wire::JobLimits {
            timeout_s: Some(120),
            max_output_bytes: Some(1_048_576),
        },
    };
    assert_eq!(
        frame_type_for_event(&started),
        standard(StandardFrameType::JobStarted)
    );
    assert_eq!(job_id_for_event(&started).as_deref(), Some("job-1"));

    let progress = ServerEventKind::JobProgressDelta {
        job_id: "job-1".to_owned(),
        stream: norte_code_wire::OutputStream::Stdout,
        delta: "ok\n".to_owned(),
        index: 0,
    };
    assert_eq!(
        frame_type_for_event(&progress),
        standard(StandardFrameType::JobProgressDelta)
    );
    assert_eq!(job_id_for_event(&progress).as_deref(), Some("job-1"));
    assert_eq!(turn_id_for_event(&progress), None);

    let finished = ServerEventKind::JobFinished {
        job: norte_code_wire::JobInfo {
            id: "job-1".to_owned(),
            label: "make".to_owned(),
            script: "make".to_owned(),
            state: norte_code_wire::JobState::Finished,
            exit_code: Some(0),
            timed_out: false,
            elapsed_ms: 10,
            output: "ok\n".to_owned(),
            dropped: false,
            output_bytes_total: 3,
        },
        triggered_new_turn: false,
        new_turn_id: None,
        injected_into_turn_id: None,
    };
    assert_eq!(
        frame_type_for_event(&finished),
        standard(StandardFrameType::JobFinished)
    );
    assert_eq!(job_id_for_event(&finished).as_deref(), Some("job-1"));
}

#[test]
fn session_job_turn_policy_is_parsed_and_unknown_options_are_rejected() {
    assert_eq!(
        parse_job_turn_policy(Some(&json!({"job_turn_policy": "always"}))).expect("always policy"),
        Some(norte_code_wire::JobTurnPolicy::Always)
    );
    assert_eq!(
        parse_job_turn_policy(Some(&json!({"job_turn_policy": "on_failure"})))
            .expect("on_failure policy"),
        Some(norte_code_wire::JobTurnPolicy::OnFailure)
    );
    assert_eq!(
        parse_job_turn_policy(Some(&json!({"job_turn_policy": "never"}))).expect("never policy"),
        Some(norte_code_wire::JobTurnPolicy::Never)
    );
    assert!(
        parse_job_turn_policy(Some(&json!({"notify": true}))).is_err(),
        "unsupported session options fail rather than being ignored"
    );
}

#[test]
fn questions_use_a_namespaced_server_frame() {
    let frame = frame_type_for_event(&ServerEventKind::QuestionAsked {
        turn: 1,
        question: "approval".to_owned(),
        prompt: "Continue?".to_owned(),
        options: Vec::new(),
        multiple: false,
    });
    assert_eq!(
        frame,
        FrameType::Extension("norte_code.question_asked".to_owned())
    );
}
