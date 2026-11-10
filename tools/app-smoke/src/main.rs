//! A headless driver for the conversation page.
//!
//! The page is Dioxus and runs in a browser; what it *does* is a conversation:
//! open a session's stream, read frames, fold them into rows, and send input
//! back. This binary runs that same path against a live server so the wire
//! contract the page depends on is exercised without a browser.
//!
//! Usage: `norte-code-app-smoke <base-url> [--session <id>] [--prompt <text>]`

use std::time::Duration;

use serde_json::Value;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    // `--scenario` runs a named flow instead of one prompt: the point of this
    // binary is to exercise the paths the page shows, not just one answer.
    let scenario = flag(&args, "--scenario");
    let runs = match scenario.as_deref() {
        Some("tools") => vec![(
            "Use the todo tool with 3 items, then read Cargo.toml and report done.",
            Expectations {
                tools: true,
                todos: true,
                text: true,
                question: false,
                jobs: false,
                job_turn: false,
            },
        )],
        Some("jobturn") => vec![(
            "Run in background with detach: sleep 4; echo job detached completado. When it finishes, tell me the output.",
            Expectations {
                jobs: true,
                text: true,
                job_turn: true,
                ..Expectations::none()
            },
        )],
        Some("export") => vec![(
            "Use the todo tool with 2 items, then say done",
            Expectations {
                tools: true,
                text: true,
                ..Expectations::none()
            },
        )],
        Some("jobs") => vec![(
            "Run in the background with detach: for i in 1 2 3 4 5; do echo out$i; sleep 2; done",
            Expectations {
                jobs: true,
                text: true,
                ..Expectations::none()
            },
        )],
        Some("ask") => vec![(
            "Call ask_user to ask me which color I prefer, offering red and blue.",
            Expectations {
                question: true,
                text: false,
                ..Expectations::none()
            },
        )],
        _ => vec![("Reply with exactly: smoke ok", Expectations::text_only())],
    };
    for (prompt, expected) in runs {
        run(&args, prompt, &expected);
    }
}

fn run(args: &[String], prompt: &str, expected: &Expectations) {
    let args = args.to_vec();
    let base = args
        .first()
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:8841".to_owned());
    let session = flag(&args, "--session");
    let prompt = prompt.to_owned();
    let agent = flag(&args, "--agent").unwrap_or_else(|| "norte-code".to_owned());

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async move {
        let client = reqwest::Client::new();
        let base = base.trim_end_matches('/').to_owned();
        let session_id = match session {
            Some(id) => id,
            None => create_session(&client, &base).await,
        };
        println!("session {session_id}");

        let mut events = EventReader::new(open_stream(&client, &base, &session_id, &agent).await);
        send_prompt(&client, &base, &session_id, &prompt).await;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        let mut seen = Counts::default();
        while tokio::time::Instant::now() < deadline {
            let Some(frame) = events.next().await else {
                break;
            };
            seen.observe(&frame);
            // A parked turn does not finish, so waiting for the end would hide
            // the question the scenario is here to see. A job-triggered turn
            // finishes twice: the first end is not the one worth stopping at.
            let job_turn_pending = expected.job_turn && seen.accepted < 2;
            if (seen.finished && !job_turn_pending) || (expected.question && seen.questions > 0) {
                break;
            }
        }
        seen.report();
        if args.iter().any(|argument| argument == "--scenario")
            && flag(&args, "--scenario").as_deref() == Some("export")
        {
            export_and_import(&client, &base, &session_id).await;
        }
        assert!(
            seen.user_input_accepted,
            "the server never accepted the input"
        );
        if expected.text {
            assert!(seen.token_deltas > 0, "no answer text arrived");
        }
        if expected.tools {
            assert!(
                seen.tool_started > 0 && seen.tool_started == seen.tool_finished,
                "tools started {} but finished {}",
                seen.tool_started,
                seen.tool_finished
            );
        }
        if expected.todos {
            assert!(seen.todos > 0, "the todo list never arrived");
        }
        if expected.question {
            assert!(seen.questions > 0, "the question never arrived");
        }
        if expected.job_turn {
            // The screenshot bug: the job finished, and the page stayed on
            // "Thinking…" because the auto-turn's events were drained by the
            // service and never forwarded. Two accepted inputs prove the
            // injected message arrived; the text assertion above proves the
            // answer that followed did too.
            assert!(
                seen.accepted >= 2,
                "the injected job message never reached the client (accepted={})",
                seen.accepted
            );
            assert!(
                seen.deltas_after_job > 0,
                "the model's answer after the job never reached the client"
            );
        }
        if expected.jobs {
            // A detached call must announce itself on the session's own
            // stream; it used to be emitted under the engine's session id and
            // dropped, so a client only ever saw the finished job.
            assert!(
                seen.jobs_started > 0,
                "JOB_STARTED never reached the session stream"
            );
            assert!(
                seen.job_progress > 0,
                "JOB_PROGRESS_DELTA never reached the session stream"
            );
        }
        if expected.question {
            // A parked turn does not finish; it is waiting on the person.
            assert!(
                !seen.states.iter().any(|state| state == "COMPLETED"),
                "a turn parked on a question reported COMPLETED"
            );
        } else {
            assert!(seen.finished, "the turn never reported an end");
        }
    });
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let index = args.iter().position(|argument| argument == name)?;
    args.get(index + 1).cloned()
}

