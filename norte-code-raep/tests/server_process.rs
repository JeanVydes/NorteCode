//! End-to-end coverage for the installed RAEP server process.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use norte_code_wire::{ClientRequest, ClientRequestKind};
use raep_types::{Envelope, FrameType};

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "norte-code-raep-process-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temporary root");
    root
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("free port")
        .local_addr()
        .expect("address")
        .port()
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn server_process_serves_health_info_and_telemetry() {
    let root = temp_root();
    let config = root.join("config");
    let database = root.join("norte-code.db");
    let ready = root.join("ready");
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_norte-code-server"))
        .current_dir(&root)
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", &config)
        .env("NORTE_CODE_BIND", format!("127.0.0.1:{port}"))
        .env("NORTE_CODE_DATABASE", &database)
        .env("NORTE_CODE_READY_FILE", &ready)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start server process");

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if ready.exists() {
            break;
        }
        if Instant::now() >= deadline {
            stop(&mut child);
            panic!("server did not become ready");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let live = client
        .get(format!("{base}/raep/v1/health/live"))
        .send()
        .await
        .expect("live request");
    assert_eq!(live.status(), reqwest::StatusCode::OK);
    assert_eq!(
        live.json::<serde_json::Value>().await.expect("live json"),
        serde_json::json!({"status": "live"})
    );

    let ready_response = client
        .get(format!("{base}/raep/v1/health/ready"))
        .send()
        .await
        .expect("ready request");
    assert_eq!(ready_response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        ready_response
            .json::<serde_json::Value>()
            .await
            .expect("ready json")["status"],
        "ready"
    );

    let info = client
        .get(format!("{base}/raep/v1/info"))
        .send()
        .await
        .expect("info request");
    assert_eq!(info.status(), reqwest::StatusCode::OK);
    let info = info.json::<serde_json::Value>().await.expect("info json");
    assert_eq!(info["protocol"]["name"], "RAEP");
    assert!(info["agents"].as_array().is_some());

    let telemetry = client
        .get(format!("{base}/raep/v1/telemetry"))
        .send()
        .await
        .expect("telemetry request");
    assert_eq!(telemetry.status(), reqwest::StatusCode::OK);
    let telemetry = telemetry
        .json::<serde_json::Value>()
        .await
        .expect("telemetry json");
    assert_eq!(telemetry["summary"]["requests"], 0);
    assert!(telemetry["records"].as_array().is_some());

    let create = Envelope::new(
        FrameType::Extension("norte_code.command".to_owned()),
        serde_json::to_value(ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::CreateSession {
                title: Some("process harness".to_owned()),
            },
        })
        .expect("create payload"),
        1,
    );
    let create = client
        .post(format!("{base}/raep/v1/sessions/unused/commands"))
        .json(&create)
        .send()
        .await
        .expect("create session");
    assert_eq!(create.status(), reqwest::StatusCode::ACCEPTED);
    let create = create
        .json::<serde_json::Value>()
        .await
        .expect("create response");
    let session = create["events"][0]["session_id"]
        .as_str()
        .expect("created session id")
        .to_owned();

    let events = client
        .get(format!("{base}/raep/v1/sessions/{session}/events"))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await
        .expect("open SSE");
    assert_eq!(events.status(), reqwest::StatusCode::OK);
    let mut event_stream = events.bytes_stream();
    let query = Envelope::new(
        FrameType::Extension("norte_code.command".to_owned()),
        serde_json::to_value(ClientRequest {
            request_id: 2,
            kind: ClientRequestKind::ListTelemetry {
                session: Some(norte_code_wire::SessionKey::new(session.clone())),
            },
        })
        .expect("telemetry payload"),
        2,
    );
    let response = client
        .post(format!("{base}/raep/v1/sessions/{session}/commands"))
        .json(&query)
        .send()
        .await
        .expect("send telemetry command");
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let mut sse = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let Some(chunk) = tokio::time::timeout(Duration::from_millis(500), event_stream.next())
            .await
            .expect("SSE chunk timeout")
            .transpose()
            .expect("SSE stream")
        else {
            break;
        };
        sse.extend_from_slice(&chunk);
        if sse
            .windows(b"telemetry".len())
            .any(|window| window == b"telemetry")
        {
            break;
        }
    }
    let sse = String::from_utf8_lossy(&sse);
    assert!(
        sse.contains("telemetry"),
        "SSE did not carry telemetry: {sse}"
    );

    stop(&mut child);
    let _ = std::fs::remove_dir_all(root);
}
