use super::compaction::recent_start;
use super::mapping::retry_plan;
use super::sandbox::sandbox_name;
use super::*;
use norte::{FinishReason, Role};
use std::collections::BTreeSet;
use std::path::Path;

use crate::provider::Wire;
use async_trait::async_trait;
use futures_util::StreamExt;
use norte::{
    CallId, Capabilities, InMemoryBlobStore, InMemoryStore, Interception, InterceptionContext,
    Message, ModelId, ModelRef, ModelRequest, ModelStream, ProviderErrorKind, ProviderId,
    Reasoning as ReasoningCapability, ReasoningEffort, ReasoningRequest, SessionId,
    TurnInterceptor, Usage,
};
use norte_code_config::{ApiKey, BudgetConfig, RetryConfig};
use norte_code_protocol::{
    Budget, BudgetUnit, ClientRequest, ClientRequestKind, Delta, Reasoning as ProtocolReasoning,
    ReasoningScope,
};
use norte_code_store_sqlite::SqliteStore;
use norte_testing::ScriptedModel;
use norte_tools::CommandRequest;

struct ScriptedFactory {
    model: Arc<dyn Model>,
}

impl ModelFactory for ScriptedFactory {
    fn build(
        &self,
        _spec: &ModelSpec,
        _capabilities: &Capabilities,
    ) -> Result<Arc<dyn Model>, ServiceError> {
        Ok(Arc::clone(&self.model) as Arc<dyn Model>)
    }
}

/// A model that fails its first `failures` requests with a transient
/// provider error, then delegates to the scripted model.
struct FlakyModel {
    inner: Arc<ScriptedModel>,
    failures: std::sync::atomic::AtomicUsize,
    retry_after: Option<Duration>,
    kind: ProviderErrorKind,
}

impl FlakyModel {
    fn new(
        inner: Arc<ScriptedModel>,
        failures: usize,
        retry_after: Option<Duration>,
        kind: ProviderErrorKind,
    ) -> Self {
        Self {
            inner,
            failures: std::sync::atomic::AtomicUsize::new(failures),
            retry_after,
            kind,
        }
    }
}

#[async_trait]
impl Model for FlakyModel {
    fn reference(&self) -> &norte::ModelRef {
        self.inner.reference()
    }

    fn capabilities(&self) -> &Capabilities {
        self.inner.capabilities()
    }

    fn continuity(&self) -> &dyn norte::ContinuityAdapter {
        self.inner.continuity()
    }

    async fn stream(&self, request: ModelRequest) -> norte::Result<ModelStream> {
        let left = self.failures.load(std::sync::atomic::Ordering::SeqCst);
        if left > 0 {
            self.failures
                .store(left - 1, std::sync::atomic::Ordering::SeqCst);
            let mut error = norte::ProviderError::new(self.kind, "slow down");
            if let Some(after) = self.retry_after {
                error = error.with_retry_after(after);
            }
            return Err(norte::Error::provider(error));
        }
        self.inner.stream(request).await
    }
}

fn catalog() -> Arc<Catalog> {
    Arc::new(
        Catalog::parse(
            br#"{
                    "providers": {
                        "test": {
                            "id": "test",
                            "env": ["TEST_API_KEY"],
                            "npm": "@ai-sdk/openai-compatible",
                            "api": "https://example.invalid/v1",
                            "models": {
                                "scripted": {
                                    "id": "scripted",
                                    "name": "Scripted",
                                    "reasoning": true,
                                    "reasoning_options": [
                                        {"type": "effort", "values": ["low", "medium", "high", "xhigh", "max"]},
                                        {"type": "budget_tokens", "max": 4096}
                                    ],
                                    "tool_call": true,
                                    "modalities": {"input": ["text"], "output": ["text"]},
                                    "limit": {"context": 1000, "output": 100}
                                },
                                "other": {
                                    "id": "other",
                                    "name": "Other",
                                    "tool_call": true,
                                    "modalities": {"input": ["text"], "output": ["text"]},
                                    "limit": {"context": 2000, "output": 200}
                                }
                            }
                        }
                    }
                }"#,
        )
        .expect("catalog"),
    )
}

fn spec(model: &str) -> ModelSpec {
    let catalog = catalog();
    let entry = catalog.find("test", model).expect("model");
    ModelSpec {
        provider: "test".to_owned(),
        model: model.to_owned(),
        wire: Wire::Chat,
        endpoint: Some("https://example.invalid/v1".to_owned()),
        api_key: ApiKey::new("test"),
        auth: None,
        capabilities: norte_code_catalog::capabilities(entry, Wire::Chat, None),
        headers: std::collections::BTreeMap::new(),
    }
}

fn config() -> Config {
    let key_file =
        std::env::temp_dir().join(format!("norte-code-service-{}-key", std::process::id()));
    std::fs::write(&key_file, "sk-test\n").expect("key file");
    Config::from_json(&format!(
        r#"{{"providers":{{"test":{{"api_key_file":"{}"}}}}}}"#,
        key_file.display()
    ))
    .expect("config")
    .config
}

fn service() -> Arc<Service> {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_deltas(["Hel", "lo"]));
    Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    )
}

/// A service whose scripted model answers one text per queued turn.
fn scripted_service(spec: ModelSpec, turns: &[&str]) -> (Arc<Service>, Arc<ScriptedModel>) {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let mut model = ScriptedModel::new(reference);
    for turn in turns {
        model = model.turn_text(*turn);
    }
    let model = Arc::new(model);
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec,
            Arc::new(ScriptedFactory {
                model: Arc::clone(&model) as Arc<dyn Model>,
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    (service, model)
}

fn user_texts(request: &ModelRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|message| message.role == Role::User)
        .map(norte::Message::text)
        .collect()
}

async fn persistent_scripted_service(
    spec: ModelSpec,
    model: Arc<dyn Model>,
    store: Arc<SqliteStore>,
) -> Arc<Service> {
    Arc::new(
        Service::with_directory(
            config(),
            catalog(),
            spec,
            Arc::new(ScriptedFactory { model }),
            store.clone(),
            store.clone(),
            store,
            "system",
        )
        .await
        .expect("service"),
    )
}

async fn drain(
    service: &Arc<Service>,
    kind: ClientRequestKind,
) -> Vec<norte_code_protocol::ServerEvent> {
    service
        .submit(ClientRequest {
            request_id: 7,
            kind,
        })
        .await
        .unwrap_or_else(|error| panic!("request failed: {error}"))
        .collect()
        .await
}

#[tokio::test]
async fn a_turn_streams_text_and_commits() {
    let service = service();
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "hi".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let mut text = String::new();
    let mut requests = 0;
    let mut committed = false;
    for event in &events {
        assert_eq!(event.request_id, 7);
        assert!(event.session.is_some());
        match &event.kind {
            ServerEventKind::Delta {
                delta: Delta::Text { text: chunk, .. },
                ..
            } => text.push_str(chunk),
            ServerEventKind::RequestReconstructed { .. } => requests += 1,
            ServerEventKind::TurnFinished {
                committed: done, ..
            } => committed = *done,
            _ => {}
        }
    }
    assert_eq!(text, "Hello");
    assert_eq!(requests, 1);
    assert!(committed);
    assert!(matches!(
        events.first().map(|event| &event.kind),
        Some(ServerEventKind::TurnStarted { .. })
    ));
}

#[tokio::test]
async fn a_transient_failure_retries_the_same_session() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let scripted = Arc::new(ScriptedModel::new(reference).turn_text("after retry"));
    let model = Arc::new(FlakyModel::new(
        Arc::clone(&scripted),
        1,
        Some(Duration::from_millis(1)),
        ProviderErrorKind::RateLimited,
    ));
    let mut config = config();
    config.retry.max_attempts = 1;
    config.retry.max_delay_ms = 10;
    config.retry.backoff_ms = 1;
    let (service, _store) = persistent_service_with_config(config, model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let retries: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            ServerEventKind::RetryScheduled {
                attempt,
                delay_ms,
                reason,
                ..
            } => Some((*attempt, *delay_ms, reason.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0].0, 1);
    assert_eq!(retries[0].1, 1);
    assert!(retries[0].2.contains("slow down"));
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            ServerEventKind::RetryStarted { attempt, .. } => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec![1]);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::Error { .. }))
    );
    let mut text = String::new();
    for event in &events {
        if let ServerEventKind::Delta {
            delta: Delta::Text { text: chunk, .. },
            ..
        } = &event.kind
        {
            text.push_str(chunk);
        }
    }
    assert_eq!(text, "after retry");
    // The failed attempt never reached the scripted model, and the retry
    // carried the user message exactly once.
    let requests = scripted.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .messages
            .iter()
            .filter(|message| message.role == Role::User)
            .count(),
        1
    );
    assert!(matches!(
        events.last().map(|event| &event.kind),
        Some(ServerEventKind::TurnFinished {
            committed: true,
            ..
        })
    ));
}

#[tokio::test]
#[cfg(unix)]
async fn cancelling_a_turn_stops_a_running_command() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(ScriptedModel::new(reference).turn(vec![
        norte::StreamEvent::ToolCallStart {
            index: 0,
            call_id: call,
            name: "command".to_owned(),
        },
        norte::StreamEvent::ToolCallDelta {
            index: 0,
            call_id: call,
            name: None,
            arguments: serde_json::json!({
                "command": {"kind": "program", "program": "sleep", "args": ["30"]}
            })
            .to_string(),
        },
        norte::StreamEvent::Finish(norte::FinishReason::ToolCalls),
    ]));
    let mut config = config();
    config.approval_mode = norte_code_config::ApprovalMode::Yolo;
    let (service, _store) = persistent_service_with_config(config, model).await;
    let mut events = service
        .submit(ClientRequest {
            request_id: 7,
            kind: ClientRequestKind::SendMessage {
                session: None,
                text: "run it".to_owned(),
                budget: None,
                reasoning: None,
            },
        })
        .await
        .expect("turn");

    let mut key = None;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.next().await {
            if let Some(session) = event.session {
                key = Some(session);
            }
            if matches!(event.kind, ServerEventKind::ToolStarted { .. }) {
                break;
            }
        }
    })
    .await
    .expect("the command starts");

    let key = key.expect("session");
    let _ = service
        .submit(ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::CancelTurn { session: key },
        })
        .await
        .expect("cancel");

    let rest = tokio::time::timeout(Duration::from_secs(5), events.collect::<Vec<_>>())
        .await
        .expect("the cancelled turn settles");
    assert!(
        rest.iter()
            .any(|event| matches!(event.kind, ServerEventKind::TurnFinished { .. })),
        "a cancelled command must let the turn finish"
    );
}