/// What a scenario has to prove.
#[allow(clippy::struct_excessive_bools)]
struct Expectations {
    text: bool,
    tools: bool,
    todos: bool,
    question: bool,
    jobs: bool,
    /// A finished job must start its own turn and deliver it live.
    job_turn: bool,
}

impl Expectations {
    fn none() -> Self {
        Self {
            text: false,
            tools: false,
            todos: false,
            question: false,
            jobs: false,
            job_turn: false,
        }
    }

    fn text_only() -> Self {
        Self {
            text: true,
            ..Self::none()
        }
    }
}

#[derive(Default)]
struct Counts {
    user_input_accepted: bool,
    /// How many inputs the host accepted: a finished job injects a second one.
    accepted: usize,
    /// Answer deltas that arrived after the job's injected message.
    deltas_after_job: usize,
    token_deltas: usize,
    tool_started: usize,
    tool_finished: usize,
    todos: usize,
    questions: usize,
    jobs_started: usize,
    job_progress: usize,
    usage: usize,
    finished: bool,
    states: Vec<String>,
}

impl Counts {
    fn observe(&mut self, frame: &Value) {
        let kind = frame["type"].as_str().unwrap_or_default();
        match kind {
            "USER_INPUT_ACCEPTED" => {
                self.user_input_accepted = true;
                self.accepted += 1;
            }
            "TOKEN_DELTA" => {
                self.token_deltas += 1;
                if self.accepted >= 2 {
                    self.deltas_after_job += 1;
                }
            }
            "TOOL_CALL_STARTED" => self.tool_started += 1,
            "TOOL_CALL_FINISHED" => self.tool_finished += 1,
            "TURN_STATE_CHANGED" => {
                let state = frame["payload"]["state"].as_str().unwrap_or_default();
                self.states.push(state.to_owned());
                if matches!(state, "COMPLETED" | "FAILED" | "STOPPED" | "CANCELLED") {
                    self.finished = true;
                }
            }
            _ if kind.ends_with("session_todos") => self.todos += 1,
            "JOB_STARTED" => self.jobs_started += 1,
            "JOB_PROGRESS_DELTA" => self.job_progress += 1,
            _ if kind.ends_with("question_asked") => self.questions += 1,
            _ if kind.ends_with("request_completed") => self.usage += 1,
            _ => {}
        }
    }

    fn report(&self) {
        println!(
            "accepted={} ({}) deltas_after_job={} deltas={} tools={}/{} todos={} questions={} jobs={}/{} usage={} finished={} states={:?}",
            self.user_input_accepted,
            self.accepted,
            self.deltas_after_job,
            self.token_deltas,
            self.tool_started,
            self.tool_finished,
            self.todos,
            self.questions,
            self.jobs_started,
            self.job_progress,
            self.usage,
            self.finished,
            self.states
        );
    }
}

