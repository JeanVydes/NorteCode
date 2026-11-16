use super::*;
use crate::{BlobArtifactStore, HostConfig, MemoryUploadIndex};
use axum::body::Body;
use axum::http::Request;
use norte_code_agent::Service;
use norte_code_protocol::InMemoryStore;
use norte_code_wire::{ClientRequest, ClientRequestKind};
use raep_types::{Envelope, FrameType, StandardFrameType};
use std::sync::Arc;
use tower::ServiceExt;

fn test_service() -> Arc<Service> {
    Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            None,
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    )
}

fn test_host(token: Option<&str>) -> Arc<NorteCodeRaepHost> {
    Arc::new(
        NorteCodeRaepHost::new(
            test_service(),
            HostConfig {
                auth_token: token.map(str::to_owned),
                ..HostConfig::default()
            },
        )
        .expect("host"),
    )
}

#[test]
fn byte_ranges_handle_closed_open_and_suffix_forms() {
    assert_eq!(parse_byte_range("bytes=2-4", 10), Ok(Some((2, 4))));
    assert_eq!(parse_byte_range("bytes=7-", 10), Ok(Some((7, 9))));
    assert_eq!(parse_byte_range("bytes=-3", 10), Ok(Some((7, 9))));
    assert!(parse_byte_range("bytes=10-", 10).is_err());
    assert!(parse_byte_range("bytes=0-1,4-5", 10).is_err());
}