#[tokio::test]
async fn retries_report_increasing_attempts_and_backoff() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let scripted = Arc::new(ScriptedModel::new(reference).turn_text("after retries"));
    let model = Arc::new(FlakyModel::new(
        Arc::clone(&scripted),
        2,
        None,
        ProviderErrorKind::Transport,
    ));
    let mut config = config();
    config.retry.max_attempts = 2;
    config.retry.max_delay_ms = 10;
    config.retry.backoff_ms = 1;
    let (service, _store) = persistent_service_with_config(config, model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let retries: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            ServerEventKind::RetryScheduled {
                attempt, delay_ms, ..
            } => Some((*attempt, *delay_ms)),
            _ => None,
        })
        .collect();
    assert_eq!(retries, vec![(1, 1), (2, 2)]);
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            ServerEventKind::RetryStarted { attempt, .. } => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec![1, 2]);
    assert!(matches!(
        events.last().map(|event| &event.kind),
        Some(ServerEventKind::TurnFinished {
            committed: true,
            ..
        })
    ));
}

#[tokio::test]
async fn a_permanent_failure_is_not_retried() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let scripted = Arc::new(ScriptedModel::new(reference).turn_text("unused"));
    let model = Arc::new(FlakyModel::new(
        Arc::clone(&scripted),
        1,
        None,
        ProviderErrorKind::QuotaExceeded,
    ));
    let mut config = config();
    config.retry.max_attempts = 2;
    let (service, _store) = persistent_service_with_config(config, model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::RetryScheduled { .. }))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, ServerEventKind::Error { .. }))
            .count(),
        1
    );
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::TurnFinished {
            committed: false,
            ..
        }
    )));
    assert!(scripted.requests().is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn the_command_sandbox_wraps_the_original_request_without_losing_it() {
    let workspace = PathBuf::from("/work");
    let arguments = serde_json::json!({
        "command": {"kind": "shell", "shell": "sh", "script": "echo hi"},
        "cwd": "/work/sub",
        "env": {"A": "B"},
        "timeout_ms": 1_000
    });
    let wrapped = sandboxed_arguments(&arguments, &workspace, false, &[PathBuf::from("/cache")])
        .expect("wrap");
    let command = wrapped
        .get("command")
        .and_then(serde_json::Value::as_object)
        .expect("command");
    assert_eq!(command["kind"], "program");
    assert_eq!(command["program"], "bwrap");
    let args: Vec<&str> = command["args"]
        .as_array()
        .expect("args")
        .iter()
        .map(|value| value.as_str().expect("string"))
        .collect();
    assert!(args.contains(&"--unshare-all"));
    assert!(args.contains(&"--cap-drop"));
    assert!(!args.contains(&"--share-net"));
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--chdir" && pair[1] == "/work/sub")
    );
    assert!(
        args.windows(3)
            .any(|pair| pair[0] == "--bind" && pair[1] == "/cache" && pair[2] == "/cache")
    );
    assert_eq!(&args[args.len() - 4..], ["--", "sh", "-c", "echo hi"]);
    assert_eq!(wrapped["cwd"], "/work/sub");
    assert_eq!(wrapped["env"]["A"], "B");
    assert_eq!(wrapped["timeout_ms"], 1_000);

    let networked = sandboxed_arguments(&arguments, &workspace, true, &[]).expect("networked");
    let args = networked["command"]["args"].as_array().expect("args");
    assert!(
        args.iter()
            .any(|value| value.as_str() == Some("--share-net"))
    );

    let program = serde_json::json!({
        "command": {"kind": "program", "program": "cargo", "args": ["test", "--all"]}
    });
    let wrapped = sandboxed_arguments(&program, &workspace, false, &[]).expect("program");
    let args: Vec<&str> = wrapped["command"]["args"]
        .as_array()
        .expect("args")
        .iter()
        .map(|value| value.as_str().expect("string"))
        .collect();
    assert_eq!(&args[args.len() - 4..], ["--", "cargo", "test", "--all"]);

    let request: CommandRequest = serde_json::from_value(arguments).expect("request");
    let wrapped = sandbox_request(&request, &workspace, false, &[]).expect("request wrap");
    let value = serde_json::to_value(&wrapped).expect("value");
    assert_eq!(value["command"]["program"], "bwrap");
    assert_eq!(sandbox_name(), "bwrap");

    let broken = serde_json::json!({"command": {"kind": "teleport"}});
    assert!(sandboxed_arguments(&broken, &workspace, false, &[]).is_err());
}

#[test]
fn the_macos_profile_limits_writes_and_the_network() {
    let profile = macos_profile(
        Path::new("/work"),
        false,
        &[PathBuf::from("/cache"), PathBuf::from("/tmp/a b")],
    )
    .expect("profile");
    assert!(profile.starts_with("(version 1)\n(allow default)\n"));
    assert!(profile.contains("(deny network*)"));
    assert!(profile.contains("(deny file-write*)"));
    assert!(profile.contains("(subpath \"/work\")"));
    assert!(profile.contains("(subpath \"/cache\")"));
    assert!(profile.contains("(subpath \"/tmp/a b\")"));
    assert!(profile.contains("(subpath \"/private/tmp\")"));
    assert!(profile.contains("(subpath \"/dev\")"));

    let online = macos_profile(Path::new("/work"), true, &[]).expect("online profile");
    assert!(!online.contains("(deny network*)"));

    assert!(macos_profile(Path::new("/work\n(allow default)"), false, &[]).is_err());
}

#[test]
fn retry_plan_uses_the_provider_hint_and_caps_the_wait() {
    let config = RetryConfig {
        max_attempts: 2,
        max_delay_ms: 5_000,
        backoff_ms: 1_000,
    };

    let hinted = norte::Error::provider(
        norte::ProviderError::new(ProviderErrorKind::RateLimited, "wait")
            .with_retry_after(Duration::from_secs(60)),
    );
    assert_eq!(
        retry_plan(&hinted, &config, 0).map(|plan| plan.0),
        Some(Duration::from_millis(5_000))
    );
    assert!(retry_plan(&hinted, &config, 2).is_none());

    let unhinted = norte::Error::provider(norte::ProviderError::new(
        ProviderErrorKind::Transport,
        "down",
    ));
    assert_eq!(
        retry_plan(&unhinted, &config, 0).map(|plan| plan.0),
        Some(Duration::from_millis(1_000))
    );
    assert_eq!(
        retry_plan(&unhinted, &config, 1).map(|plan| plan.0),
        Some(Duration::from_millis(2_000))
    );

    let permanent = norte::Error::provider(norte::ProviderError::new(
        ProviderErrorKind::QuotaExceeded,
        "no credit",
    ));
    assert!(retry_plan(&permanent, &config, 0).is_none());
}

#[test]
fn the_default_retry_schedule_waits_out_a_rate_limit_window() {
    let config = RetryConfig::default();
    let limited = norte::Error::provider(norte::ProviderError::new(
        ProviderErrorKind::RateLimited,
        "wait",
    ));
    let delays: Vec<Duration> = (0..4)
        .filter_map(|used| retry_plan(&limited, &config, used).map(|plan| plan.0))
        .collect();
    assert_eq!(
        delays,
        vec![
            Duration::from_secs(5),
            Duration::from_secs(10),
            Duration::from_secs(20),
            Duration::from_secs(40),
        ]
    );
    assert!(retry_plan(&limited, &config, 4).is_none());
}

#[tokio::test]
async fn sessions_are_created_listed_and_renamed() {
    let service = service();
    let created = drain(
        &service,
        ClientRequestKind::CreateSession {
            title: Some("first".to_owned()),
        },
    )
    .await;
    let key = created[0].session.clone().expect("session");
    assert!(matches!(
        created[0].kind,
        ServerEventKind::SessionCreated { .. }
    ));

    let listed = drain(&service, ClientRequestKind::ListSessions).await;
    let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
        panic!("expected a list");
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title.as_deref(), Some("first"));

    let renamed = drain(
        &service,
        ClientRequestKind::RenameSession {
            session: key,
            title: "second".to_owned(),
        },
    )
    .await;
    assert!(matches!(
        renamed[0].kind,
        ServerEventKind::SessionRenamed { .. }
    ));
}

#[tokio::test]
async fn a_fork_copies_the_history_under_a_new_engine_session() {
    let service = service();
    let created = drain(
        &service,
        ClientRequestKind::CreateSession {
            title: Some("source".to_owned()),
        },
    )
    .await;
    let source_key = created[0].session.clone().expect("session");
    drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(source_key.clone()),
            text: "hello".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let (source_id, source_spec, _) = service.require_record(&source_key).expect("source record");
    let exported = service
        .engine(&source_spec)
        .expect("engine")
        .export(source_id)
        .await
        .expect("export");
    let source_messages = exported.messages.len();

    let forked = drain(
        &service,
        ClientRequestKind::ForkSession {
            session: source_key.clone(),
            title: Some("branch".to_owned()),
        },
    )
    .await;
    assert!(matches!(
        forked[0].kind,
        ServerEventKind::SessionForked { .. }
    ));
    let fork_key = forked[0].session.clone().expect("fork session");
    assert_ne!(fork_key, source_key);

    let (fork_id, fork_spec, _) = service.require_record(&fork_key).expect("fork record");
    assert_ne!(fork_id, source_id, "the fork needs its own engine session");
    assert_eq!(
        service
            .lock_sessions()
            .get(&fork_key)
            .expect("fork record")
            .compacted_from,
        None
    );
    let copied = service
        .engine(&fork_spec)
        .expect("engine")
        .export(fork_id)
        .await
        .expect("export");
    assert_eq!(copied.messages.len(), source_messages);
}