/// Exports the session, imports the bundle back, and commits it.
///
/// A conversation with tool calls could not be exported at all until the
/// bundle learned to carry them as text; this is the regression check.
async fn export_and_import(client: &reqwest::Client, base: &str, session_id: &str) {
    let body = serde_json::json!({
        "version": "1.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "session_id": session_id,
        "timestamp": 0,
        "type": "SESSION_EXPORT",
        "payload": { "include": {
            "children": false, "uploads": false, "job_output": false,
            "outputs": false, "workspace": false
        }},
    });
    let reply = client
        .post(format!("{base}/raep/v1/sessions/{session_id}/commands"))
        .json(&body)
        .send()
        .await
        .expect("export request")
        .json::<Value>()
        .await
        .expect("export reply");
    let export_id = reply["events"]
        .as_array()
        .and_then(|events| {
            events
                .iter()
                .find_map(|event| event["payload"]["export_id"].as_str())
        })
        .unwrap_or_else(|| panic!("no export id in {reply}"));
    let bundle = client
        .get(format!("{base}/raep/v1/exports/{export_id}"))
        .send()
        .await
        .expect("download export")
        .bytes()
        .await
        .expect("bundle bytes");
    assert!(!bundle.is_empty(), "the exported bundle is empty");
    let imported = client
        .post(format!("{base}/raep/v1/imports"))
        .header("content-type", "application/vnd.raep.bundle+zip")
        .body(bundle)
        .send()
        .await
        .expect("import request")
        .json::<Value>()
        .await
        .expect("import reply");
    let import_id = imported["import_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no import id in {imported}"))
        .to_owned();
    let committed = client
        .post(format!("{base}/raep/v1/imports/{import_id}/commit"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("commit request");
    assert!(
        committed.status().is_success(),
        "import commit failed: {}",
        committed.status()
    );
    println!("export/import roundtrip ok");
}

async fn create_session(client: &reqwest::Client, base: &str) -> String {
    let response = client
        .post(format!("{base}/raep/v1/sessions"))
        .json(&serde_json::json!({ "agent_id": "norte-code", "title": "smoke" }))
        .send()
        .await
        .expect("create session")
        .json::<Value>()
        .await
        .expect("session json");
    response["session_id"]
        .as_str()
        .expect("session id")
        .to_owned()
}

async fn open_stream(
    client: &reqwest::Client,
    base: &str,
    session_id: &str,
    agent: &str,
) -> reqwest::Response {
    client
        .get(format!(
            "{base}/raep/v1/sessions/{session_id}/events?agent_id={agent}"
        ))
        .header("accept", "text/event-stream")
        .send()
        .await
        .expect("open stream")
}

async fn send_prompt(client: &reqwest::Client, base: &str, session_id: &str, prompt: &str) {
    let body = serde_json::json!({
        "version": "1.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "session_id": session_id,
        "turn_id": uuid::Uuid::new_v4().to_string(),
        "timestamp": 0,
        "type": "USER_INPUT",
        "payload": { "prompt": prompt, "queue_policy": "reject" },
    });
    let reply = client
        .post(format!("{base}/raep/v1/sessions/{session_id}/commands"))
        .json(&body)
        .send()
        .await
        .expect("send prompt")
        .json::<Value>()
        .await
        .expect("reply json");
    assert_eq!(
        reply["accepted"], true,
        "the host refused the input: {reply}"
    );
}

/// Reads SSE frames incrementally, keeping whatever part of a frame was cut
/// across two chunks.
struct EventReader {
    response: reqwest::Response,
    buffer: String,
}

impl EventReader {
    fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            buffer: String::new(),
        }
    }

    /// The next frame's `data:` payload, waiting for more bytes when the
    /// buffer does not hold a whole one.
    async fn next(&mut self) -> Option<Value> {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let frame = self.buffer[..end].to_owned();
                self.buffer.drain(..end + 2);
                if let Some(payload) = frame
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim_start)
                    .find_map(|data| serde_json::from_str::<Value>(data).ok())
                {
                    return Some(payload);
                }
                continue;
            }
            let chunk = self.response.chunk().await.ok()??;
            self.buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
}