#[tokio::test]
async fn export_download_is_authenticated_range_capable_bundle() {
    let host = Arc::new(
        NorteCodeRaepHost::new(
            test_service(),
            HostConfig {
                auth_principals: std::collections::HashMap::from([
                    ("token-a".to_owned(), "alice".to_owned()),
                    ("token-b".to_owned(), "bob".to_owned()),
                ]),
                ..HostConfig::default()
            },
        )
        .expect("authenticated host"),
    );
    let attached = host
        .handle_as(
            Envelope::new(
                FrameType::Standard(StandardFrameType::SessionAttach),
                json!({"agent_id": "norte-code", "create_if_missing": true}),
                1,
            ),
            Some("alice"),
        )
        .await
        .expect("attach");
    let session_id = attached[0].session_id.clone();
    let mut export = Envelope::new(
        FrameType::Standard(StandardFrameType::SessionExport),
        json!({"include": {
            "children": false,
            "uploads": false,
            "job_output": false,
            "outputs": false,
            "workspace": false
        }}),
        2,
    );
    export.session_id = session_id;
    let ready = host.handle_as(export, Some("alice")).await.expect("export");
    let export_id = ready[0].payload["export_id"].as_str().expect("export id");
    let app = router(Arc::clone(&host));
    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/exports/{export_id}"))
                .body(Body::empty())
                .expect("unauthorized request"),
        )
        .await
        .expect("unauthorized response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let wrong_owner = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/exports/{export_id}"))
                .header("authorization", "Bearer token-b")
                .body(Body::empty())
                .expect("wrong-owner request"),
        )
        .await
        .expect("wrong-owner response");
    assert_eq!(wrong_owner.status(), StatusCode::NOT_FOUND);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/exports/{export_id}"))
                .header("range", "bytes=0-9")
                .header("authorization", "Bearer token-a")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_RANGE],
        format!("bytes 0-9/{}", ready[0].payload["bytes"])
    );
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("range body");
    assert_eq!(body.len(), 10);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn text_bundle_preview_commit_restores_history_as_a_new_session() {
    let service = test_service();
    let source_key = service
        .import_plain_history(
            Some("source".to_owned()),
            vec![
                norte_code_agent::service::ImportedHistoryItem {
                    role: "user".to_owned(),
                    content: "question".to_owned(),
                    turn_id: Some("turn_1".to_owned()),
                    timestamp: Some(1),
                    origin: Some(json!({"kind": "user"})),
                },
                norte_code_agent::service::ImportedHistoryItem {
                    role: "assistant".to_owned(),
                    content: "answer".to_owned(),
                    turn_id: Some("turn_1".to_owned()),
                    timestamp: Some(2),
                    origin: Some(json!({"kind": "assistant"})),
                },
            ],
            json!({"session_id": "original"}),
            Some(json!({
                "input_tokens": 11,
                "output_tokens": 7,
                "cached_tokens": 3,
                "cost": {"amount": 0.25, "currency": "USD"},
                "extra": {"requests": 2}
            })),
        )
        .await
        .expect("seed source history");
    let host = Arc::new(
        NorteCodeRaepHost::new(
            service.clone(),
            HostConfig {
                auth_principals: std::collections::HashMap::from([
                    ("token-a".to_owned(), "alice".to_owned()),
                    ("token-b".to_owned(), "bob".to_owned()),
                ]),
                ..HostConfig::default()
            },
        )
        .expect("host"),
    );
    let source_id = source_key.to_string();
    let mut attach_source = Envelope::new(
        FrameType::Standard(StandardFrameType::SessionAttach),
        json!({"agent_id": "norte-code", "create_if_missing": false}),
        1,
    );
    attach_source.session_id = source_id.clone();
    host.handle_as(attach_source, Some("alice"))
        .await
        .expect("attach source");
    let mut export = Envelope::new(
        FrameType::Standard(StandardFrameType::SessionExport),
        json!({"include": {
            "children": false,
            "uploads": false,
            "job_output": false,
            "outputs": false,
            "workspace": false
        }}),
        2,
    );
    export.session_id = source_id.clone();
    let ready = host.handle_as(export, Some("alice")).await.expect("export");
    let export_id = ready[0].payload["export_id"].as_str().expect("export id");
    let (bundle, _) = host
        .get_export_as(export_id, "alice")
        .await
        .expect("bundle");
    let app = router(Arc::clone(&host));
    let preview = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/imports")
                .header("content-type", "application/vnd.raep.bundle+zip")
                .header("authorization", "Bearer token-a")
                .body(Body::from(bundle))
                .expect("preview request"),
        )
        .await
        .expect("preview response");
    assert_eq!(preview.status(), StatusCode::CREATED);
    let preview: Value = serde_json::from_slice(
        &axum::body::to_bytes(preview.into_body(), 1024 * 1024)
            .await
            .expect("preview body"),
    )
    .expect("preview JSON");
    let import_id = preview["import_id"].as_str().expect("import id");
    assert_eq!(preview["agents"][0]["available"], true);

    let wrong_owner_delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/raep/v1/imports/{import_id}"))
                .header("authorization", "Bearer token-b")
                .body(Body::empty())
                .expect("wrong-owner delete request"),
        )
        .await
        .expect("wrong-owner delete response");
    assert_eq!(wrong_owner_delete.status(), StatusCode::NOT_FOUND);

    let wrong_owner_commit = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/raep/v1/imports/{import_id}/commit"))
                .header("authorization", "Bearer token-b")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("wrong-owner commit request"),
        )
        .await
        .expect("wrong-owner commit response");
    assert_eq!(wrong_owner_commit.status(), StatusCode::NOT_FOUND);

    let committed = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/raep/v1/imports/{import_id}/commit"))
                .header("authorization", "Bearer token-a")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("commit request"),
        )
        .await
        .expect("commit response");
    assert_eq!(committed.status(), StatusCode::OK);
    let result: Value = serde_json::from_slice(
        &axum::body::to_bytes(committed.into_body(), 1024 * 1024)
            .await
            .expect("commit body"),
    )
    .expect("commit JSON");
    let imported_id = result["session_id"].as_str().expect("new session id");
    assert_ne!(imported_id, source_id);
    assert_eq!(result["id_map"][source_id.clone()], imported_id);
    assert_eq!(
        service
            .session_usage_total(&norte_code_wire::SessionKey::new(imported_id))
            .expect("carried usage"),
        json!({
            "input_tokens": 11,
            "output_tokens": 7,
            "cached_tokens": 3,
            "cost": {"amount": 0.25, "currency": "USD"},
            "extra": {"requests": 2, "unpriced_requests": 0}
        })
    );
    assert_eq!(
        host.session_summary(imported_id, "alice")
            .await
            .expect("suspended import summary")["status"],
        "SUSPENDED"
    );

    let mut attach = Envelope::new(
        FrameType::Standard(StandardFrameType::SessionAttach),
        json!({"agent_id": "norte-code", "create_if_missing": false}),
        3,
    );
    attach.session_id = imported_id.to_owned();
    let state = host
        .handle_as(attach, Some("alice"))
        .await
        .expect("attach imported session");
    assert_eq!(state[1].payload["imported_from"]["session_id"], source_id);
    let mut history_request = Envelope::new(
        FrameType::Standard(StandardFrameType::HistoryGet),
        json!({"limit": 10}),
        4,
    );
    history_request.session_id = imported_id.to_owned();
    let history = host
        .handle_as(history_request, Some("alice"))
        .await
        .expect("imported history");
    assert_eq!(history[0].payload["items"].as_array().unwrap().len(), 2);
    assert_eq!(history[0].payload["items"][0]["content"], "question");
    assert_eq!(history[0].payload["items"][0]["origin"]["kind"], "imported");
}