#[tokio::test]
async fn two_sessions_run_turns_concurrently() {
    struct FreshFactory {
        reference: ModelRef,
    }

    impl ModelFactory for FreshFactory {
        fn build(
            &self,
            _spec: &ModelSpec,
            _capabilities: &Capabilities,
        ) -> Result<Arc<dyn Model>, ServiceError> {
            Ok(
                Arc::new(ScriptedModel::new(self.reference.clone()).turn_text("ok"))
                    as Arc<dyn Model>,
            )
        }
    }

    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(FreshFactory { reference }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );

    let mut keys = Vec::new();
    for title in ["a", "b"] {
        let events = drain(
            &service,
            ClientRequestKind::CreateSession {
                title: Some(title.to_owned()),
            },
        )
        .await;
        keys.push(events[0].session.clone().expect("session"));
    }

    let first = service
        .submit(ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::SendMessage {
                session: Some(keys[0].clone()),
                text: "one".to_owned(),
                budget: None,
                reasoning: None,
            },
        })
        .await
        .expect("first turn");
    let second = service
        .submit(ClientRequest {
            request_id: 2,
            kind: ClientRequestKind::SendMessage {
                session: Some(keys[1].clone()),
                text: "two".to_owned(),
                budget: None,
                reasoning: None,
            },
        })
        .await
        .expect("second turn");

    let (first, second) = tokio::join!(first.collect::<Vec<_>>(), second.collect::<Vec<_>>());
    for events in [first, second] {
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                committed: true,
                ..
            }
        )));
    }
}

#[tokio::test]
async fn models_are_listed_with_their_catalogue_limits() {
    let service = service();
    let events = drain(&service, ClientRequestKind::ListModels).await;
    let ServerEventKind::ModelList { models } = &events[0].kind else {
        panic!("expected a model list");
    };
    assert_eq!(models.len(), 2);
    let scripted = models
        .iter()
        .find(|model| model.model == "scripted")
        .expect("scripted");
    assert_eq!(scripted.max_context, Some(1000));
    assert_eq!(scripted.max_output, Some(100));
    assert!(scripted.tools);
}

#[tokio::test]
async fn a_session_can_switch_models_at_a_boundary() {
    let service = service();
    let created = drain(
        &service,
        ClientRequestKind::CreateSession {
            title: Some("switch".to_owned()),
        },
    )
    .await;
    let key = created[0].session.clone().expect("session");
    let changed = drain(
        &service,
        ClientRequestKind::SetModel {
            session: key.clone(),
            model: "test/other".to_owned(),
        },
    )
    .await;
    assert!(matches!(
        changed[0].kind,
        ServerEventKind::ModelChanged { .. }
    ));
    let listed = drain(&service, ClientRequestKind::ListSessions).await;
    let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
        panic!("expected a list");
    };
    assert_eq!(sessions[0].model.as_deref(), Some("test/other"));

    let (nid, spec, _) = service.require_record(&key).expect("record");
    let history = service
        .engine(&spec)
        .expect("engine")
        .history(nid)
        .await
        .expect("history");
    let system: Vec<_> = history
        .iter()
        .filter(|message| message.role == norte::Role::System)
        .collect();
    assert_eq!(system.len(), 3);
    assert!(system[1].text().starts_with("# Runtime awareness"));
    assert!(system[2].text().starts_with("# Runtime update"));
    assert!(system[2].text().contains("test/other"));
}

#[test]
fn workspace_paths_are_rerooted_and_escapes_are_refused() {
    let root = Path::new("/tmp/norte-workspace");
    let relative = transform_path(
        &serde_json::json!({"path": "src/lib.rs", "content": "x"}),
        "path",
        root,
        false,
    )
    .expect("relative path");
    assert_eq!(
        relative.get("path").and_then(serde_json::Value::as_str),
        Some("/tmp/norte-workspace/src/lib.rs")
    );
    assert!(
        transform_path(
            &serde_json::json!({"path": "../../etc/passwd"}),
            "path",
            root,
            false
        )
        .is_err()
    );
    assert!(
        transform_path(
            &serde_json::json!({"path": "/etc/passwd"}),
            "path",
            root,
            false
        )
        .is_err()
    );
    let missing = transform_path(
        &serde_json::json!({"pattern": "**/*.rs"}),
        "path",
        root,
        true,
    )
    .expect("default path");
    assert_eq!(
        missing.get("path").and_then(serde_json::Value::as_str),
        Some("/tmp/norte-workspace")
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_workspace_cannot_reach_outside_it() {
    let base = std::env::temp_dir().join(format!("norte-policy-{}", uuid::Uuid::new_v4()));
    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::write(outside.join("secret"), "do not read").expect("secret");
    let root = std::fs::canonicalize(&root).expect("canonical root");
    std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");

    let error = transform_path(
        &serde_json::json!({"path": "link/secret"}),
        "path",
        &root,
        false,
    )
    .expect_err("a symlink escape must be refused");
    assert!(error.contains("outside the workspace root"), "{error}");
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_is_refused_instead_of_followed() {
    let base = std::env::temp_dir().join(format!("norte-policy-{}", uuid::Uuid::new_v4()));
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("root");
    let root = std::fs::canonicalize(&root).expect("canonical root");
    std::os::unix::fs::symlink(root.join("missing"), root.join("dangling")).expect("symlink");

    let error = transform_path(
        &serde_json::json!({"path": "dangling"}),
        "path",
        &root,
        false,
    )
    .expect_err("a dangling symlink must be refused");
    assert!(error.contains("target does not exist"), "{error}");
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn a_symlink_that_stays_inside_the_workspace_is_followed() {
    let base = std::env::temp_dir().join(format!("norte-policy-{}", uuid::Uuid::new_v4()));
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("target"), "inside").expect("target");
    let root = std::fs::canonicalize(&root).expect("canonical root");
    std::os::unix::fs::symlink(root.join("target"), root.join("link")).expect("symlink");

    let arguments = transform_path(&serde_json::json!({"path": "link"}), "path", &root, false)
        .expect("an inside symlink is allowed");
    assert_eq!(
        arguments.get("path").and_then(serde_json::Value::as_str),
        Some(root.join("target").to_str().expect("utf-8"))
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn a_fifo_is_refused_before_a_tool_can_block_on_it() {
    let base = std::env::temp_dir().join(format!("norte-policy-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).expect("base");
    let fifo = base.join("pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo");
    assert!(status.success());
    let arguments = serde_json::json!({"path": fifo});
    let error =
        check_path_kind(&arguments, "path", PathKind::File).expect_err("a FIFO must be refused");
    assert!(error.contains("not a regular file"), "{error}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn reasoning_settings_are_checked_against_the_model() {
    let capabilities = ReasoningCapability {
        supported: true,
        can_disable: true,
        efforts: BTreeSet::from([
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
            ReasoningEffort::Max,
        ]),
        supports_budget: true,
        max_budget: Some(4096),
    };
    let automatic = serde_json::json!("auto");

    assert_eq!(
        resolve_reasoning(
            Some(&ProtocolReasoning::Effort {
                effort: "medium".to_owned()
            }),
            &automatic,
            &capabilities
        )
        .expect("medium is declared"),
        ReasoningRequest::Effort(ReasoningEffort::Medium)
    );
    assert_eq!(
        resolve_reasoning(
            Some(&ProtocolReasoning::Effort {
                effort: "xhigh".to_owned()
            }),
            &automatic,
            &capabilities
        )
        .expect("xhigh is declared"),
        ReasoningRequest::Effort(ReasoningEffort::XHigh)
    );
    assert_eq!(
        resolve_reasoning(
            Some(&ProtocolReasoning::Effort {
                effort: "max".to_owned()
            }),
            &automatic,
            &capabilities
        )
        .expect("max is declared"),
        ReasoningRequest::Effort(ReasoningEffort::Max)
    );
    let without_top = ReasoningCapability {
        efforts: BTreeSet::from([
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ]),
        ..capabilities.clone()
    };
    assert!(
        resolve_reasoning(
            Some(&ProtocolReasoning::Effort {
                effort: "max".to_owned()
            }),
            &automatic,
            &without_top
        )
        .is_err()
    );
    assert!(
        resolve_reasoning(
            Some(&ProtocolReasoning::Effort {
                effort: "x".to_owned()
            }),
            &automatic,
            &capabilities
        )
        .is_err()
    );
    assert!(
        resolve_reasoning(
            Some(&ProtocolReasoning::Budget { tokens: 5000 }),
            &automatic,
            &capabilities
        )
        .is_err()
    );
    assert_eq!(
        resolve_reasoning(
            Some(&ProtocolReasoning::Budget { tokens: 1024 }),
            &automatic,
            &capabilities
        )
        .expect("a budget under the cap"),
        ReasoningRequest::Budget(1024)
    );
}

#[test]
fn the_implicit_default_prefers_medium() {
    let capabilities = ReasoningCapability {
        supported: true,
        can_disable: false,
        efforts: BTreeSet::from([ReasoningEffort::Low, ReasoningEffort::Medium]),
        supports_budget: false,
        max_budget: None,
    };
    assert_eq!(
        resolve_reasoning(None, &serde_json::json!("auto"), &capabilities)
            .expect("medium is declared"),
        ReasoningRequest::Effort(ReasoningEffort::Medium)
    );

    let without_medium = ReasoningCapability {
        efforts: BTreeSet::from([ReasoningEffort::Low]),
        ..capabilities
    };
    assert_eq!(
        resolve_reasoning(None, &serde_json::json!("auto"), &without_medium)
            .expect("fall back to the closest lower effort"),
        ReasoningRequest::Effort(ReasoningEffort::Low)
    );

    let unsupported = ReasoningCapability {
        supported: false,
        ..without_medium
    };
    assert_eq!(
        resolve_reasoning(None, &serde_json::json!("auto"), &unsupported)
            .expect("a non-reasoning model stays automatic"),
        ReasoningRequest::Automatic
    );
}

#[tokio::test]
async fn set_reasoning_applies_to_the_session() {
    let (service, model) = scripted_service(spec("scripted"), &["ok"]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");

    let ack = drain(
        &service,
        ClientRequestKind::SetReasoning {
            session: key.clone(),
            scope: ReasoningScope::Session,
            reasoning: ProtocolReasoning::Effort {
                effort: "high".to_owned(),
            },
        },
    )
    .await;
    assert!(ack.is_empty());

    drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "hi".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].reasoning,
        ReasoningRequest::Effort(ReasoningEffort::High)
    );
}

#[tokio::test]
async fn a_message_reasoning_override_is_used_once() {
    let (service, model) = scripted_service(spec("scripted"), &["first", "second"]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");

    drain(
        &service,
        ClientRequestKind::SetReasoning {
            session: key.clone(),
            scope: ReasoningScope::Message,
            reasoning: ProtocolReasoning::Effort {
                effort: "high".to_owned(),
            },
        },
    )
    .await;
    for text in ["one", "two"] {
        drain(
            &service,
            ClientRequestKind::SendMessage {
                session: Some(key.clone()),
                text: text.to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
    }

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].reasoning,
        ReasoningRequest::Effort(ReasoningEffort::High)
    );
    assert_eq!(
        requests[1].reasoning,
        ReasoningRequest::Effort(ReasoningEffort::Medium)
    );
}

#[tokio::test]
async fn set_reasoning_rejects_an_unsupported_effort() {
    let (service, _) = scripted_service(spec("scripted"), &[]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");

    let result = service
        .submit(ClientRequest {
            request_id: 7,
            kind: ClientRequestKind::SetReasoning {
                session: key,
                scope: ReasoningScope::Session,
                reasoning: ProtocolReasoning::Effort {
                    effort: "minimal".to_owned(),
                },
            },
        })
        .await;
    assert!(matches!(result, Err(ServiceError::Reasoning(_))));
}

#[tokio::test]
async fn a_model_switch_rejects_an_incompatible_reasoning_override() {
    let (service, _) = scripted_service(spec("scripted"), &[]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");
    drain(
        &service,
        ClientRequestKind::SetReasoning {
            session: key.clone(),
            scope: ReasoningScope::Session,
            reasoning: ProtocolReasoning::Effort {
                effort: "high".to_owned(),
            },
        },
    )
    .await;

    let result = service
        .submit(ClientRequest {
            request_id: 7,
            kind: ClientRequestKind::SetModel {
                session: key,
                model: "test/other".to_owned(),
            },
        })
        .await;
    assert!(matches!(result, Err(ServiceError::Reasoning(_))));
}

#[tokio::test]
async fn steer_queues_a_message_before_the_next_turn_input() {
    let (service, model) = scripted_service(spec("scripted"), &["one", "two"]);
    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "one".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");

    let ack = drain(
        &service,
        ClientRequestKind::Steer {
            session: key.clone(),
            text: "two".to_owned(),
            id: String::new(),
        },
    )
    .await;
    assert!(ack.is_empty());

    drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "three".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        user_texts(&requests[1]),
        vec!["one".to_owned(), "two".to_owned(), "three".to_owned()]
    );
}

#[tokio::test]
async fn the_steers_in_repose_all_enter_the_turn_that_starts() {
    let (service, model) = scripted_service(spec("scripted"), &["one", "after"]);
    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "one".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");

    // Two messages arrive while the session rests.
    drain(
        &service,
        ClientRequestKind::Steer {
            session: key.clone(),
            text: "a".to_owned(),
            id: String::new(),
        },
    )
    .await;
    drain(
        &service,
        ClientRequestKind::Steer {
            session: key.clone(),
            text: "b".to_owned(),
            id: String::new(),
        },
    )
    .await;

    // A turn with no input of its own still takes them, in order.
    drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
        },
    )
    .await;

    let requests = model.requests();
    assert_eq!(requests.len(), 2, "the resume made one request");
    assert_eq!(
        user_texts(&requests[1]),
        vec!["one".to_owned(), "a".to_owned(), "b".to_owned()],
        "both steers reach the turn, in arrival order"
    );
}

#[tokio::test]
async fn a_queued_model_that_cannot_be_honoured_does_not_break_the_session() {
    let (service, model) = scripted_service(spec("scripted"), &["one", "two"]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");

    // A switch to a model that does not reason, queued while the session asks
    // for one, cannot be honoured at the boundary.
    {
        let mut sessions = service.lock_sessions();
        let record = sessions.get_mut(&key).expect("record");
        record.pending_model = Some("test/other".to_owned());
        record.reasoning = Some(ProtocolReasoning::Effort {
            effort: "high".to_owned(),
        });
    }

    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::Error { .. })),
        "the turn must not fail because of a switch it cannot apply"
    );
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                committed: true,
                ..
            }
        )),
        "and the turn commits"
    );
    assert!(
        service
            .lock_sessions()
            .get(&key)
            .expect("record")
            .pending_model
            .is_none(),
        "the switch that cannot be honoured is forgotten"
    );
    assert_eq!(model.requests().len(), 1, "the turn made one request");
}

#[tokio::test]
async fn a_model_switch_and_a_compaction_at_the_same_boundary_are_both_reported() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn(vec![
                norte::StreamEvent::Usage(usage(100, 10, Some(0.01))),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "first".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ])
            .turn_text("the user asked for a first answer"),
    );
    let mut config = config();
    config.compaction.threshold = 0.1;
    config.compaction.keep_recent = 1;
    let (service, _store) = persistent_service_with_config(config, model).await;
    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "first".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");

    // The second turn grows the history past the threshold. Its compaction at
    // the end of the turn cannot summarise, so the history stays long and the
    // next boundary compacts.
    drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "second".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    // The next boundary applies a queued switch and compacts.
    {
        let mut sessions = service.lock_sessions();
        let record = sessions.get_mut(&key).expect("record");
        record.pending_model = Some("test/other".to_owned());
    }
    let second = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "third".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let changed = second
        .iter()
        .any(|event| matches!(event.kind, ServerEventKind::ModelChanged { .. }));
    let compacted = second
        .iter()
        .any(|event| matches!(event.kind, ServerEventKind::CompactionStarted { .. }));
    assert!(compacted, "the boundary compacted");
    assert!(changed, "and the queued switch was reported too");
}

