//! A host that answers what the attachment tests need.
//!
//! The tests need a server that refuses a command once with the refusal the
//! host writes when a session is not attached, and answers everything else.
//! A small HTTP server built on the runtime keeps the client free of a test
//! dependency.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// The refusals and answers the host gives.
#[derive(Clone, Default)]
pub(crate) struct MockHost {
    /// Where the host listens.
    pub(crate) base_url: String,
    /// How many attachments the host received.
    pub(crate) attaches: Arc<AtomicUsize>,
    /// How many commands the host received.
    pub(crate) commands: Arc<AtomicUsize>,
}

/// Starts a host that refuses the first command with the refusal the host
/// writes when a session is not attached.
pub(crate) async fn start() -> MockHost {
    serve(Answer::RefuseOnce).await
}

/// Starts a host that refuses every command the same way.
pub(crate) async fn start_always_refusing() -> MockHost {
    serve(Answer::RefuseAlways).await
}

/// Starts a host that lists background jobs and stops them.
pub(crate) async fn start_with_jobs() -> MockHost {
    serve(Answer::Jobs).await
}

/// What the mock host answers.
#[derive(Clone, Copy)]
enum Answer {
    /// Refuse the first command, then answer it.
    RefuseOnce,
    /// Refuse every command.
    RefuseAlways,
    /// Answer job commands and refuse the rest.
    Jobs,
}

async fn serve(answer: Answer) -> MockHost {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock host");
    let address = listener.local_addr().expect("mock host address");
    let host = MockHost {
        base_url: format!("http://{address}"),
        attaches: Arc::new(AtomicUsize::new(0)),
        commands: Arc::new(AtomicUsize::new(0)),
    };
    let attaches = Arc::clone(&host.attaches);
    let commands = Arc::clone(&host.commands);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let attaches = Arc::clone(&attaches);
            let commands = Arc::clone(&commands);
            tokio::spawn(async move {
                let request = read_request(&mut stream).await;
                let answer = if request.contains("SESSION_ATTACH") {
                    attaches.fetch_add(1, Ordering::SeqCst);
                    response(200, r#"{"events":[]}"#)
                } else if request.contains("JOB_LIST") {
                    response(200, &job_list())
                } else if request.contains("CANCEL_JOB") {
                    response(200, &job_finished())
                } else {
                    let seen = commands.fetch_add(1, Ordering::SeqCst);
                    if matches!(answer, Answer::RefuseAlways) || seen == 0 {
                        response(422, &not_attached())
                    } else {
                        response(200, r#"{"events":[]}"#)
                    }
                };
                let _ = stream.write_all(&answer).await;
                let _ = stream.flush().await;
            });
        }
    });
    host
}

/// The job list the host answers.
fn job_list() -> String {
    serde_json::json!({
        "events": [{
            "version": "1.0",
            "id": "frame-1",
            "session_id": "session-1",
            "timestamp": 0,
            "type": "JOB_LIST_RESULT",
            "payload": {
                "jobs": [{
                    "job_id": "job-1",
                    "command": "cargo build",
                    "status": "RUNNING",
                }],
            },
        }],
    })
    .to_string()
}

/// The finished job the host answers.
fn job_finished() -> String {
    serde_json::json!({
        "events": [{
            "version": "1.0",
            "id": "frame-2",
            "session_id": "session-1",
            "timestamp": 0,
            "type": "JOB_FINISHED",
            "payload": {
                "job_id": "job-1",
                "summary": "cargo build",
                "status": "KILLED",
                "duration_ms": 1_200,
                "output_snapshot": "compiling",
                "output_truncated": false,
                "output_bytes_total": 9,
                "triggered_new_turn": false,
            },
        }],
    })
    .to_string()
}

/// The refusal the host writes when a session is not attached.
fn not_attached() -> String {
    serde_json::json!({
        "version": "1.0",
        "id": "frame-1",
        "session_id": "session-1",
        "timestamp": 0,
        "type": "ERROR",
        "payload": {
            "code": "INVALID_FRAME",
            "message": "invalid frame: session is not attached",
            "retryable": false,
        },
    })
    .to_string()
}

fn response(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} Answer\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

async fn read_request(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    while let Ok(read) = stream.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(headers_end) = headers_end(&buffer) {
            let headers = String::from_utf8_lossy(&buffer[..headers_end]).to_lowercase();
            let length = headers
                .split("\r\n")
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if buffer.len() >= headers_end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buffer).to_string()
}

fn headers_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