#[tokio::test]
async fn health_and_info_are_public_when_authentication_is_enabled() {
    let app = router(test_host(Some("secret")));
    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/raep/v1/health/live")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(health.status(), StatusCode::OK);
    let body = axum::body::to_bytes(health.into_body(), 1024)
        .await
        .expect("health body");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).expect("health"),
        json!({"status": "live"})
    );

    let info = app
        .oneshot(
            Request::builder()
                .uri("/raep/v1/info")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(info.status(), StatusCode::OK);
}

#[tokio::test]
async fn protected_artifact_routes_require_the_configured_token() {
    let app = router(test_host(Some("secret")));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/artifacts")
                .header("content-type", "text/plain")
                .body(Body::from("hello"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn extension_commands_return_remote_telemetry_and_tools() {
    let app = router(test_host(None));
    let request = ClientRequest {
        request_id: 1,
        kind: ClientRequestKind::ListTelemetry { session: None },
    };
    let frame = Envelope::new(
        FrameType::Extension(crate::NorteCodeExtension::Command.as_str().to_owned()),
        serde_json::to_value(request).expect("request"),
        1,
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/sessions/unused/commands")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&frame).expect("frame")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&body).expect("json");
    let events = body["events"].as_array().expect("events");
    assert_eq!(events[0]["payload"]["event"], "service_data");
    assert_eq!(events[0]["payload"]["kind"], "telemetry");

    let request = ClientRequest {
        request_id: 2,
        kind: ClientRequestKind::ListTools { session: None },
    };
    let frame = Envelope::new(
        FrameType::Extension(crate::NorteCodeExtension::Command.as_str().to_owned()),
        serde_json::to_value(request).expect("request"),
        2,
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/sessions/unused/commands")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&frame).expect("frame")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&body).expect("json");
    let events = body["events"].as_array().expect("events");
    assert_eq!(events[0]["payload"]["kind"], "tool_list");
}

#[tokio::test]
async fn session_uploads_are_authenticated_and_scoped() {
    let blobs = Arc::new(norte::InMemoryBlobStore::new());
    let host = Arc::new(
        NorteCodeRaepHost::new(
            test_service(),
            HostConfig {
                auth_principals: std::collections::HashMap::from([
                    ("token-a".to_owned(), "alice".to_owned()),
                    ("token-b".to_owned(), "bob".to_owned()),
                ]),
                ..HostConfig::default()
            },
        )
        .expect("authenticated host")
        .with_artifact_store(Arc::new(BlobArtifactStore::new(
            Arc::clone(&blobs) as Arc<dyn norte::BlobStore>
        )))
        .with_upload_index(Arc::new(MemoryUploadIndex::default())),
    );
    let attached = host
        .handle_as(
            Envelope::new(
                FrameType::Standard(StandardFrameType::SessionAttach),
                json!({"agent_id": "norte-code", "create_if_missing": true}),
                1,
            ),
            Some("alice"),
        )
        .await
        .expect("attach");
    let session_id = &attached[0].session_id;
    let app = router(host);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/raep/v1/sessions/{session_id}/uploads"))
                .header("authorization", "Bearer token-a")
                .header("content-type", "audio/wav")
                .body(Body::from(b"audio bytes".to_vec()))
                .expect("upload request"),
        )
        .await
        .expect("upload response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("upload body");
    let receipt: Value = serde_json::from_slice(&body).expect("upload receipt");
    assert!(
        receipt["upload_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("upl_"))
    );
    assert_eq!(receipt["mime_type"], "audio/wav");
    assert_eq!(receipt["bytes"], 11);

    let wrong_owner = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/raep/v1/sessions/{session_id}/uploads"))
                .header("authorization", "Bearer token-b")
                .header("content-type", "audio/wav")
                .body(Body::from(b"audio bytes".to_vec()))
                .expect("upload request"),
        )
        .await
        .expect("upload response");
    assert_eq!(wrong_owner.status(), StatusCode::NOT_FOUND);

    let missing_session = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/sessions/unknown/uploads")
                .header("authorization", "Bearer token-a")
                .header("content-type", "audio/wav")
                .body(Body::from(b"audio bytes".to_vec()))
                .expect("upload request"),
        )
        .await
        .expect("upload response");
    assert_eq!(missing_session.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browser_origin_must_be_on_the_allow_list() {
    let host = Arc::new(
        NorteCodeRaepHost::new(
            test_service(),
            HostConfig {
                allowed_origins: vec!["https://app.example".to_owned()],
                ..HostConfig::default()
            },
        )
        .expect("host"),
    );
    let app = router(host);
    let preflight = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/raep/v1/sessions")
                .header("origin", "https://app.example")
                .header("access-control-request-method", "POST")
                .header(
                    "access-control-request-headers",
                    "authorization,content-type",
                )
                .body(Body::empty())
                .expect("preflight request"),
        )
        .await
        .expect("preflight response");
    assert_eq!(preflight.status(), StatusCode::OK);
    assert_eq!(
        preflight.headers().get("access-control-allow-origin"),
        Some(&HeaderValue::from_static("https://app.example"))
    );

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/raep/v1/info")
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let allowed = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/raep/v1/info")
                .header("origin", "https://app.example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(
        allowed.headers().get("access-control-allow-origin"),
        Some(&HeaderValue::from_static("https://app.example"))
    );
}

#[tokio::test]
async fn websocket_requires_the_raep_v1_subprotocol() {
    let app = router(test_host(None));
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/raep/v1/ws")
                .header("connection", "upgrade")
                .header("upgrade", "websocket")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
}

#[tokio::test]
async fn events_stream_starts_with_hello_and_reads_last_event_id() {
    let app = router(test_host(None));
    let session = create_session(&app).await;
    let attach = SessionAttach {
        agent_id: "norte-code".to_owned(),
        agent_override: None,
        create_if_missing: Some(false),
        mode: Some(AttachMode::Observer),
        takeover: None,
        resume_from_seq: Some(0),
        capabilities: None,
        client_info: None,
        options: None,
    };
    let encoded_attach =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&attach).expect("serialize attach"));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/raep/v1/sessions/{session}/events?mode=controller&attach={encoded_attach}"
                ))
                .header("accept", "text/event-stream")
                .header("last-event-id", "0")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let mut text = String::new();
    for _ in 0..10 {
        let chunk = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            futures_util::StreamExt::next(&mut body),
        )
        .await
        .expect("frame timed out")
        .expect("body ended early")
        .expect("body error");
        text.push_str(&String::from_utf8_lossy(&chunk));
        if text.contains("event: HELLO") && text.contains("event: SESSION_ATTACHED") {
            break;
        }
    }
    assert!(
        text.contains("event: HELLO"),
        "stream must open with HELLO: {text}"
    );
    assert!(
        text.contains("event: SESSION_ATTACHED"),
        "stream must attach after HELLO: {text}"
    );
    assert!(
        text.contains("retry: 2000"),
        "stream must configure the reconnect delay: {text}"
    );
    assert!(
        text.contains(": keepalive"),
        "stream must start with the keepalive comment: {text}"
    );
    assert!(
        text.contains("\"mode\":\"observer\""),
        "encoded attach payload must take precedence over individual query parameters: {text}"
    );
    drop(body);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let reattached = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/raep/v1/sessions/{session}/events"))
                .header("accept", "text/event-stream")
                .body(Body::empty())
                .expect("reattach request"),
        )
        .await
        .expect("reattach response");
    assert_eq!(
        reattached.status(),
        StatusCode::OK,
        "closing an SSE stream must release its session attachment"
    );
}