/// An interceptor that stops the turn, the way a host's own policy would.
struct Stopper {
    reason: String,
}

#[async_trait]
impl TurnInterceptor for Stopper {
    fn name(&self) -> &'static str {
        "stopper"
    }

    async fn intercept(&self, _context: &InterceptionContext<'_>) -> norte::Result<Interception> {
        Ok(Interception::Stop {
            reason: self.reason.clone(),
        })
    }
}

#[tokio::test]
async fn a_turn_an_interceptor_stopped_is_reported_as_stopped() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("never"));
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory {
                model: Arc::clone(&model) as Arc<dyn Model>,
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service")
        .with_interceptor(Arc::new(Stopper {
            reason: "the host said no".to_owned(),
        })),
    );

    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let stopped = events.iter().find_map(|event| match &event.kind {
        ServerEventKind::TurnStopped { reason } => Some(reason.clone()),
        _ => None,
    });
    assert_eq!(
        stopped.as_deref(),
        Some("the host said no"),
        "the client hears why the turn ended"
    );
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                committed: false,
                ..
            }
        )),
        "and the turn ends with nothing committed"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::Error { .. })),
        "a deliberate stop is not a failure"
    );
    assert_eq!(model.request_count(), 0, "the turn made no request");
}

#[tokio::test]
async fn the_budget_stops_the_next_request_every_time() {
    // The gate must see the request it just let through. The cost used to be
    // recorded in the service, which runs while the engine already asks for the
    // next request, so the budget sometimes let that request through.
    let mut config = config();
    config.budgets.session = Some(BudgetConfig {
        unit: "requests".to_owned(),
        limit: 1.0,
        action: "stop".to_owned(),
    });
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let mut model = ScriptedModel::new(reference);
    for _ in 0..10 {
        // Each turn asks for a tool: the budget stops the request that would
        // carry the answer, so the model never gets to the turn after it.
        // The delay makes the service settle the request before the answer is
        // committed, which is the race the budget used to lose.
        let call = CallId::new();
        model = model.turn_delayed(
            vec![
                norte::StreamEvent::ToolCallStart {
                    index: 0,
                    call_id: call,
                    name: "todo".to_owned(),
                },
                norte::StreamEvent::ToolCallDelta {
                    index: 0,
                    call_id: call,
                    name: None,
                    arguments: serde_json::json!({"todos": []}).to_string(),
                },
                norte::StreamEvent::Finish(norte::FinishReason::ToolCalls),
            ],
            Duration::from_millis(20),
        );
    }
    let model = Arc::new(model);
    let store = Arc::new(SqliteStore::in_memory().expect("store"));
    let service = Arc::new(
        Service::with_directory(
            config,
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            store.clone(),
            store.clone(),
            store.clone(),
            "system",
        )
        .await
        .expect("service"),
    );

    for _ in 0..10 {
        let events = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "work".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        eprintln!(
            "DBG {}",
            events
                .iter()
                .map(|event| format!("{:?}", event.kind))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.kind, ServerEventKind::BudgetExceeded { .. })),
            "the budget stops the request after the one it paid for"
        );
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                committed: false,
                ..
            }
        )));
    }
}

#[tokio::test]
async fn reasoning_and_steering_survive_a_restart() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
    let store = Arc::new(SqliteStore::in_memory().expect("store"));
    let first = persistent_scripted_service(
        spec("scripted"),
        Arc::clone(&model) as Arc<dyn Model>,
        Arc::clone(&store),
    )
    .await;
    let created = drain(&first, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");
    drain(
        &first,
        ClientRequestKind::SetReasoning {
            session: key.clone(),
            scope: ReasoningScope::Session,
            reasoning: ProtocolReasoning::Effort {
                effort: "high".to_owned(),
            },
        },
    )
    .await;
    drain(
        &first,
        ClientRequestKind::Steer {
            session: key.clone(),
            text: "steered".to_owned(),
            id: String::new(),
        },
    )
    .await;
    drop(first);

    let second = persistent_scripted_service(
        spec("scripted"),
        Arc::clone(&model) as Arc<dyn Model>,
        store,
    )
    .await;
    drain(
        &second,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "hello".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].reasoning,
        ReasoningRequest::Effort(ReasoningEffort::High)
    );
    assert_eq!(
        user_texts(&requests[0]),
        vec!["steered".to_owned(), "hello".to_owned()]
    );
}

#[test]
fn configured_reasoning_accepts_efforts_and_budgets() {
    assert_eq!(
        parse_configured_reasoning(&serde_json::json!("high")).expect("effort"),
        ProtocolReasoning::Effort {
            effort: "high".to_owned()
        }
    );
    assert_eq!(
        parse_configured_reasoning(&serde_json::json!("xhigh")).expect("xhigh"),
        ProtocolReasoning::Effort {
            effort: "xhigh".to_owned()
        }
    );
    assert_eq!(
        parse_configured_reasoning(&serde_json::json!("max")).expect("max"),
        ProtocolReasoning::Effort {
            effort: "max".to_owned()
        }
    );
    assert_eq!(
        parse_configured_reasoning(&serde_json::json!({"budget": 2048})).expect("budget"),
        ProtocolReasoning::Budget { tokens: 2048 }
    );
    assert!(parse_configured_reasoning(&serde_json::json!({"budget": 0})).is_err());
}

fn usage(input: u64, output: u64, cost: Option<f64>) -> Usage {
    let mut usage = Usage::default();
    usage.input_tokens = Some(input);
    usage.output_tokens = Some(output);
    usage.vendor = cost.map_or_else(BTreeMap::new, |cost| {
        BTreeMap::from([("cost".to_owned(), serde_json::json!(cost))])
    });
    usage
}

async fn persistent_service(model: Arc<ScriptedModel>) -> (Arc<Service>, Arc<SqliteStore>) {
    persistent_service_with_config(config(), model).await
}

async fn persistent_service_with_config(
    config: Config,
    model: Arc<dyn Model>,
) -> (Arc<Service>, Arc<SqliteStore>) {
    let store = Arc::new(SqliteStore::in_memory().expect("store"));
    let service = Service::with_directory(
        config,
        catalog(),
        spec("scripted"),
        Arc::new(ScriptedFactory { model }),
        store.clone(),
        store.clone(),
        store.clone(),
        "system",
    )
    .await
    .expect("service");
    (Arc::new(service), store)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn automatic_compaction_creates_a_new_north_session_at_the_threshold() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn(vec![
                norte::StreamEvent::Usage(usage(100, 10, Some(0.01))),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "first".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ])
            .turn_text("the user asked for a first answer")
            .turn(vec![
                norte::StreamEvent::Usage(usage(80, 10, Some(0.01))),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "second".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ]),
    );
    let mut config = config();
    config.compaction.threshold = 0.1;
    config.compaction.keep_recent = 1;
    let (service, store) = persistent_service_with_config(config, model).await;
    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "first".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    let mut started = false;
    let mut finished = None;
    for event in &first {
        match &event.kind {
            ServerEventKind::CompactionStarted { reason } => {
                assert_eq!(reason, "threshold");
                started = true;
            }
            ServerEventKind::CompactionFinished {
                compacted,
                north_session,
                ..
            } => finished = Some((*compacted, north_session.clone())),
            _ => {}
        }
    }
    assert!(started);
    let (compacted, north) = finished.expect("compaction finished");
    assert!(compacted);
    let north = north.expect("new session id");
    let after_first = store
        .sessions()
        .await
        .expect("sessions")
        .into_iter()
        .find(|session| session.key == key.as_str())
        .expect("stored session");
    assert_eq!(after_first.north.to_string(), north);
    let old = after_first.compacted_from.expect("compacted from");
    assert!(store.load(old).await.is_ok());

    let second = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "second".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let mut text = String::new();
    for event in &second {
        match &event.kind {
            ServerEventKind::CompactionStarted { .. }
            | ServerEventKind::CompactionFinished { .. } => {
                panic!("the second turn did not need another compaction");
            }
            ServerEventKind::Delta {
                delta: Delta::Text { text: chunk, .. },
                ..
            } => text.push_str(chunk),
            _ => {}
        }
    }
    assert_eq!(text, "second");
    let after_second = store
        .sessions()
        .await
        .expect("sessions")
        .into_iter()
        .find(|session| session.key == key.as_str())
        .expect("stored session");
    assert_eq!(after_second.north, after_first.north);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn eager_compaction_restarts_a_tool_turn_at_a_safe_point() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn(vec![
                norte::StreamEvent::Usage(usage(600, 10, None)),
                norte::StreamEvent::ToolCallStart {
                    index: 0,
                    call_id: call,
                    name: "todo".to_owned(),
                },
                norte::StreamEvent::ToolCallDelta {
                    index: 0,
                    call_id: call,
                    name: None,
                    arguments: serde_json::json!({"todos": []}).to_string(),
                },
                norte::StreamEvent::Finish(FinishReason::ToolCalls),
            ])
            .turn_text("summary of the earlier conversation")
            .turn_text("done"),
    );
    let mut config = config();
    config.compaction.threshold = 0.5;
    config.compaction.trigger = "eager".to_owned();
    config.compaction.keep_recent = 1;
    let (service, store) = persistent_service_with_config(config, model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    let mut started = 0;
    let mut finished = None;
    let mut text = String::new();
    let mut turn_finished = None;
    for event in &events {
        match &event.kind {
            ServerEventKind::CompactionStarted { reason } => {
                assert_eq!(reason, "threshold");
                started += 1;
            }
            ServerEventKind::CompactionFinished {
                compacted,
                north_session,
                ..
            } => finished = Some((*compacted, north_session.clone())),
            ServerEventKind::Delta {
                delta: Delta::Text { text: chunk, .. },
                ..
            } => text.push_str(chunk),
            ServerEventKind::TurnFinished {
                phase, committed, ..
            } => turn_finished = Some((*phase, *committed)),
            _ => {}
        }
    }
    assert_eq!(started, 1, "{events:#?}");
    let (compacted, north) = finished.expect("compaction finished");
    assert!(compacted);
    assert_eq!(text, "done");
    assert_eq!(
        turn_finished,
        Some((norte_code_protocol::Phase::Idle, true))
    );
    let north = north.expect("new session id");
    let stored = store
        .sessions()
        .await
        .expect("sessions")
        .into_iter()
        .find(|session| session.key == key.as_str())
        .expect("stored session");
    assert_eq!(stored.north.to_string(), north);
    let old = stored.compacted_from.expect("compacted from");
    assert!(store.load(old).await.is_ok());
}

#[tokio::test]
async fn a_failed_eager_summary_stops_at_the_safe_point() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(ScriptedModel::new(reference).turn(vec![
        norte::StreamEvent::Usage(usage(600, 10, None)),
        norte::StreamEvent::ToolCallStart {
            index: 0,
            call_id: call,
            name: "todo".to_owned(),
        },
        norte::StreamEvent::ToolCallDelta {
            index: 0,
            call_id: call,
            name: None,
            arguments: serde_json::json!({"todos": []}).to_string(),
        },
        norte::StreamEvent::Finish(FinishReason::ToolCalls),
    ]));
    let mut config = config();
    config.compaction.threshold = 0.5;
    config.compaction.trigger = "eager".to_owned();
    config.compaction.keep_recent = 1;
    let (service, store) = persistent_service_with_config(config, model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        ServerEventKind::CompactionFinished {
            compacted: false,
            error: Some(_),
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        ServerEventKind::TurnFinished {
            committed: false,
            ..
        }
    )));
    let stored = store
        .sessions()
        .await
        .expect("sessions")
        .into_iter()
        .find(|session| session.key == key.as_str())
        .expect("stored session");
    assert_eq!(stored.compacted_from, None);
    assert!(store.load(stored.north).await.is_ok());
}

#[tokio::test]
async fn manual_compaction_links_the_new_session_and_keeps_the_old_one() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn(vec![
                norte::StreamEvent::Usage(usage(10, 2, Some(0.001))),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "kept".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ])
            .turn_text("the user wants a compacted handoff"),
    );
    let (service, store) = persistent_service(model).await;
    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    let before = store
        .sessions()
        .await
        .expect("sessions")
        .into_iter()
        .find(|session| session.key == key.as_str())
        .expect("stored session");
    let events = drain(
        &service,
        ClientRequestKind::Compact {
            session: key.clone(),
        },
    )
    .await;
    let finished = events.iter().find_map(|event| match &event.kind {
        ServerEventKind::CompactionFinished {
            compacted,
            north_session,
            ..
        } => Some((*compacted, north_session.clone())),
        _ => None,
    });
    let (compacted, north) = finished.expect("finished event");
    assert!(compacted);
    let after = store
        .sessions()
        .await
        .expect("sessions")
        .into_iter()
        .find(|session| session.key == key.as_str())
        .expect("stored session");
    assert_ne!(after.north, before.north);
    assert_eq!(after.north.to_string(), north.expect("new id"));
    assert_eq!(after.compacted_from, Some(before.north));
    assert!(store.load(before.north).await.is_ok());
}

#[tokio::test]
async fn a_failed_summary_leaves_the_session_unchanged() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn(vec![
        norte::StreamEvent::Usage(usage(10, 2, Some(0.001))),
        norte::StreamEvent::TextDelta {
            index: 0,
            text: "kept".to_owned(),
        },
        norte::StreamEvent::Finish(FinishReason::Stop),
    ]));
    let (service, store) = persistent_service(model).await;
    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    let before = store
        .sessions()
        .await
        .expect("sessions")
        .into_iter()
        .find(|session| session.key == key.as_str())
        .expect("stored session");
    let events = drain(
        &service,
        ClientRequestKind::Compact {
            session: key.clone(),
        },
    )
    .await;
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        ServerEventKind::CompactionFinished {
            compacted: false,
            error: Some(_),
            ..
        }
    )));
    let after = store
        .sessions()
        .await
        .expect("sessions")
        .into_iter()
        .find(|session| session.key == key.as_str())
        .expect("stored session");
    assert_eq!(after.north, before.north);
    assert_eq!(after.compacted_from, before.compacted_from);
}

#[test]
fn a_recent_suffix_never_orphans_tool_results() {
    let call = CallId::new();
    let assistant = Message::new(
        Role::Assistant,
        vec![norte::Part::ToolCall(norte::ToolCallPart {
            call_id: call,
            name: "read".to_owned(),
            arguments: serde_json::json!({}),
            continuity: None,
        })],
    );
    let tool = Message::tool(vec![norte::ToolResultPart::text(call, "ok")]);
    let messages = vec![
        Message::system("system"),
        Message::user("go"),
        assistant,
        tool,
    ];
    assert_eq!(recent_start(&messages, 1), 2);
}

#[tokio::test]
async fn the_request_table_records_usage_latency_and_metadata() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn(vec![
        norte::StreamEvent::Usage(usage(120, 30, Some(0.125))),
        norte::StreamEvent::TextDelta {
            index: 0,
            text: "hello".to_owned(),
        },
        norte::StreamEvent::Finish(FinishReason::Stop),
    ]));
    let (service, store) = persistent_service(model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "hi".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let session = events
        .iter()
        .find_map(|event| event.session.as_ref())
        .expect("session")
        .to_string();
    let requests = store.requests(None).await.expect("requests");
    assert_eq!(requests.len(), 1);
    let record = &requests[0];
    assert_eq!(record.session, session);
    assert_eq!(record.provider, "test");
    assert_eq!(record.model, "scripted");
    assert_eq!(record.input_tokens, Some(120));
    assert_eq!(record.output_tokens, Some(30));
    assert_eq!(record.cost_usd, Some(0.125));
    assert_eq!(record.status, "ok");
    assert!(record.first_token_ms.is_some());
    assert!(record.metadata.get("engine_events").is_some());
    assert_eq!(record.params, serde_json::json!({"max_output_tokens": 100}));
}