#[tokio::test]
async fn events_stream_accepts_resume_from_seq() {
    let app = router(test_host(None));
    let session = create_session(&app).await;

    // A known cursor is accepted and the stream opens; reading the body
    // is not needed to prove the query was understood.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/raep/v1/sessions/{session}/events?resume_from_seq=0"
                ))
                .header("accept", "text/event-stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/raep/v1/sessions/{session}/events?resume_from_seq=abc"
                ))
                .header("accept", "text/event-stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn events_stream_can_allocate_a_session() {
    let app = router(test_host(None));
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/raep/v1/sessions/new/events?agent_id=norte-code")
                .header("accept", "text/event-stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let mut text = String::new();
    for _ in 0..10 {
        let chunk = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            futures_util::StreamExt::next(&mut body),
        )
        .await
        .expect("frame timed out")
        .expect("body ended early")
        .expect("body error");
        text.push_str(&String::from_utf8_lossy(&chunk));
        if text.contains("event: SESSION_ATTACHED") {
            break;
        }
    }
    let attached = text
        .split("event: SESSION_ATTACHED\n")
        .nth(1)
        .and_then(|event| event.lines().find(|line| line.starts_with("data: ")))
        .and_then(|data| serde_json::from_str::<Value>(&data[6..]).ok())
        .expect("SESSION_ATTACHED envelope");
    assert!(
        attached["session_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "allocation must return the new session id: {attached}"
    );
}

#[tokio::test]
async fn session_creation_uses_the_requested_agent() {
    let host = Arc::new(
        NorteCodeRaepHost::with_agents(
            std::collections::HashMap::from([
                ("norte-code".to_owned(), test_service()),
                ("alternate".to_owned(), test_service()),
            ]),
            HostConfig {
                agent_id: "norte-code".to_owned(),
                ..HostConfig::default()
            },
        )
        .expect("multi-agent host"),
    );
    let app = router(host);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "agent_id": "alternate",
                        "title": "Agent-specific session"
                    })
                    .to_string(),
                ))
                .expect("create request"),
        )
        .await
        .expect("create response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body");
    let session: Value = serde_json::from_slice(&body).expect("session response");
    assert_eq!(session["agent"]["id"], "alternate");
    assert_eq!(session["title"], "Agent-specific session");
}

#[tokio::test]
async fn agent_resource_lists_reads_and_versions() {
    let app = router(test_host(None));
    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/raep/v1/agents")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(list.status(), StatusCode::OK);
    let body = axum::body::to_bytes(list.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let list: Value = serde_json::from_slice(&body).expect("json");
    assert!(
        list["agents"]
            .as_array()
            .expect("agents")
            .iter()
            .any(|agent| agent["id"] == "norte-code")
    );

    let read = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/raep/v1/agents/norte-code")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(read.status(), StatusCode::OK);
    let body = axum::body::to_bytes(read.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let agent: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(agent["agent"]["id"], "norte-code");

    let versions = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/raep/v1/agents/norte-code/versions")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(versions.status(), StatusCode::OK);
    let body = axum::body::to_bytes(versions.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let versions: Value = serde_json::from_slice(&body).expect("json");
    assert_ne!(versions["versions"].as_array().expect("versions").len(), 0);

    let missing = app
        .oneshot(
            Request::builder()
                .uri("/raep/v1/agents/absent")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn session_resource_lists_reads_updates_and_deletes() {
    let app = router(test_host(None));
    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/sessions")
                .header("content-type", "application/json")
                .header("idempotency-key", "create-1")
                .body(Body::from(
                    r#"{"agent_id":"norte-code","title":"resource harness"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(create.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(create.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let summary: Value = serde_json::from_slice(&body).expect("json");
    let session = summary["session_id"]
        .as_str()
        .expect("session id")
        .to_owned();
    assert_eq!(summary["title"], "resource harness");
    assert_eq!(summary["status"], "ACTIVE");
    assert!(summary["created_at"].as_i64().is_some());

    let repeat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/sessions")
                .header("content-type", "application/json")
                .header("idempotency-key", "create-1")
                .body(Body::from(r#"{"agent_id":"norte-code"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(repeat.status(), StatusCode::OK);
    let body = axum::body::to_bytes(repeat.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let repeat: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(repeat["session_id"], session);

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/raep/v1/sessions")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(list.status(), StatusCode::OK);
    let body = axum::body::to_bytes(list.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let list: Value = serde_json::from_slice(&body).expect("json");
    assert!(
        list["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|entry| entry["session_id"] == session)
    );

    let read = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/sessions/{session}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(read.status(), StatusCode::OK);

    let patch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/raep/v1/sessions/{session}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":"renamed"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(patch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(patch.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let patched: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(patched["title"], "renamed");

    let unsupported = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/raep/v1/sessions/{session}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"pinned":true}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unsupported.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/raep/v1/sessions/{session}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    let missing = app
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/sessions/{session}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_session_is_hidden_from_another_principal() {
    let host = Arc::new(
        NorteCodeRaepHost::new(
            test_service(),
            HostConfig {
                auth_principals: std::collections::HashMap::from([
                    ("token-a".to_owned(), "alice".to_owned()),
                    ("token-b".to_owned(), "bob".to_owned()),
                ]),
                ..HostConfig::default()
            },
        )
        .expect("host"),
    );
    let app = router(host);
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/sessions")
                .header("authorization", "Bearer token-a")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"agent_id":"norte-code"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(created.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let summary: Value = serde_json::from_slice(&body).expect("json");
    let session = summary["session_id"]
        .as_str()
        .expect("session id")
        .to_owned();

    let stolen = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/sessions/{session}"))
                .header("authorization", "Bearer token-b")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(stolen.status(), StatusCode::NOT_FOUND);

    let list = app
        .oneshot(
            Request::builder()
                .uri("/raep/v1/sessions")
                .header("authorization", "Bearer token-b")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(list.status(), StatusCode::OK);
    let body = axum::body::to_bytes(list.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let list: Value = serde_json::from_slice(&body).expect("json");
    assert!(
        list["sessions"].as_array().expect("sessions").is_empty(),
        "a principal must not see another principal's sessions"
    );
}

#[tokio::test]
async fn child_session_list_is_scoped_to_parent_owner() {
    let host = Arc::new(
        NorteCodeRaepHost::new(
            test_service(),
            HostConfig {
                auth_principals: std::collections::HashMap::from([
                    ("token-a".to_owned(), "alice".to_owned()),
                    ("token-b".to_owned(), "bob".to_owned()),
                ]),
                ..HostConfig::default()
            },
        )
        .expect("host"),
    );
    let app = router(host);
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/sessions")
                .header("authorization", "Bearer token-a")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"agent_id":"norte-code"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(created.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let summary: Value = serde_json::from_slice(&body).expect("json");
    let session = summary["session_id"]
        .as_str()
        .expect("session id")
        .to_owned();

    let children = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/sessions/{session}/children"))
                .header("authorization", "Bearer token-a")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(children.status(), StatusCode::OK);
    let body = axum::body::to_bytes(children.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let children: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(children["children"], json!([]));

    let hidden_children = app
        .oneshot(
            Request::builder()
                .uri(format!("/raep/v1/sessions/{session}/children"))
                .header("authorization", "Bearer token-b")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(hidden_children.status(), StatusCode::NOT_FOUND);
}

async fn create_session(app: &Router) -> String {
    let request = ClientRequest {
        request_id: 1,
        kind: ClientRequestKind::CreateSession {
            title: Some("event harness".to_owned()),
        },
    };
    let frame = Envelope::new(
        FrameType::Extension(crate::NorteCodeExtension::Command.as_str().to_owned()),
        serde_json::to_value(request).expect("request"),
        1,
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/raep/v1/sessions/unused/commands")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&frame).expect("frame")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&body).expect("json");
    body["events"][0]["session_id"]
        .as_str()
        .expect("session id")
        .to_owned()
}