#[test]
fn project_instructions_are_bounded_and_appended_to_the_prompt() {
    let dir = std::env::temp_dir().join(format!("norte-instructions-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    assert!(load_project_instructions(&dir).is_none());

    std::fs::write(dir.join(PROJECT_INSTRUCTIONS_FILE), "   \n").expect("write");
    assert!(load_project_instructions(&dir).is_none());

    std::fs::write(dir.join(PROJECT_INSTRUCTIONS_FILE), "Use tabs.\n").expect("write");
    let loaded = load_project_instructions(&dir).expect("instructions");
    assert_eq!(loaded.text, "Use tabs.\n");
    let prompt = compose_system_prompt("base".to_owned(), Some(&loaded));
    assert!(prompt.starts_with("base\n\n# Project instructions"));
    assert!(prompt.contains("Use tabs."));
    assert_eq!(compose_system_prompt("base".to_owned(), None), "base");
    assert!(!compose_system_prompt(String::new(), Some(&loaded)).starts_with('\n'));

    // A socket is not a regular file, so it is refused instead of read.
    let socket = dir.join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("socket");
    std::fs::remove_file(dir.join(PROJECT_INSTRUCTIONS_FILE)).expect("remove");
    std::os::unix::fs::symlink(&socket, dir.join(PROJECT_INSTRUCTIONS_FILE)).expect("link");
    assert!(load_project_instructions(&dir).is_none());
    std::fs::remove_file(dir.join(PROJECT_INSTRUCTIONS_FILE)).expect("unlink");

    std::fs::write(
        dir.join(PROJECT_INSTRUCTIONS_FILE),
        "a".repeat(MAX_PROJECT_INSTRUCTIONS_BYTES + 100),
    )
    .expect("write");
    let loaded = load_project_instructions(&dir).expect("instructions");
    assert!(
        loaded
            .text
            .ends_with("[project instructions truncated at 64 KiB]")
    );
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn token_rate_starts_at_the_first_delta_and_ends_with_the_request() {
    // 206 tokens after the 4.278 s wait: the remaining 0.736 s is the
    // generation window.
    let rate = token_rate(Some(206), 5_014, Some(4_278)).expect("rate");
    assert!((rate - 279.9).abs() < 0.2, "{rate}");
    // A single final chunk leaves no meaningful window: use the whole
    // request instead of dividing by a sliver.
    let rate = token_rate(Some(64), 1_700, Some(1_698)).expect("rate");
    assert!((rate - 37.6).abs() < 0.2, "{rate}");
    let rate = token_rate(Some(81), 2_300, Some(2_000)).expect("rate");
    assert!((rate - 270.0).abs() < 1.0, "{rate}");
    assert_eq!(token_rate(None, 1_000, Some(10)), None);
    assert_eq!(token_rate(Some(0), 1_000, Some(10)), None);
    assert_eq!(token_rate(Some(10), 0, None), None);
}

#[tokio::test]
async fn a_reconstructed_request_carries_its_tools_and_timing() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(call, "todo", &serde_json::json!({"todos": []}))
            .turn_text("done"),
    );
    let (service, _store) = persistent_service(model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let requests: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            ServerEventKind::RequestReconstructed {
                tools, duration_ms, ..
            } => Some((tools.clone(), *duration_ms)),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].0, vec!["todo".to_owned()]);
    assert!(requests[1].0.is_empty());
}

#[tokio::test]
async fn the_cost_ledger_records_every_request_with_reported_or_missing_cost() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn(vec![
                norte::StreamEvent::Usage(usage(100, 20, Some(0.25))),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "first".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ])
            .turn(vec![
                norte::StreamEvent::Usage(usage(50, 10, None)),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "second".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ]),
    );
    let (service, store) = persistent_service(model).await;
    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "one".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = first[0].session.clone().expect("session");
    let second = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "two".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    assert!(second.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::RequestReconstructed { cost_usd: None, .. }
    )));
    let costs = store.costs(Some(key.as_str())).await.expect("costs");
    assert_eq!(costs.len(), 2);
    assert_eq!(costs[0].index, 1);
    assert_eq!(costs[0].input_tokens, Some(100));
    assert_eq!(costs[0].cost_usd, Some(0.25));
    assert!(costs[1].cost_usd.is_none());
    assert_eq!(costs[1].index, 2);
}

#[tokio::test]
async fn a_session_budget_stops_the_next_request() {
    let mut config = config();
    config.budgets.session = Some(BudgetConfig {
        unit: "requests".to_owned(),
        limit: 1.0,
        action: "stop".to_owned(),
    });
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(call, "todo", &serde_json::json!({"todos": []}))
            .turn_text("done"),
    );
    let store = Arc::new(SqliteStore::in_memory().expect("store"));
    let service = Arc::new(
        Service::with_directory(
            config,
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            store.clone(),
            store.clone(),
            store.clone(),
            "system",
        )
        .await
        .expect("service"),
    );
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let exceeded = events
        .iter()
        .filter(|event| matches!(event.kind, ServerEventKind::BudgetExceeded { .. }))
        .count();
    assert_eq!(exceeded, 1, "the stop is reported once");
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::BudgetExceeded {
            scope: norte_code_protocol::BudgetScope::Session,
            action: norte_code_protocol::BudgetAction::Stop,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::TurnFinished {
            committed: false,
            ..
        }
    )));
    let key = events[0].session.clone().expect("session");
    let changed = drain(
        &service,
        ClientRequestKind::SetBudget {
            session: key.clone(),
            budget: Budget {
                unit: BudgetUnit::Requests,
                limit: 10.0,
                action: norte_code_protocol::BudgetAction::Stop,
            },
        },
    )
    .await;
    assert!(changed.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::BudgetChanged { budget: Some(_) }
    )));
    let resumed = drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
        },
    )
    .await;
    assert!(resumed.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::TurnFinished {
            committed: true,
            ..
        }
    )));
}

#[tokio::test]
async fn vendor_extensions_reach_the_model_request() {
    let mut config = config();
    config.extensions.insert(
        "openai.chat:provider.order".to_owned(),
        serde_json::json!(["deepinfra"]),
    );
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
    let seen = Arc::clone(&model);
    let service = Arc::new(
        Service::new(
            config,
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "hello".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let requests = seen.requests();
    assert_eq!(
        requests[0].extensions["openai.chat:provider.order"],
        serde_json::json!(["deepinfra"])
    );
}

#[tokio::test]
async fn a_usd_budget_is_refused_when_the_provider_does_not_report_cost() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn(vec![
        norte::StreamEvent::Usage(usage(10, 5, None)),
        norte::StreamEvent::TextDelta {
            index: 0,
            text: "again".to_owned(),
        },
        norte::StreamEvent::Finish(FinishReason::Stop),
    ]));
    let (service, _) = persistent_service(model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "one".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = events[0].session.clone().expect("session");
    let result = service
        .submit(ClientRequest {
            request_id: 8,
            kind: ClientRequestKind::SetBudget {
                session: key,
                budget: Budget {
                    unit: BudgetUnit::Usd,
                    limit: 1.0,
                    action: norte_code_protocol::BudgetAction::Stop,
                },
            },
        })
        .await;
    let Err(error) = result else {
        panic!("the unmeasurable budget should have been refused");
    };
    assert!(error.to_string().contains("no reported cost"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_write_parks_for_approval_and_resumes() {
    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!("norte-code-write-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&target);
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "write",
                &serde_json::json!({
                    "path": target.display().to_string(),
                    "content": "written by a tool",
                    "overwrite": true
                }),
            )
            .turn_text("done"),
    );
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );

    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "write it".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let session = first[0].session.clone().expect("session");
    let (approval, arguments) = first
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ApprovalRequested {
                approval,
                arguments,
                ..
            } => Some((approval.clone(), arguments.clone())),
            _ => None,
        })
        .expect("approval request");
    assert_eq!(
        arguments.expect("approval arguments")["path"],
        target.display().to_string()
    );
    assert!(first.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::TurnFinished {
            phase: norte_code_protocol::Phase::AwaitingApproval,
            ..
        }
    )));
    assert!(!target.exists());

    let second = drain(
        &service,
        ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: None,
            remember: None,
        },
    )
    .await;
    assert!(second.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::ApprovalResolved { approved: true, .. }
    )));
    let started = second
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ToolStarted {
                tool, arguments, ..
            } if tool == "write" => arguments.clone(),
            _ => None,
        })
        .expect("tool start arguments");
    assert_eq!(started["path"], target.display().to_string());
    let finished = second
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ToolFinished {
                tool,
                is_error: false,
                output,
                ..
            } if tool == "write" => output.clone(),
            _ => None,
        })
        .expect("tool output");
    assert!(
        finished.contains("wrote") && finished.contains(&target.display().to_string()),
        "unexpected write output: {finished}"
    );
    assert_eq!(
        std::fs::read_to_string(&target).expect("written file"),
        "written by a tool"
    );
    let _ = std::fs::remove_file(&target);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_model_switch_while_parked_is_applied_at_the_next_turn_boundary() {
    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!(
            "norte-code-queued-model-{}.txt",
            std::process::id()
        ));
    let _ = std::fs::remove_file(&target);
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "write",
                &serde_json::json!({
                    "path": target.display().to_string(),
                    "content": "written by a tool",
                    "overwrite": true
                }),
            )
            .turn_text("done")
            .turn_text("after the switch"),
    );
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );

    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "write it".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let session = first[0].session.clone().expect("session");
    let approval = first
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ApprovalRequested { approval, .. } => Some(approval.clone()),
            _ => None,
        })
        .expect("approval request");

    // The session is parked, so the switch is accepted but not applied.
    let queued = drain(
        &service,
        ClientRequestKind::SetModel {
            session: session.clone(),
            model: "test/other".to_owned(),
        },
    )
    .await;
    assert!(
        queued
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::ModelQueued { .. })),
        "the switch should be queued while the session is busy"
    );
    let listed = drain(&service, ClientRequestKind::ListSessions).await;
    let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
        panic!("expected a list");
    };
    assert_eq!(sessions[0].model.as_deref(), Some("test/scripted"));

    // The approval continuation is not a turn boundary, so the model stays.
    let second = drain(
        &service,
        ClientRequestKind::Approve {
            session: session.clone(),
            approval,
            arguments: None,
            scope: None,
            remember: None,
        },
    )
    .await;
    assert!(
        !second
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::ModelChanged { .. }))
    );
    let _ = std::fs::remove_file(&target);

    // The next real turn applies the queued switch.
    let third = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(session.clone()),
            text: "next".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    assert!(
        third
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::ModelChanged { .. })),
        "the queued switch should be applied at the turn boundary"
    );
    let listed = drain(&service, ClientRequestKind::ListSessions).await;
    let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
        panic!("expected a list");
    };
    assert_eq!(sessions[0].model.as_deref(), Some("test/other"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn grants_are_listed_and_can_be_revoked() {
    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!("norte-code-grants-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&target);
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "write",
                &serde_json::json!({
                    "path": target.display().to_string(),
                    "content": "written by a tool",
                    "overwrite": true
                }),
            )
            .turn_text("done"),
    );
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );

    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "write it".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let session = first[0].session.clone().expect("session");
    let approval = first
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ApprovalRequested { approval, .. } => Some(approval.clone()),
            _ => None,
        })
        .expect("approval request");

    let listed = drain(
        &service,
        ClientRequestKind::ListGrants {
            session: session.clone(),
        },
    )
    .await;
    let ServerEventKind::GrantList { grants } = &listed[0].kind else {
        panic!("expected a grant list");
    };
    assert!(grants.is_empty(), "no grant exists before the approval");

    drain(
        &service,
        ClientRequestKind::Approve {
            session: session.clone(),
            approval: approval.clone(),
            arguments: None,
            scope: Some(norte_code_protocol::GrantScope::Session),
            remember: None,
        },
    )
    .await;

    let listed = drain(
        &service,
        ClientRequestKind::ListGrants {
            session: session.clone(),
        },
    )
    .await;
    let ServerEventKind::GrantList { grants } = &listed[0].kind else {
        panic!("expected a grant list");
    };
    assert_eq!(grants.len(), 1, "the approval issued one grant");
    let grant = grants[0].clone();
    assert!(!grant.revoked);
    assert_eq!(grant.scope, norte_code_protocol::GrantScope::Session);
    assert_eq!(grant.tool, "write");

    let revoked = drain(
        &service,
        ClientRequestKind::Revoke {
            session: session.clone(),
            approval: grant.approval.clone(),
            grant: grant.grant.clone(),
        },
    )
    .await;
    assert!(
        revoked
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::GrantRevoked { .. }))
    );

    let listed = drain(
        &service,
        ClientRequestKind::ListGrants {
            session: session.clone(),
        },
    )
    .await;
    let ServerEventKind::GrantList { grants } = &listed[0].kind else {
        panic!("expected a grant list");
    };
    assert!(grants[0].revoked, "the grant should be marked as revoked");
    let _ = std::fs::remove_file(&target);
}

#[tokio::test]
async fn an_edit_snapshots_the_source_before_it_runs() {
    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!(
            "norte-code-edit-{}-{}.txt",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
    std::fs::write(&target, "one\ntwo\nthree\n").expect("editable file");
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "edit",
                &serde_json::json!({
                    "path": target.display().to_string(),
                    "edits": [{"search": "two\n", "replace": "TWO\n"}]
                }),
            )
            .turn_text("done"),
    );
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );

    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "edit it".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let session = first[0].session.clone().expect("session");
    let approval = first
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ApprovalRequested { approval, .. } => Some(approval.clone()),
            _ => None,
        })
        .expect("approval request");

    let second = drain(
        &service,
        ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: None,
            remember: None,
        },
    )
    .await;
    let (arguments, source) = second
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ToolStarted {
                tool,
                arguments,
                source,
                ..
            } if tool == "edit" => Some((arguments.clone(), source.clone())),
            _ => None,
        })
        .expect("tool start");
    assert_eq!(
        arguments.expect("edit arguments")["path"],
        target.display().to_string()
    );
    assert_eq!(source.as_deref(), Some("one\ntwo\nthree\n"));
    assert_eq!(
        std::fs::read_to_string(&target).expect("edited file"),
        "one\nTWO\nthree\n"
    );
    let _ = std::fs::remove_file(&target);
}

fn write_model(call: CallId, target: &std::path::Path) -> Arc<ScriptedModel> {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "write",
                &serde_json::json!({
                    "path": target.display().to_string(),
                    "content": "rules",
                    "overwrite": true
                }),
            )
            .turn_text("done"),
    )
}

#[tokio::test]
async fn a_global_allow_skips_the_approval_prompt() {
    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!(
            "norte-code-global-allow-{}.txt",
            std::process::id()
        ));
    let _ = std::fs::remove_file(&target);
    let mut config = config();
    config.approvals.allow = vec!["write".to_owned()];
    let service = Arc::new(
        Service::new(
            config,
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory {
                model: write_model(CallId::new(), &target),
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "write it".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::ApprovalRequested { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::ToolFinished {
            ref tool,
            is_error: false,
            ..
        } if tool == "write"
    )));
    assert_eq!(
        std::fs::read_to_string(&target).expect("written file"),
        "rules"
    );
    let _ = std::fs::remove_file(&target);
}

#[tokio::test]
async fn a_global_deny_refuses_without_asking() {
    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!("norte-code-global-deny-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&target);
    let mut config = config();
    config.approvals.deny = vec!["write".to_owned()];
    let service = Arc::new(
        Service::new(
            config,
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory {
                model: write_model(CallId::new(), &target),
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "write it".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::ApprovalRequested { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::ToolFinished {
            ref tool,
            is_error: true,
            ..
        } if tool == "write"
    )));
    assert!(!target.exists());
}

#[tokio::test]
async fn a_global_rule_is_persisted_and_applied() {
    let path = std::env::temp_dir().join(format!(
        "norte-code-approval-rules-{}-{}.json",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let _ = std::fs::remove_file(&path);
    let service = service();
    service.set_config_path(Some(path.clone()));
    let session = SessionId::new();

    service
        .remember_global_rule("write", false)
        .expect("deny rule");
    assert!(service.approval_rules.denies(session, "write"));
    let loaded = Config::load(Some(&path), None).expect("config");
    assert_eq!(loaded.config.approvals.deny, ["write"]);

    service
        .remember_global_rule("write", true)
        .expect("allow rule");
    assert!(!service.approval_rules.denies(session, "write"));
    assert!(service.approval_rules.allows(session, "write"));
    let loaded = Config::load(Some(&path), None).expect("config");
    assert_eq!(loaded.config.approvals.allow, ["write"]);
    assert!(loaded.config.approvals.deny.is_empty());
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn session_rules_persist_and_allow_revokes_a_deny() {
    let service = service();
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "hello".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    let nid = service.require_record(&key).expect("record").0;

    service
        .set_approval_rule(Some(&key), "command", false, false)
        .await
        .expect("deny");
    assert!(service.approval_rules.denies(nid, "command"));
    {
        let records = service.lock_sessions();
        let record = records.get(&key).expect("session");
        assert_eq!(record.denied_tools, ["command"]);
        assert!(record.allowed_tools.is_empty());
    }

    service
        .set_approval_rule(Some(&key), "command", true, false)
        .await
        .expect("allow");
    assert!(service.approval_rules.allows(nid, "command"));
    assert!(!service.approval_rules.denies(nid, "command"));
    {
        let records = service.lock_sessions();
        let record = records.get(&key).expect("session");
        assert!(record.denied_tools.is_empty());
        assert_eq!(record.allowed_tools, ["command"]);
    }

    assert!(
        service
            .set_approval_rule(Some(&key), " ", true, false)
            .await
            .is_err()
    );
    assert!(
        service
            .set_approval_rule(None, "command", true, false)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn skills_and_tools_are_listed_over_the_protocol() {
    let service = service();
    let skills = drain(&service, ClientRequestKind::ListSkills).await;
    assert!(matches!(skills[0].kind, ServerEventKind::SkillList { .. }));
    let refreshed = drain(&service, ClientRequestKind::RefreshSkills).await;
    assert!(matches!(
        refreshed[0].kind,
        ServerEventKind::SkillsUpdated { .. }
    ));
}

#[tokio::test]
async fn ask_user_parks_the_turn_until_the_client_answers() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "ask_user",
                &serde_json::json!({
                    "question": "Which file should I edit?",
                    "options": ["src/lib.rs", "src/main.rs"],
                    "multiple": false
                }),
            )
            .turn_text("I will wait for the answer."),
    );
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let request = ClientRequest {
        request_id: 7,
        kind: ClientRequestKind::SendMessage {
            session: None,
            text: "do the work".to_owned(),
            budget: None,
            reasoning: None,
        },
    };
    let mut stream = service.submit(request).await.expect("turn");
    let mut session = None;
    let mut asked = None;
    let mut finished = false;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("turn should not hang")
    {
        session = session.or(event.session.clone());
        if let ServerEventKind::QuestionAsked {
            question,
            prompt,
            options,
            multiple,
            ..
        } = &event.kind
        {
            asked = Some(question.clone());
            assert_eq!(prompt, "Which file should I edit?");
            assert_eq!(options, &["src/lib.rs", "src/main.rs"]);
            assert!(!multiple);
        }
        if matches!(event.kind, ServerEventKind::TurnFinished { .. }) {
            finished = true;
            break;
        }
        if let Some(question) = asked.clone() {
            let response = ClientRequest {
                request_id: 8,
                kind: ClientRequestKind::AnswerQuestion {
                    session: session.clone().expect("session"),
                    question,
                    answer: "src/lib.rs".to_owned(),
                },
            };
            let mut ack = service.submit(response).await.expect("answer");
            while ack.next().await.is_some() {}
            asked = None;
        }
    }
    assert!(finished, "the turn should finish after the answer");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_steer_during_a_running_turn_reaches_the_request_in_flight() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "ask_user",
                &serde_json::json!({
                    "question": "Which file should I edit?",
                    "options": ["src/lib.rs", "src/main.rs"],
                    "multiple": false
                }),
            )
            .turn_text("I will wait for the answer."),
    );
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory {
                model: model.clone(),
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let request = ClientRequest {
        request_id: 7,
        kind: ClientRequestKind::SendMessage {
            session: None,
            text: "do the work".to_owned(),
            budget: None,
            reasoning: None,
        },
    };
    let mut stream = service.submit(request).await.expect("turn");
    let mut session = None;
    let mut asked = None;
    let mut steered = false;
    let mut applied: Option<Vec<String>> = None;
    let mut finished = false;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("turn should not hang")
    {
        session = session.or(event.session.clone());
        if let ServerEventKind::QuestionAsked { question, .. } = &event.kind {
            asked = Some(question.clone());
        }
        if let ServerEventKind::SteerApplied { ids } = &event.kind {
            applied = Some(ids.clone());
        }
        if matches!(event.kind, ServerEventKind::TurnFinished { .. }) {
            finished = true;
            break;
        }
        if let Some(question) = asked.clone() {
            // The turn is alive and parked on the question: a steer sent now
            // reaches the request it is about to make.
            if !steered {
                steered = true;
                let ack = drain(
                    &service,
                    ClientRequestKind::Steer {
                        session: session.clone().expect("session"),
                        text: "also run the tests".to_owned(),
                        id: "steer-1".to_owned(),
                    },
                )
                .await;
                assert!(ack.is_empty());
            }
            let response = ClientRequest {
                request_id: 8,
                kind: ClientRequestKind::AnswerQuestion {
                    session: session.clone().expect("session"),
                    question,
                    answer: "src/lib.rs".to_owned(),
                },
            };
            let mut ack = service.submit(response).await.expect("answer");
            while ack.next().await.is_some() {}
            asked = None;
        }
    }
    assert!(finished, "the turn should finish after the answer");
    assert!(steered, "the steer should have been sent during the turn");
    assert_eq!(
        applied.as_deref(),
        Some(&["steer-1".to_owned()][..]),
        "the engine reports the message it applied, by name"
    );

    let requests = model.requests();
    assert_eq!(requests.len(), 2, "the turn made two requests");
    assert_eq!(
        user_texts(&requests[0]),
        vec!["do the work".to_owned()],
        "the first request carries nothing from the steer"
    );
    assert_eq!(
        user_texts(&requests[1]),
        vec!["do the work".to_owned(), "also run the tests".to_owned()],
        "the steer reaches the request the turn was about to make"
    );

    // The durable copy is gone once the engine applied it.
    let key = session.expect("session");
    let (_, _, _) = service.require_record(&key).expect("record");
    assert!(
        service
            .lock_sessions()
            .get(&key)
            .expect("record")
            .pending_steers
            .is_empty(),
        "the applied steer leaves no pending copy"
    );
}

#[tokio::test]
async fn an_unknown_session_is_an_error() {
    let service = service();
    let error = service
        .submit(ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::SendMessage {
                session: Some(SessionKey::new("missing")),
                text: "hi".to_owned(),
                budget: None,
                reasoning: None,
            },
        })
        .await;
    assert!(error.is_err());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_parked_approval_survives_a_restart() {
    let directory = std::env::temp_dir().join(format!(
        "norte-code-approval-restart-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&directory).expect("directory");
    let path = directory.join("sessions.db");
    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!(
            "norte-code-restart-{}-{}.txt",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
    let _ = std::fs::remove_file(&target);
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "write",
                &serde_json::json!({
                    "path": target.display().to_string(),
                    "content": "survived",
                    "overwrite": true
                }),
            )
            .turn_text("done"),
    );

    let (store, first) = sqlite_service(&path, Arc::clone(&model)).await;
    let events = drain(
        &first,
        ClientRequestKind::SendMessage {
            session: None,
            text: "write it".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let session = events[0].session.clone().expect("session");
    let approval = events
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ApprovalRequested { approval, .. } => Some(approval.clone()),
            _ => None,
        })
        .expect("approval request");
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::TurnFinished {
            phase: norte_code_protocol::Phase::AwaitingApproval,
            ..
        }
    )));
    drop(first);
    drop(store);

    let (store, second) = sqlite_service(&path, model).await;
    let listed = drain(&second, ClientRequestKind::ListSessions).await;
    let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
        panic!("expected a list");
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session, session);
    assert_eq!(
        sessions[0].phase,
        norte_code_protocol::Phase::AwaitingApproval
    );

    let resumed = drain(
        &second,
        ClientRequestKind::Resume {
            session: session.clone(),
        },
    )
    .await;
    assert!(resumed.iter().any(|event| matches!(
        &event.kind,
        ServerEventKind::ApprovalRequested { approval: pending, .. } if pending == &approval
    )));

    let approved = drain(
        &second,
        ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: None,
            remember: None,
        },
    )
    .await;
    assert!(approved.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::ToolFinished {
            ref tool,
            is_error: false,
            ..
        } if tool == "write"
    )));
    assert_eq!(
        std::fs::read_to_string(&target).expect("written file"),
        "survived"
    );
    drop(second);
    drop(store);
    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn a_write_that_started_before_a_crash_is_not_repeated() {
    let directory = std::env::temp_dir().join(format!(
        "norte-code-effect-restart-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&directory).expect("directory");
    let path = directory.join("sessions.db");
    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!(
            "norte-code-effect-{}-{}.txt",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
    let _ = std::fs::remove_file(&target);
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "write",
                &serde_json::json!({
                    "path": target.display().to_string(),
                    "content": "should not appear",
                    "overwrite": true
                }),
            )
            .turn_text("done"),
    );

    let (store, first) = sqlite_service(&path, Arc::clone(&model)).await;
    let events = drain(
        &first,
        ClientRequestKind::SendMessage {
            session: None,
            text: "write it".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;
    let session = events[0].session.clone().expect("session");
    let approval = events
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ApprovalRequested { approval, .. } => Some(approval.clone()),
            _ => None,
        })
        .expect("approval request");
    let north = store
        .sessions()
        .await
        .expect("stored sessions")
        .first()
        .expect("one session")
        .north;
    // Pretend the previous process died after the effect but before the
    // result was recorded.
    store
        .effect_started(north, call, "write", "crashed-process")
        .expect("intent");
    drop(first);
    drop(store);

    let (store, second) = sqlite_service(&path, model).await;
    let approved = drain(
        &second,
        ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: None,
            remember: None,
        },
    )
    .await;
    assert!(approved.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::ToolFinished {
            ref tool,
            is_error: true,
            ..
        } if tool == "write"
    )));
    assert!(
        !target.exists(),
        "a write that may have run before the crash must not run again"
    );
    assert!(
        store
            .unfinished_effects()
            .await
            .expect("journal")
            .is_empty(),
        "the refusal is recorded as a finished effect"
    );
    drop(second);
    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_pending_question_survives_a_restart() {
    let directory = std::env::temp_dir().join(format!(
        "norte-code-question-restart-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&directory).expect("directory");
    let path = directory.join("sessions.db");
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "ask_user",
                &serde_json::json!({
                    "question": "Which file?",
                    "options": ["src/lib.rs", "src/main.rs"],
                    "multiple": true
                }),
            )
            .turn_text("done"),
    );

    let (store, first) = sqlite_service(&path, Arc::clone(&model)).await;
    let request = ClientRequest {
        request_id: 7,
        kind: ClientRequestKind::SendMessage {
            session: None,
            text: "go".to_owned(),
            budget: None,
            reasoning: None,
        },
    };
    let mut stream = first.submit(request).await.expect("turn");
    let mut session = None;
    let mut asked = None;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the turn should park")
    {
        session = session.or(event.session.clone());
        if let ServerEventKind::QuestionAsked { question, .. } = &event.kind {
            asked = Some(question.clone());
            break;
        }
    }
    let session = session.expect("session");
    let asked = asked.expect("question");
    drop(stream);
    drop(first);
    drop(store);

    let (store, second) = sqlite_service(&path, model).await;
    let mut resumed = second
        .submit(ClientRequest {
            request_id: 8,
            kind: ClientRequestKind::Resume {
                session: session.clone(),
            },
        })
        .await
        .expect("resume");

    let mut reasked = None;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), resumed.next())
        .await
        .expect("the resume should re-ask")
    {
        if let ServerEventKind::QuestionAsked {
            question,
            options,
            multiple,
            ..
        } = &event.kind
        {
            assert_eq!(options, &["src/lib.rs", "src/main.rs"]);
            assert!(multiple);
            reasked = Some(question.clone());
            break;
        }
    }
    assert_eq!(reasked.as_deref(), Some(asked.as_str()));

    let answered = drain(
        &second,
        ClientRequestKind::AnswerQuestion {
            session: session.clone(),
            question: asked,
            answer: "src/lib.rs".to_owned(),
        },
    )
    .await;
    assert!(matches!(
        answered[0].kind,
        ServerEventKind::QuestionAnswered { .. }
    ));

    let mut finished = false;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), resumed.next())
        .await
        .expect("the resume should finish")
    {
        if matches!(event.kind, ServerEventKind::TurnFinished { .. }) {
            finished = true;
            break;
        }
    }
    assert!(finished, "the resumed turn should finish after the answer");
    drop(second);
    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

async fn sqlite_service(
    path: &std::path::Path,
    model: Arc<ScriptedModel>,
) -> (Arc<SqliteStore>, Arc<Service>) {
    let store = Arc::new(SqliteStore::open(path).expect("open"));
    let session_store: Arc<dyn Store> = store.clone();
    let blobs: Arc<dyn BlobStore> = store.clone();
    let directory: Arc<dyn SessionDirectory> = store.clone();
    let service = Service::with_directory(
        config(),
        catalog(),
        spec("scripted"),
        Arc::new(ScriptedFactory { model }),
        session_store,
        blobs,
        directory,
        "system",
    )
    .await
    .expect("service");
    (store, Arc::new(service))
}

/// A symlink inside the workspace must not become a way out of it, even
/// when the tool call goes through the whole engine.
///
/// The unit tests above exercise the policy directly. This one drives a
/// real `read` call through the engine, so the denial is proven on the
/// path a model actually takes.
#[cfg(unix)]
#[tokio::test]
async fn a_read_through_an_escaping_symlink_is_denied_by_the_engine() {
    let base = std::env::temp_dir().join(format!(
        "norte-escape-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let outside = base.join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::write(outside.join("secret"), "do not read").expect("secret");

    // The service takes its workspace root from the current directory, so
    // the symlink has to live in it.
    let root = std::env::current_dir().expect("working directory");
    let link = root.join(format!("norte-escape-link-{}.tmp", uuid::Uuid::new_v4()));
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");

    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                call,
                "read",
                &serde_json::json!({ "path": link.join("secret").display().to_string() }),
            )
            .turn_text("done"),
    );
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );

    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "read it".to_owned(),
            budget: None,
            reasoning: None,
        },
    )
    .await;

    let finished = events
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::ToolFinished { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("the read should have been attempted and failed");
    let message = finished.unwrap_or_default();
    assert!(
        message.contains("outside the workspace root"),
        "the escape must be refused, not read: {message}"
    );

    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_dir_all(&base);
}
