use super::compaction::recent_start;
use super::jobs::{JOB_BUFFER_BYTES, JobRegistry, JobState};
use super::mapping::retry_plan;
#[cfg(test)]
use super::prompt::MAX_SKILLS_BLOCK_CHARS;
use super::sandbox::sandbox_name;
use super::titling::TITLE_PROMPT;
use super::*;
use norte::{FinishReason, Role};
use std::collections::BTreeSet;
use std::path::Path;

use crate::provider::Wire;
use async_trait::async_trait;
use futures_util::StreamExt;
use norte::harness::{InMemoryBlobStore, InMemoryStore};
use norte::harness::{Interception, InterceptionContext, TurnInterceptor};
use norte::{
    CallId, Capabilities, Message, ModelId, ModelRef, ModelRequest, ModelStream, ProviderErrorKind,
    ProviderId, Reasoning as ReasoningCapability, ReasoningEffort, ReasoningRequest, SessionId,
    ToolContext, Usage,
};
use norte_code_config::{ApiKey, BudgetConfig, RetryConfig};
use norte_code_protocol::{
    Budget, BudgetUnit, ClientRequest, ClientRequestKind, Delta, ErrorCode,
    Reasoning as ProtocolReasoning, ReasoningScope,
};
use norte_code_store_sqlite::SqliteStore;
use norte_testing::ScriptedModel;
use norte_tools::{ShellLimits, ShellRequest, ShellTool};

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
        no_prompt_cache_key: false,
        api_key_env: None,
        capabilities: norte_code_catalog::capabilities(entry, Wire::Chat, None),
        headers: std::collections::BTreeMap::new(),
    }
}

/// A key file no other test shares.
///
/// Every call gets its own name. They used to be one file per process, and
/// `std::fs::write` truncates before it writes, so a test that read the file
/// while another rewrote it saw an empty one: the key then fell through to the
/// catalogue's `env`, and `a_model_switch_while_parked_is_applied_at_the_next_turn_boundary`
/// failed with "no value in any of: `TEST_API_KEY`" in a full run while passing on
/// its own. A unique name per call is what makes the tests independent; the
/// atomic write means even a shared name could not be seen half-written.
fn key_file(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "norte-code-service-{}-{tag}-{}-key",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, "sk-test\n").expect("key file");
    std::fs::rename(&tmp, &path).expect("key file");
    path
}

fn config() -> Config {
    let key_file = key_file("default");
    Config::from_json(&format!(
        r#"{{"providers":{{"test":{{"api_key_file":"{}"}}}}}}"#,
        key_file.display()
    ))
    .expect("config")
    .config
}

#[tokio::test]
#[ignore = "starts a real MCP server over stdio; needs node and the network"]
async fn a_real_mcp_server_publishes_its_tools_into_the_service() {
    // The path a user takes: a server named in `mcpServers`, started at
    // startup, its tools in front of the model.
    let mut config = config();
    config.mcp_servers.insert(
        "everything".to_owned(),
        norte_code_config::McpServerConfig {
            command: "npx".to_owned(),
            args: vec![
                "-y".to_owned(),
                "@modelcontextprotocol/server-everything".to_owned(),
            ],
            ..norte_code_config::McpServerConfig::default()
        },
    );
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("done"));
    let service = Arc::new(
        Service::with_directory(
            config,
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            Arc::new(InMemoryDirectory::new()),
            "system",
        )
        .await
        .expect("service"),
    );

    let status = service.mcp_status();
    assert_eq!(status.len(), 1, "one server connected: {status:?}");
    let server = &status[0];
    assert_eq!(server.name, "everything");
    assert!(!server.tools.is_empty(), "the server published tools");
    // The names are the point of reporting them: they are what a person asks
    // the model for, and every one of them carries the namespace.
    assert!(
        server
            .tools
            .iter()
            .all(|tool| tool.starts_with("mcp__everything__")),
        "every tool is namespaced: {:?}",
        server.tools
    );

    // The tools are in the registry the engine reads, with the namespace.
    let registry = service.tools().expect("registry");
    let specs = registry.specs();
    let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("mcp__everything__")),
        "an MCP tool is registered: {names:?}"
    );
    assert!(
        service.mcp_failures().is_empty(),
        "no server failed: {:?}",
        service.mcp_failures()
    );
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

#[tokio::test]
async fn a_session_reports_the_model_reference_it_is_on() {
    // `/provider` swaps the provider and keeps the model, so the client has to
    // read the reference from the session: during a turn the renderer's own name
    // is the one already in flight, and there is nowhere else to ask.
    let (service, _model) = scripted_service(spec("scripted"), &["hello"]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");

    assert_eq!(
        service.session_model(&key).as_deref(),
        Some("test/scripted"),
        "a session names the provider and the model it is on"
    );

    let unknown = SessionKey::new("00000000-0000-0000-0000-000000000000".to_owned());
    assert_eq!(
        service.session_model(&unknown),
        None,
        "a session that does not exist has no model"
    );
}

#[tokio::test]
async fn a_compaction_summariser_can_be_a_configured_provider() {
    // `config.compaction.model` names the model that writes the summary, and it
    // is free to be a private endpoint. Resolving it through the catalogue alone
    // meant a session configured with one could not compact at all — and the
    // failure would arrive exactly when the context was full and room had to be
    // made, which is the worst moment to have none.
    let mut config = config();
    config.compaction.model = Some("private/model-x".to_owned());
    config.providers.insert(
        "private".to_owned(),
        norte_code_config::ProviderConfig {
            endpoint: Some("https://example.invalid/v1".to_owned()),
            wire: Some(norte_code_config::WireKind::Chat),
            api_key_file: Some(key_file("private")),
            ..norte_code_config::ProviderConfig::default()
        },
    );
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
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

    let resolved = service
        .summary_spec_for_a_test(&spec("scripted"))
        .expect("the configured summariser resolves");
    assert_eq!(
        resolved.label(),
        "private/model-x",
        "the summariser is the one the config named, not the session's model"
    );
}

#[tokio::test]
async fn a_model_switch_accepts_a_configured_provider() {
    // The same reference `run --provider private` resolves, asked the other way.
    // `/model private/model-x` went straight to the catalogue — which does not
    // list a private endpoint — and answered "unknown provider", so the provider
    // that worked from the command line could not be switched to from the chat.
    let mut config = config();
    config.providers.insert(
        "private".to_owned(),
        norte_code_config::ProviderConfig {
            endpoint: Some("https://example.invalid/v1".to_owned()),
            wire: Some(norte_code_config::WireKind::Chat),
            api_key_file: Some(key_file("private")),
            ..norte_code_config::ProviderConfig::default()
        },
    );
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
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
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");

    let events = drain(
        &service,
        ClientRequestKind::SetModel {
            session: key,
            model: "private/model-x".to_owned(),
        },
    )
    .await;
    assert!(
        !events.iter().any(|event| matches!(
            &event.kind,
            ServerEventKind::Error { message, .. } if message.contains("unknown provider")
        )),
        "a configured provider resolves without the catalogue: {:?}",
        events.iter().map(|event| &event.kind).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_session_saved_on_a_configured_provider_reopens_on_it() {
    // The worst of the three, and the only silent one: a restored session whose
    // model the catalogue does not list fell back to the default, so `exa` work
    // came back as whatever model the config named last. The next message would
    // go somewhere the work never was.
    let dir = std::env::temp_dir().join(format!("norte-reopen-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let key_file = dir.join("private.key");
    std::fs::write(&key_file, "sk-private\n").expect("key file");

    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
    let (store, _service) = sqlite_service(&dir.join("sessions.db"), model).await;

    // A session on file, on a provider the catalogue does not list.
    let stored = norte_code_store_sqlite::StoredSession {
        key: "exa-session".to_owned(),
        north: norte::SessionId::new(),
        title: None,
        model: "private/model-x".to_owned(),
        created_at: chrono::Utc::now(),
        phase: "idle".to_owned(),
        stats: serde_json::Value::Null,
        denied_tools: Vec::new(),
        allowed_tools: Vec::new(),
        compacted_from: None,
        workdir: None,
    };
    store.save_session(&stored).await.expect("store");

    // A service built over that directory has to bring it back on its own
    // model, not on the default.
    let config_text = format!(
        r#"{{"providers":{{"private":{{"endpoint":"https://example.invalid/v1","wire":"chat","api_key_file":"{}"}}}}}}"#,
        key_file.display()
    );
    let config = norte_code_config::Config::from_json(&config_text)
        .expect("config")
        .config;
    let session_store: Arc<dyn Store> = store.clone();
    let blobs: Arc<dyn BlobStore> = store.clone();
    let directory: Arc<dyn SessionDirectory> = store.clone();
    let model = Arc::new(ScriptedModel::new(ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    )));
    let reopened = Service::with_directory(
        config,
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

    let key = SessionKey::new("exa-session".to_owned());
    assert_eq!(
        reopened.session_model(&key).as_deref(),
        Some("private/model-x"),
        "the session reopened on the model it was saved with"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_reload_accepts_a_default_model_from_a_configured_provider() {
    // A private endpoint is declared in `config.json` and nowhere else, so the
    // catalogue does not list its provider. A run resolves one of those without
    // complaint; the reload went straight to the catalogue and answered the same
    // reference with "unknown provider", which made `/refresh` fail on a config
    // that worked.
    let (service, _model) = scripted_service(spec("scripted"), &["hello"]);
    let dir = std::env::temp_dir().join(format!("norte-reload-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("config.json");
    let key_file = dir.join("private.key");
    std::fs::write(&key_file, "sk-private\n").expect("key file");
    // `private` is configured with an endpoint and a key file, and it is not in
    // the catalogue the service was built with.
    std::fs::write(
        &path,
        format!(
            r#"{{"default_model":"private/model-x","providers":{{"private":{{"endpoint":"https://example.invalid/v1","wire":"chat","api_key_file":"{}"}}}}}}"#,
            key_file.display()
        ),
    )
    .expect("write");
    service.set_config_path(Some(path.clone()));

    let reloaded = service
        .reload_config()
        .expect("a configured provider resolves without the catalogue");
    assert_eq!(
        reloaded.config.default_model.as_deref(),
        Some("private/model-x")
    );
    assert_eq!(
        service.default_spec().label(),
        "private/model-x",
        "the default model the config named is the one in force"
    );

    // A reference that names neither the catalogue nor the config is still an
    // error: falling back is for a declared provider, not for a typo.
    std::fs::write(&path, r#"{"default_model":"nowhere/model-x"}"#).expect("write");
    let error = service
        .reload_config()
        .expect_err("an undeclared provider is still refused");
    assert!(
        matches!(error, ServiceError::UnknownProvider(_)),
        "the failure names the provider: {error:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_reload_applies_a_good_config_and_discards_a_bad_one() {
    let (service, _model) = scripted_service(spec("scripted"), &["hello"]);
    let dir = std::env::temp_dir().join(format!("norte-reload-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("config.json");
    std::fs::write(&path, r#"{"approval_mode":"yolo","max_iterations":7}"#).expect("write");
    service.set_config_path(Some(path.clone()));

    let reloaded = service.reload_config().expect("a valid config reloads");
    assert_eq!(reloaded.config.max_iterations, Some(7));
    assert_eq!(service.config().max_iterations, Some(7));

    // A file that does not parse leaves the service on the config it had.
    std::fs::write(&path, "{ not json").expect("write");
    let error = service
        .reload_config()
        .expect_err("a broken file is refused");
    assert!(
        matches!(error, ServiceError::Config(_)),
        "the failure names the config: {error:?}"
    );
    assert_eq!(
        service.config().max_iterations,
        Some(7),
        "the last good config stays in force"
    );

    let _ = std::fs::remove_dir_all(&dir);
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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

#[tokio::test]
async fn a_resume_sends_the_task_list_the_session_last_had() {
    // The list lives in the model's `todo` call, which is committed with the
    // conversation, so a resumed client can be told what it was. It used to
    // come back empty: the state behind the widget is in memory, and nothing
    // read the call back, so a session resuming mid-task drew no tasks and a
    // compaction before the model wrote them again summarised over the gap.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("done"));
    let store = Arc::new(SqliteStore::in_memory().expect("store"));
    let build = |store: Arc<SqliteStore>| {
        let model = Arc::clone(&model) as Arc<dyn Model>;
        async move {
            Service::with_directory(
                config(),
                catalog(),
                spec("scripted"),
                Arc::new(ScriptedFactory { model }),
                store.clone(),
                store.clone(),
                store,
                "system",
            )
            .await
            .expect("service")
        }
    };
    let service = Arc::new(build(store.clone()).await);
    let created = drain(
        &service,
        ClientRequestKind::CreateSession {
            title: Some("tasks".to_owned()),
        },
    )
    .await;
    let key = created[0].session.clone().expect("session");
    let (nid, _, _) = service.require_record(&key).expect("record");

    // Write a list the way the model does: a committed `todo` call.
    let call = CallId::new();
    let engine = service.engine(&spec("scripted")).expect("engine");
    let node = norte::harness::Node::message(
        Message::new(
            Role::Assistant,
            vec![norte::Part::ToolCall(norte::ToolCallPart {
                call_id: call,
                name: "todo".to_owned(),
                arguments: serde_json::json!({
                    "todos": [
                        {"content": "first", "status": "completed"},
                        {"content": "second", "status": "in_progress"},
                        {"content": "third", "status": "pending"},
                    ]
                }),
                continuity: None,
            })],
        ),
        None,
    );
    let session = engine.session(nid).await.expect("session");
    // The head has to be the node itself: a commit with `head: None` leaves the
    // session where it was, which for a fresh one is nowhere, and the history
    // read back below would be empty. `Idle` is what a finished assistant turn
    // leaves, so the resume reads the history instead of trying to run the call
    // the model already made.
    let head = node.id;
    store
        .commit(norte::harness::Commit {
            session_id: nid,
            expected_version: Some(session.version),
            nodes: vec![node],
            head: Some(head),
            phase: norte::harness::Phase::Idle,
            turn: session.turn,
            grants: session.grants.clone(),
            metadata: None,
        })
        .await
        .expect("commit the todo call");

    // A fresh service over the same store is a relaunch; the resume is what
    // asks for the list back.
    let reopened = Arc::new(build(store).await);
    let events = drain(
        &reopened,
        ClientRequestKind::Resume {
            session: key.clone(),
        },
    )
    .await;
    let todos = events.iter().find_map(|event| match &event.kind {
        ServerEventKind::SessionTodos { todos } => Some(todos.clone()),
        _ => None,
    });
    let todos = todos.expect("a resume sends the list the session last had");
    assert_eq!(todos.len(), 3, "{todos:?}");
    assert_eq!(todos[0].content, "first");
    assert_eq!(
        todos[1].status,
        norte_code_protocol::TodoStatus::InProgress,
        "the status comes across, not just the text"
    );
    assert_eq!(todos[2].content, "third");
}

#[tokio::test]
async fn a_session_can_be_named_by_the_prefix_the_footer_shows() {
    // The footer draws the first eight characters of the id, and `resume` took
    // the argument straight to the session table: the name on screen answered
    // "not known". A prefix that matches one session resolves to it; one that
    // matches several is refused with the ids, because continuing the wrong
    // conversation is silent; and one that matches none still says unknown.
    let service = service();
    let created = drain(
        &service,
        ClientRequestKind::CreateSession {
            title: Some("named by a prefix".to_owned()),
        },
    )
    .await;
    let key = created[0].session.clone().expect("session");
    let prefix: String = key.as_str().chars().take(8).collect();

    let listed = drain(
        &service,
        ClientRequestKind::SwitchSession {
            session: SessionKey::new(prefix.clone()),
        },
    )
    .await;
    assert!(
        matches!(listed[0].kind, ServerEventKind::SessionSwitched),
        "the footer's eight characters name the session: {listed:#?}"
    );

    // A name nothing starts with is still unknown, with the message people
    // already know, so this is a widening and not a new failure mode.
    let result = service
        .submit(ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::SwitchSession {
                session: SessionKey::new("ffffffff".to_owned()),
            },
        })
        .await;
    assert!(
        matches!(result, Err(ServiceError::UnknownSession(name)) if name == "ffffffff"),
        "an unmatched prefix is unknown"
    );
}

#[tokio::test]
async fn a_prefix_that_matches_two_sessions_is_refused_by_name() {
    // Two sessions sharing a prefix cannot be told apart, and taking the first
    // would continue one of them without saying which. The error names both, so
    // the person can pick.
    let service = service();
    let mut keys = Vec::new();
    for title in ["one", "two"] {
        let created = drain(
            &service,
            ClientRequestKind::CreateSession {
                title: Some(title.to_owned()),
            },
        )
        .await;
        keys.push(created[0].session.clone().expect("session"));
    }
    // Two ids from the same table share a leading hex digit often enough that a
    // one-character prefix is the honest way to write this test; when it does
    // not, the two are the same length and the empty prefix still matches both.
    let first = keys[0].as_str();
    let second = keys[1].as_str();
    let shared = first
        .chars()
        .zip(second.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let prefix: String = first.chars().take(shared).collect();

    let result = service
        .submit(ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::SwitchSession {
                session: SessionKey::new(prefix.clone()),
            },
        })
        .await;
    match result {
        Err(ServiceError::AmbiguousSession { matches, .. }) => {
            assert_eq!(matches.len(), 2, "both sessions are named: {matches:?}");
        }
        Err(other) => panic!("expected an ambiguous name, got {other}"),
        Ok(_) => panic!("two sessions under one prefix must not resolve"),
    }
}

#[tokio::test]
async fn a_failed_turn_leaves_the_session_able_to_switch_models() {
    // The provider fails for good: no retry is scheduled, and the turn ends
    // with an error. The engine left the session `Streaming` so a resume could
    // pick it up, but this process has no retry left and never lowers it — so
    // before this was settled, `/model` queued behind an idle boundary the
    // session could no longer reach: the switch was accepted and never applied.
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
    let (service, _store) = persistent_service_with_config(config(), model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("the failed turn still names its session");
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::TurnFinished {
            committed: false,
            ..
        }
    )));

    // The switch goes through — and, being a boundary switch, applies now
    // instead of being queued as a pending model.
    let changed = drain(
        &service,
        ClientRequestKind::SetModel {
            session: key.clone(),
            model: "test/other".to_owned(),
        },
    )
    .await;
    assert!(
        matches!(changed[0].kind, ServerEventKind::ModelChanged { .. }),
        "a failed turn must not leave the session busy: {changed:#?}"
    );
    let (_, spec, _) = service.require_record(&key).expect("record");
    assert_eq!(spec.model, "other");
}

#[tokio::test]
async fn a_session_left_streaming_by_a_kill_is_settled_when_it_reopens() {
    // A process that dies mid-turn leaves `Phase::Streaming` in the store, and
    // no process is streaming a session it just loaded. The repair commits the
    // phase back to idle; it used to carry the session's own head, which a
    // commit with no nodes rejects, so the write failed and the session stayed
    // streamable-forever — the store, not just the directory row, was the thing
    // `/model` read.
    let (service, store) = persistent_service_with_config(config(), scripted_model()).await;
    let created = drain(
        &service,
        ClientRequestKind::CreateSession {
            title: Some("killed".to_owned()),
        },
    )
    .await;
    let key = created[0].session.clone().expect("session");
    let (nid, _, _) = service.require_record(&key).expect("record");

    // The head the repair must preserve, and a store left mid-stream. The
    // session already exists (creating it committed once), so the write carries
    // the version it holds.
    let current = store.load(nid).await.expect("session");
    let node = norte::harness::Node::message(Message::user("committed"), current.head);
    let head = node.id;
    store
        .commit(norte::harness::Commit {
            session_id: nid,
            expected_version: Some(current.version),
            nodes: vec![node],
            head: Some(head),
            phase: norte::harness::Phase::Streaming,
            turn: 1,
            grants: Vec::new(),
            metadata: None,
        })
        .await
        .expect("park the session mid-stream");

    // A fresh service over the same store is what a relaunch looks like.
    let reopened = Service::with_directory(
        config(),
        catalog(),
        spec("scripted"),
        Arc::new(ScriptedFactory {
            model: scripted_model(),
        }),
        store.clone(),
        store.clone(),
        store.clone(),
        "system",
    )
    .await
    .expect("reopen");

    let session = store.load(nid).await.expect("session");
    assert_eq!(
        session.phase,
        norte::harness::Phase::Idle,
        "reopening must settle a session no process is streaming"
    );
    assert_eq!(
        session.head,
        Some(head),
        "settling the phase must not drop the conversation"
    );
    let (_, _, phase) = reopened.require_record(&key).expect("record");
    assert_eq!(phase, norte_code_protocol::Phase::Idle);
}

/// A model that answers with one text turn, for the tests that care about the
/// session's state rather than about the answer.
fn scripted_model() -> Arc<dyn Model> {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    Arc::new(ScriptedModel::new(reference).turn_text("done"))
}

#[cfg(target_os = "linux")]
#[test]
fn the_command_sandbox_wraps_the_original_request_without_losing_it() {
    let workspace = PathBuf::from("/work");
    let arguments = serde_json::json!({
        "script": "echo hi",
        "shell": "sh",
        "cwd": "/work/sub",
        "env": {"A": "B"},
        "timeout_ms": 1_000
    });
    let wrapped = sandboxed_arguments(&arguments, &workspace, false, &[PathBuf::from("/cache")])
        .expect("wrap");
    let command = wrapped.as_object().expect("the request stays flat");
    assert_eq!(command["shell"], "sh");
    let script = command["script"].as_str().expect("script");
    assert!(
        script.starts_with("bwrap"),
        "the sandbox becomes the program: {script}"
    );
    let args: Vec<&str> = script.split_whitespace().collect();
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
    assert_eq!(wrapped["cwd"], "/work/sub");
    assert_eq!(wrapped["env"]["A"], "B");
    assert_eq!(wrapped["timeout_ms"], 1_000);

    // Same request, network allowed: the sandbox gains the sharing flag.
    let networked = sandboxed_arguments(&arguments, &workspace, true, &[]).expect("networked");
    let script = networked["script"].as_str().expect("script");
    assert!(
        script.split_whitespace().any(|word| word == "--share-net"),
        "the network is shared when asked: {script}"
    );

    // A program with arguments is a script too, so the only difference is what
    // the original text says. The sandbox still goes in front of it.
    let program = serde_json::json!({ "script": "cargo test --all" });
    let wrapped = sandboxed_arguments(&program, &workspace, false, &[]).expect("program");
    let script = wrapped["script"].as_str().expect("script");
    assert!(
        script.contains("cargo test --all"),
        "the original line survives: {script}"
    );

    // The typed path produces the same request the JSON path does.
    let request: ShellRequest = serde_json::from_value(arguments).expect("request");
    let wrapped = sandbox_request(&request, &workspace, false, &[]).expect("request wrap");
    let value = serde_json::to_value(&wrapped).expect("value");
    assert_eq!(sandbox_name(), "bwrap");
    assert!(
        value["script"]
            .as_str()
            .is_some_and(|script| script.starts_with("bwrap")),
        "the sandbox is the program now: {value}"
    );

    // A request with no script cannot be wrapped, and says so.
    let broken = serde_json::json!({ "shell": "sh" });
    assert!(sandboxed_arguments(&broken, &workspace, false, &[]).is_err());
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
async fn a_rename_survives_the_process_that_made_it() {
    // `/rename` is worth nothing if the name is only in memory: the point of a
    // title is that the next run lists the session by it. The store is the only
    // thing that outlives the process, so the name is read back from the store
    // and not from the service that wrote it.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("hello"));
    let (service, store) = persistent_service(model).await;
    let created = drain(
        &service,
        ClientRequestKind::CreateSession {
            title: Some("before".to_owned()),
        },
    )
    .await;
    let key = created[0].session.clone().expect("session");

    drain(
        &service,
        ClientRequestKind::RenameSession {
            session: key.clone(),
            title: "fix the footer overflow".to_owned(),
        },
    )
    .await;

    // Read it back from the database, which is what the next process does.
    let rows = store.sessions().await.expect("sessions");
    let row = rows
        .iter()
        .find(|row| row.key == key.as_str())
        .expect("the session is in the store");
    assert_eq!(row.title.as_deref(), Some("fix the footer overflow"));
}

#[tokio::test]
async fn a_title_from_the_model_is_what_the_store_holds() {
    // The automatic path and the manual one have to agree: `/rename` uses the
    // rename request, and `title_session` writes through the same `rename`, so
    // both end up as the session's `title` rather than two names that disagree
    // depending on which path set it.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("fix the footer overflow"));
    let (service, store) = persistent_service(model).await;
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");

    let title = service
        .title_session(&key, "the footer overflows when the terminal is narrow")
        .await
        .expect("a title");
    assert_eq!(title, "fix the footer overflow");

    let rows = store.sessions().await.expect("sessions");
    let row = rows
        .iter()
        .find(|row| row.key == key.as_str())
        .expect("the session is in the store");
    assert_eq!(
        row.title.as_deref(),
        Some(title.as_str()),
        "what the model wrote is what the store holds"
    );
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                replay: false,
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
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                replay: false,
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
    assert_eq!(system.len(), 1);
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

#[test]
fn an_open_gate_hands_the_path_through_untouched() {
    // With `tools.confinement.workspace_only` off the policy does not resolve
    // the argument, so a path outside the workspace is passed as written. The
    // policy itself is covered end to end by the CLI tests; what matters here
    // is that the gate is what decides, and that a special file is still
    // refused because that check is not the root check.
    let root = Path::new("/tmp/norte-workspace");
    let outside = serde_json::json!({"path": "/etc/hosts", "content": "x"});

    // Gate on: the escape is refused.
    assert!(
        transform_path(&outside, "path", root, false).is_err(),
        "the gate refuses a path outside the root"
    );

    // Gate off: the same arguments reach the tool as written.
    let arguments = outside.clone();
    assert_eq!(
        arguments.get("path").and_then(serde_json::Value::as_str),
        Some("/etc/hosts")
    );
    // And the kind check still rejects a special file, whoever paid for it.
    assert!(
        check_path_kind(
            &serde_json::json!({"path": "/dev/null"}),
            "path",
            PathKind::File
        )
        .is_err()
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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

/// An interceptor that lets `after` consults through and stops the next one.
///
/// The first consult of a turn runs before the input is appended, when the
/// session is still idle; stopping there leaves it idle, which is not what most
/// of these tests are about. `nth(2)` stops on the consult that follows
/// `append_turn`, when the turn is already `Streaming` — the leftover this file
/// asserts the host settles.
struct StopAtConsult {
    reason: String,
    after: usize,
    seen: std::sync::atomic::AtomicUsize,
}

impl StopAtConsult {
    fn nth(after: usize, reason: &str) -> Self {
        Self {
            reason: reason.to_owned(),
            after,
            seen: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl TurnInterceptor for StopAtConsult {
    fn name(&self) -> &'static str {
        "stop-at-consult"
    }

    async fn intercept(&self, _context: &InterceptionContext<'_>) -> norte::Result<Interception> {
        let seen = self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if seen < self.after {
            return Ok(Interception::Continue);
        }
        Ok(Interception::Stop {
            reason: self.reason.clone(),
        })
    }
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
    // The turn is over and nothing is streaming it: a session left `Streaming`
    // here would refuse every later `/model` with a queued switch that no
    // boundary could apply.
    let finished = events.iter().find_map(|event| match &event.kind {
        ServerEventKind::TurnFinished { phase, .. } => Some(*phase),
        _ => None,
    });
    assert_eq!(finished, Some(norte_code_protocol::Phase::Idle));
}

#[tokio::test]
async fn a_stop_after_the_input_is_appended_leaves_the_session_idle() {
    // The stop here lands after `append_turn`, so the engine has already
    // committed `Streaming` and hands the turn back as an `Ok` outcome — not an
    // error. A host that only settled the failure path left this one streaming,
    // and every `/model` after it queued behind a boundary that never came.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("never"));
    // `with_interceptor` builds the service, so the store is made here and
    // handed to both the interceptor's service and the assertions.
    let store = Arc::new(SqliteStore::in_memory().expect("store"));
    let service = Arc::new(
        Service::with_directory(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory {
                model: Arc::clone(&model) as Arc<dyn Model>,
            }),
            store.clone(),
            store.clone(),
            store.clone(),
            "system",
        )
        .await
        .expect("service")
        .with_interceptor(Arc::new(StopAtConsult::nth(1, "the host said no"))),
    );

    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    assert_eq!(
        events.iter().find_map(|event| match &event.kind {
            ServerEventKind::TurnFinished { phase, .. } => Some(*phase),
            _ => None,
        }),
        Some(norte_code_protocol::Phase::Idle),
        "{events:#?}"
    );
    let (nid, _, _) = service.require_record(&key).expect("record");
    assert_eq!(
        store.load(nid).await.expect("session").phase,
        norte::harness::Phase::Idle,
        "the store, not just the reported phase, must be idle"
    );

    // The boundary is usable: the switch applies now instead of queueing.
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
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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

/// The same, with the prompt tokens the provider served from its cache.
fn cached_usage(input: u64, output: u64, cache_read: u64, cost: Option<f64>) -> Usage {
    let mut usage = usage(input, output, cost);
    usage.cache_read_tokens = Some(cache_read);
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
    // The summary travels with the event: it is what the fresh session was
    // seeded with, and it is the only way a client can show what the history
    // became. The model was scripted to answer it, so it must come back whole.
    let summary = events
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::CompactionFinished { summary, .. } => summary.clone(),
            _ => None,
        })
        .expect("the summary reaches the client");
    assert_eq!(summary, "the user wants a compacted handoff");
    // The events told the truth in order: it started, then it finished.
    assert!(matches!(
        events[0].kind,
        ServerEventKind::CompactionStarted { .. }
    ));
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
fn the_skills_block_is_bounded_and_says_what_it_left_out() {
    // Only names go in now, so the block only overflows when there are genuinely
    // many skills: each line is about twenty characters, and the budget is 4 KB.
    let skills: Vec<String> = (0..400).map(|n| format!("skill-number-{n:03}")).collect();

    let block = skills_block(&skills);
    assert!(
        block.chars().count() <= MAX_SKILLS_BLOCK_CHARS,
        "the block is bounded: {} chars",
        block.chars().count()
    );
    assert!(
        block.contains("more not listed"),
        "the skills that did not fit are counted, not dropped in silence: {block}"
    );
    assert!(block.contains("`skill-number-000`"), "{block}");
    // The name, and nothing that has to be read for every request.
    let line = block
        .lines()
        .find(|line| line.starts_with("- `skill-number-000`"))
        .expect("the first skill is listed");
    assert_eq!(
        line.trim(),
        "- `skill-number-000`",
        "the name alone: {line}"
    );
}

#[test]
fn the_names_go_in_and_the_descriptions_do_not() {
    // A description is long — the AWS set alone is nineteen thousand characters —
    // and `skill(action: list)` hands the same text back when it is asked for.
    // The prompt carries what a model needs to know a skill exists.
    let skills = vec!["tiny".to_owned()];
    let block = skills_block(&skills);
    assert!(block.contains("- `tiny`"), "{block}");
    assert!(
        !block.contains("Does one thing."),
        "no description is paid for per request: {block}"
    );
    assert!(
        !block.contains("more not listed"),
        "nothing was left out: {block}"
    );
}

#[test]
fn the_skills_block_names_every_installed_skill() {
    // A model cannot activate a skill it has no way of knowing about, so the
    // names go into the prompt. Neither the description nor the body does: one
    // is a listing request and the other is what `skill(activate)` returns.
    let skills = vec!["pdf-extract".to_owned(), "commit-style".to_owned()];
    let prompt = compose_system_prompt("base", None, &skills);
    assert!(prompt.contains("# Skills"), "{prompt}");
    assert!(prompt.contains("`pdf-extract`"), "{prompt}");
    assert!(prompt.contains("`commit-style`"), "{prompt}");
    assert!(prompt.contains("activate"), "the prompt says how: {prompt}");
    assert!(
        prompt.contains("action: list"),
        "and how to see what each is for: {prompt}"
    );
    assert!(prompt.starts_with("base"), "the base prompt comes first");

    // No skills, no block: a session that has none pays nothing for it.
    assert_eq!(compose_system_prompt("base", None, &[]), "base");
    // And with no base prompt either, the block is still well formed.
    let only_skills = compose_system_prompt("", None, &skills);
    assert!(only_skills.starts_with("# Skills"), "{only_skills}");
    assert!(!only_skills.starts_with('\n'), "{only_skills}");
    assert_eq!(skills_block(&[]), "");
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
    let prompt = compose_system_prompt("base", Some(&loaded), &[]);
    assert!(prompt.starts_with("base\n\n# Project instructions"));
    assert!(prompt.contains("Use tabs."));
    assert_eq!(compose_system_prompt("base", None, &[]), "base");
    assert!(!compose_system_prompt("", Some(&loaded), &[]).starts_with('\n'));

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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
    assert!(
        requests[1].0.is_empty(),
        "the second request offers no tools"
    );
}

#[tokio::test]
async fn two_tools_in_one_answer_produce_one_request_summary() {
    // The line the client prints under an answer is a request summary, not a
    // tool summary: this pins the count to the requests the model was actually
    // asked, which the engine reports once per request. Two tool calls in one
    // answer are one request followed by one more that carries their results --
    // so two lines, and never one per call.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let first = CallId::new();
    let second = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            // One answer that asks for two reads at once, which is what makes
            // this the case worth pinning: the engine reports the request once.
            .turn(vec![
                norte::StreamEvent::ToolCallStart {
                    index: 0,
                    call_id: first,
                    name: "read".to_owned(),
                },
                norte::StreamEvent::ToolCallDelta {
                    index: 0,
                    call_id: first,
                    name: None,
                    arguments: serde_json::json!({ "path": "uno.txt" }).to_string(),
                },
                norte::StreamEvent::ToolCallStart {
                    index: 1,
                    call_id: second,
                    name: "read".to_owned(),
                },
                norte::StreamEvent::ToolCallDelta {
                    index: 1,
                    call_id: second,
                    name: None,
                    arguments: serde_json::json!({ "path": "dos.txt" }).to_string(),
                },
                norte::StreamEvent::Finish(FinishReason::ToolCalls),
            ])
            .turn_text("done"),
    );
    let (service, _store) = persistent_service(model).await;
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");

    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "lee los dos archivos".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;

    let requests: Vec<u64> = events
        .iter()
        .filter_map(|event| match &event.kind {
            ServerEventKind::RequestReconstructed { index, tools, .. } => {
                Some((*index, tools.len()))
            }
            _ => None,
        })
        .map(|(index, _)| index)
        .collect();
    let calls = events
        .iter()
        .filter(|event| matches!(&event.kind, ServerEventKind::ToolStarted { .. }))
        .count();
    assert_eq!(calls, 2, "the model asked for two reads and both ran");
    assert_eq!(
        requests.len(),
        2,
        "one summary per request, not per call: {requests:?}"
    );
    // And they are numbered, so a client can tell them apart and count them.
    assert_eq!(requests, vec![1, 2]);
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    let key = first[0].session.clone().expect("session");
    let second = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "two".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
    assert!(
        loaded.config.approvals.deny.is_empty(),
        "an allow rule is not a deny"
    );
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
        assert!(record.allowed_tools.is_empty(), "only the deny is recorded");
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
        assert!(record.denied_tools.is_empty(), "only the allow is recorded");
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
async fn forgetting_a_global_rule_clears_memory_and_the_config() {
    // "Ask again" is what the third press of the tools panel does, and it has to
    // reach the config file for the same reason a global rule does: a rule
    // forgotten in memory and kept on disk comes back at the next start.
    let path = std::env::temp_dir().join(format!(
        "norte-code-forget-rule-{}-{}.json",
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

    service
        .forget_approval_rule(None, "write", true)
        .await
        .expect("forget");
    assert!(
        !service.approval_rules.denies(session, "write"),
        "the memory rule is gone"
    );
    let loaded = Config::load(Some(&path), None).expect("config");
    assert!(
        loaded.config.approvals.deny.is_empty(),
        "and the file does not carry it back"
    );
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn forgetting_a_session_rule_clears_memory_and_the_record() {
    let service = service();
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "hello".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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

    service
        .forget_approval_rule(Some(&key), "command", false)
        .await
        .expect("forget");
    assert!(
        !service.approval_rules.denies(nid, "command"),
        "the live rule is gone"
    );
    {
        let records = service.lock_sessions();
        let record = records.get(&key).expect("session");
        assert!(record.denied_tools.is_empty(), "and the record too");
        assert!(record.allowed_tools.is_empty(), "and there is no allow");
    }

    assert!(
        service
            .forget_approval_rule(None, "command", false)
            .await
            .is_err(),
        "a session rule needs an open session"
    );
}

#[tokio::test]
async fn a_message_with_an_attachment_carries_the_image_to_the_model() {
    // The end of the paste path. A client pastes a file, the draft carries its
    // path, and the message the model is asked about has to hold the image as a
    // part of the same message — not a second one, and not a path the model
    // would have to go and open.
    let dir = std::env::temp_dir().join(format!("norte-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let photo = dir.join("photo.png");
    // A one-pixel PNG: small, real, and enough for the part to be built.
    std::fs::write(&photo, ONE_PIXEL_PNG).expect("png");

    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(call, "read", &serde_json::json!({ "path": "x" }))
            .turn_text("ok"),
    );
    let (service, _store) = persistent_service(model).await;
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");

    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "what is in this picture?".to_owned(),
            attachments: vec![norte_code_protocol::Attachment {
                path: photo.display().to_string(),
                mime: "image/png".to_owned(),
                name: "photo.png".to_owned(),
            }],
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;

    // The turn ran: the proof is that it did not stop with an error about the
    // attachment. A part the service could not read would come back as one.
    assert!(
        !events.iter().any(|event| matches!(
            &event.kind,
            ServerEventKind::Error { message, .. } if message.contains("attachment")
        )),
        "the attachment was accepted: {:?}",
        events.iter().map(|event| &event.kind).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_attachment_the_model_cannot_take_never_enters_the_session() {
    // The bug this pins down: the engine commits the user's message and only
    // then checks it against the model, so a refused image used to land in the
    // history first. Every later turn rebuilt that history and met the same
    // part, which left the session unusable — and a model switch did not help,
    // because the part is in the history, not in the request. The check now runs
    // before anything is committed, so the session is untouched and the next
    // message goes through.
    let dir = std::env::temp_dir().join(format!("norte-refuse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let photo = dir.join("photo.png");
    std::fs::write(&photo, ONE_PIXEL_PNG).expect("png");

    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    // Text in, text out: the model the catalogue lists as such, which is every
    // model until the host says otherwise.
    let model = Arc::new(ScriptedModel::new(reference).turn_text("fine"));
    let (service, _store) = persistent_service(model).await;
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created
        .first()
        .and_then(|event| event.session.clone())
        .expect("session");

    let refused = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "what is in this picture?".to_owned(),
            attachments: vec![norte_code_protocol::Attachment {
                path: photo.display().to_string(),
                mime: "image/png".to_owned(),
                name: "photo.png".to_owned(),
            }],
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;

    // The refusal names the modality and reaches the client as a capability
    // error, not as a provider failure: the wire was never asked.
    let error = refused
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::Error { code, message } => Some((*code, message.clone())),
            _ => None,
        })
        .expect("an error event");
    assert_eq!(error.0, ErrorCode::Capability, "{}", error.1);
    assert!(error.1.contains("Image"), "{}", error.1);
    assert!(error.1.contains("model_overrides"), "{}", error.1);
    assert!(
        refused.iter().any(|event| matches!(
            &event.kind,
            ServerEventKind::TurnFinished {
                committed: false,
                ..
            }
        )),
        "the turn did not commit"
    );

    // The point of the whole thing: the session still works. A plain message
    // after the refused attachment answers normally, which it could not do if
    // the image had been committed — the history would refuse it every turn.
    let after = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "never mind".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    assert!(
        !after
            .iter()
            .any(|event| matches!(&event.kind, ServerEventKind::Error { .. })),
        "the next turn answers: {:?}",
        after.iter().map(|event| &event.kind).collect::<Vec<_>>()
    );
    assert!(
        after.iter().any(|event| matches!(
            &event.kind,
            ServerEventKind::TurnFinished {
                committed: true,
                ..
            }
        )),
        "and it commits"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A one-pixel PNG, as bytes.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

#[tokio::test]
async fn the_shell_tools_and_the_job_manager_are_registered() {
    // The two that have to agree with each other: one runs a call, waited for or
    // let go of, and one is what reads or stops what the first left behind. A
    // model that can start a job and cannot read it is worse off than one that
    // could not start it.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
    let (service, _store) = persistent_service(model).await;
    let names: Vec<String> = service
        .tools()
        .expect("tools")
        .specs()
        .iter()
        .map(|spec| spec.name.to_string())
        .collect();
    for expected in ["shell", "manage_jobs"] {
        assert!(
            names.iter().any(|name| name == expected),
            "`{expected}` reaches the model; the registry has {names:?}"
        );
    }
    // And there is no second shell: whether a call is waited for is a field on
    // the request, so a second name would be a second place the same answer has
    // to be kept in step.
    assert!(
        !names.iter().any(|name| name == "shell_bg"),
        "the tool list has one shell: {names:?}"
    );
    // That field has to be in the schema the model reads: a request carrying it
    // is rejected when the schema does not name it.
    let shell = service
        .tools()
        .expect("tools")
        .specs()
        .into_iter()
        .find(|spec| spec.name.as_str() == "shell")
        .expect("shell");
    let schema = shell.parameters.to_string();
    assert!(
        schema.contains("detach"),
        "the model can ask to be detached: {schema}"
    );
}

#[tokio::test]
async fn a_zero_bound_is_granted_only_to_a_call_nobody_waits_for() {
    // One shell means one set of limits, so the host grants the zero and says
    // which shape gets it. The shape is the whole rule: a call nobody waits for
    // may run unbounded, and the same zero on a call that holds the turn is
    // dropped before it reaches the executor. What the executor then does with
    // the field is tested in `norte-tools`, where waiting out a bound is cheap.
    let keeper = ShellCalls::declaration(
        ShellTool::new()
            .with_limits(ShellLimits {
                default_timeout: std::time::Duration::from_millis(150),
                grace: std::time::Duration::from_millis(50),
                allow_indefinite: true,
                ..ShellLimits::default()
            })
            .into_declaration(),
        Arc::new(JobRegistry::default()),
        Arc::new(Steering::default()),
        Arc::new(DetachRegistry::default()),
    );
    let context = ToolContext::new()
        .with_session(SessionId::new())
        .with_call(CallId::new());

    let held = keeper
        .executor
        .call_with_context(
            &context,
            serde_json::from_value(serde_json::json!({ "script": "sleep 30", "timeout_ms": 0 }))
                .expect("request"),
        )
        .await
        .expect("execute");
    let structured = held.structured.expect("structured");
    assert_eq!(
        structured["code"], "timeout",
        "the zero was refused and the host's default applied: {structured:?}"
    );

    let let_go = keeper
        .executor
        .call_with_context(
            &context,
            serde_json::from_value(
                serde_json::json!({ "script": "sleep 30", "timeout_ms": 0, "detach": true }),
            )
            .expect("request"),
        )
        .await
        .expect("execute");
    // The answer to a call let go of from the start is the job's name, not a
    // shell result: the shell result is what arrives when the process ends, and
    // the model is told where to find it.
    let text: String = let_go
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect();
    assert!(
        text.contains("job-1"),
        "a call nobody waits for answers with the job it became: {text}"
    );
    assert!(
        !let_go.is_error,
        "being let go of is not a failure: it is what was asked for"
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
        if let ServerEventKind::SteerApplied { ids, .. } = &event.kind {
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
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
// One scenario end to end: park, crash, reopen, refuse. The length is the
// scenario, and the `replay` field every `SendMessage` carries is what tipped it
// past the limit.
#[allow(clippy::too_many_lines)]
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
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

/// The replay events of a stream, as `(role, text)` pairs.
fn replays(events: &[ServerEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            ServerEventKind::MessageReplayed { role, text, .. } => {
                Some((role.clone(), text.clone()))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_message_to_an_existing_session_replays_it_when_asked() {
    // `run --session KEY` sends a message to a session that already has a
    // history, and the client starts blank. The replay used to hang on the
    // request having no message, so this case got no transcript at all.
    let (service, _model) = scripted_service(spec("scripted"), &["hello there", "again"]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");

    drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "first".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;

    // The same session, a new message, and the client asking for the history.
    let attached = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "second".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: true,
        },
    )
    .await;
    let replay = replays(&attached);
    assert!(
        replay
            .iter()
            .any(|(role, text)| role == "user" && text == "first"),
        "the history comes back: {replay:?}"
    );

    // Without the flag nothing is replayed: the transcript is already on screen.
    let plain = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "third".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    assert!(
        replays(&plain).is_empty(),
        "a plain message does not repeat history: {:?}",
        replays(&plain)
    );
}

#[tokio::test]
async fn a_resume_reports_the_usage_the_session_already_spent() {
    // A resuming client starts blank, so the session's accumulated counters
    // have to arrive with the replay; otherwise the footer shows zero usage for
    // a session that has spent tokens.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn(vec![
        norte::StreamEvent::Usage(cached_usage(120, 30, 90, Some(0.02))),
        norte::StreamEvent::TextDelta {
            index: 0,
            text: "first answer".to_owned(),
        },
        norte::StreamEvent::Finish(FinishReason::Stop),
    ]));
    let (service, _store) = persistent_service_with_config(config(), model).await;

    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "first question".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");

    let resumed = drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
        },
    )
    .await;
    let stats = resumed
        .iter()
        .find_map(|event| match &event.kind {
            ServerEventKind::SessionStats {
                input_tokens,
                output_tokens,
                requests,
                cache_read_tokens,
                ..
            } => Some((*input_tokens, *output_tokens, *requests, *cache_read_tokens)),
            _ => None,
        })
        .expect("the resume reports the session usage");
    assert_eq!(
        stats,
        (120, 30, 1, 90),
        "the spent usage comes back, cache included: {stats:?}"
    );
}

#[tokio::test]
async fn a_resume_after_a_compaction_still_replays_the_earlier_turns() {
    // A compaction replaces the live session with a summary and the last few
    // messages. The replay used to read only that session, so a resume showed
    // `keep_recent` messages instead of the conversation the user just had.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn(vec![
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "first answer".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ])
            .turn_text("the summary of the earlier conversation")
            .turn(vec![
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "second answer".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ]),
    );
    let mut config = config();
    config.compaction.threshold = 0.1;
    config.compaction.keep_recent = 1;
    let (service, _store) = persistent_service_with_config(config, model).await;

    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "first question".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    let compacted = first.iter().any(|event| {
        matches!(
            &event.kind,
            ServerEventKind::CompactionFinished { compacted, .. } if *compacted
        )
    });
    assert!(compacted, "the first turn compacted");

    // The message written before the compaction is outside the live session, so
    // only the ancestor chain can supply it.
    let resumed = drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
        },
    )
    .await;
    let replay = replays(&resumed);
    assert!(
        replay.contains(&("user".to_owned(), "first question".to_owned())),
        "the pre-compaction question comes back: {replay:?}"
    );
    assert!(
        replay.contains(&("assistant".to_owned(), "first answer".to_owned())),
        "the pre-compaction answer comes back: {replay:?}"
    );
}

#[tokio::test]
async fn a_resume_replays_the_recent_messages() {
    let (service, _model) = scripted_service(spec("scripted"), &["hello there"]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");

    drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "hi".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;

    let resumed = drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
        },
    )
    .await;
    let replay = replays(&resumed);
    assert!(
        replay.contains(&("user".to_owned(), "hi".to_owned())),
        "the user message comes back: {replay:?}"
    );
    // The turn is announced before the transcript it replays: a client that
    // rendered the history first showed the session as idle while the turn was
    // already running.
    let started = resumed
        .iter()
        .position(|event| matches!(event.kind, ServerEventKind::TurnStarted { .. }))
        .expect("the resume announces its turn");
    let first_replay = resumed
        .iter()
        .position(|event| matches!(event.kind, ServerEventKind::MessageReplayed { .. }))
        .expect("the resume replays the history");
    assert!(
        started < first_replay,
        "TurnStarted must come before the replay, got {started} > {first_replay}"
    );
    assert!(
        replay.contains(&("assistant".to_owned(), "hello there".to_owned())),
        "the answer comes back: {replay:?}"
    );

    // A normal message must not repeat history: only the resume shows it.
    let sent = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "again".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    assert!(
        replays(&sent).is_empty(),
        "a message does not replay the conversation: {:?}",
        replays(&sent)
    );
}

/// A registry with one job, for the state transitions.
fn registry_with_job() -> (Arc<JobRegistry>, SessionId, CallId, String) {
    let registry = Arc::new(JobRegistry::default());
    let session = SessionId::new();
    let call = CallId::new();
    let cancel = Arc::new(norte::Cancellation::new());
    let id = registry.start(session, call, "sleep 60", Arc::clone(&cancel));
    (registry, session, call, id)
}

#[test]
fn a_job_ends_when_its_streams_close_not_when_its_call_finishes() {
    // The whole reason the registry keys on the call: a `shell_bg` result
    // arrives long before the process does, so the result cannot be the end.
    let (registry, _, call, id) = registry_with_job();
    assert_eq!(registry.get(&id).expect("job").state, JobState::Running);

    // One stream closing is not the end when the other is still open.
    registry.close(call, false);
    assert_eq!(registry.get(&id).expect("job").state, JobState::Running);
    registry.close(call, true);
    assert_eq!(registry.get(&id).expect("job").state, JobState::Finished);

    // The exit code lands after the streams have closed, on a finished job.
    registry.settle(&id, Some(7), false);
    let job = registry.get(&id).expect("job");
    assert_eq!(job.exit_code, Some(7));
    assert_eq!(job.state, JobState::Finished);
}

#[test]
fn a_cancelled_job_keeps_its_state_when_the_call_reports_an_exit() {
    // Cancelling signals the token, which makes the process die non-zero. That
    // code is the consequence of the cancellation, not a new fact about the job.
    let (registry, _, _, id) = registry_with_job();
    assert!(registry.cancel(&id), "a running job is cancellable");
    assert_eq!(registry.get(&id).expect("job").state, JobState::Cancelled);

    registry.settle(&id, Some(143), false);
    let job = registry.get(&id).expect("job");
    assert_eq!(job.state, JobState::Cancelled);
    assert_eq!(job.exit_code, None, "the signal exit is not the job's code");
    assert!(
        !registry.cancel(&id),
        "a cancelled job is not cancellable again"
    );
}

#[test]
fn stopping_a_session_reaches_only_its_own_jobs() {
    let registry = Arc::new(JobRegistry::default());
    let mine = SessionId::new();
    let theirs = SessionId::new();
    let cancel = Arc::new(norte::Cancellation::new());
    let ours = registry.start(mine, CallId::new(), "one", Arc::clone(&cancel));
    registry.start(theirs, CallId::new(), "two", Arc::clone(&cancel));

    assert!(registry.any_running(mine));
    assert_eq!(registry.cancel_session(mine), 1, "exactly one job was ours");
    assert_eq!(registry.get(&ours).expect("job").state, JobState::Cancelled);
    assert!(
        registry.any_running(theirs),
        "the other session's job is untouched"
    );
    assert!(!registry.any_running(mine), "and ours is done");
}

#[test]
fn the_job_buffer_keeps_the_tail_and_says_it_dropped_the_rest() {
    let (registry, _, call, id) = registry_with_job();
    // The cap is generous, so filling it needs a lot: this writes more than it
    // holds and checks the *end* survived, which is what a reader wants.
    let chunk = "x".repeat(JOB_BUFFER_BYTES);
    registry.write(call, &chunk);
    registry.write(call, "THE END");
    let job = registry.get(&id).expect("job");
    assert!(job.dropped, "older output was dropped");
    assert!(job.output.ends_with("THE END"), "the tail survives");
    assert!(
        job.output.len() <= JOB_BUFFER_BYTES + "THE END".len(),
        "the buffer stays bounded: {} bytes",
        job.output.len()
    );
}

#[tokio::test]
async fn a_detached_call_answers_at_once_and_reports_when_the_process_ends() {
    // The real tool, not a stand-in: what is under test is that the job's own
    // sink collects the output and that its end is the stream closing, neither
    // of which a fake executor would exercise.
    let dir = std::env::temp_dir().join(format!("norte-bg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let marker = dir.join("done");
    let jobs = Arc::new(JobRegistry::default());
    let steering = Arc::new(Steering::default());
    let declaration = ShellTool::new().into_declaration();
    let background = ShellCalls::declaration(
        declaration,
        Arc::clone(&jobs),
        Arc::clone(&steering),
        Arc::new(DetachRegistry::default()),
    );

    // The process writes, waits, then writes again and touches its marker: the
    // call has to answer long before any of that is done.
    let script = format!(
        "echo first; sleep 1; echo second; touch {}",
        marker.display()
    );
    let request: ShellRequest = serde_json::from_value(serde_json::json!({
        "script": script,
        "detach": true
    }))
    .expect("request");
    let session = SessionId::new();
    let call = CallId::new();
    let context = ToolContext::new().with_session(session).with_call(call);

    let started = tokio::time::Instant::now();
    let answer = background
        .executor
        .call_with_context(&context, request.clone())
        .await
        .expect("the call answers");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(900),
        "the call must not wait for the process: {:?}",
        started.elapsed()
    );
    let text = answer
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect::<String>();
    assert!(
        text.contains("job-1"),
        "the model is told the job id: {text}"
    );

    let job = jobs.get("job-1").expect("the job is registered");
    assert_eq!(job.state, JobState::Running, "it is still running");
    assert_eq!(
        job.script,
        format!(
            "echo first; sleep 1; echo second; touch {}",
            marker.display()
        )
    );

    // Let the process finish of its own accord. The job ends because its
    // streams closed, and its output is what its own sink collected.
    for _ in 0..50 {
        if jobs.get("job-1").is_some_and(|job| job.state.is_done()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let job = jobs.get("job-1").expect("the job is still registered");
    assert_eq!(
        job.state,
        JobState::Finished,
        "the closing chunk ends the job, not its call: {job:?}"
    );
    assert!(
        job.output.contains("first"),
        "the sink collected: {:?}",
        job.output
    );
    assert!(
        job.output.contains("second"),
        "and the tail too: {:?}",
        job.output
    );
    assert!(marker.exists(), "the process ran to completion");

    // The result reached the model as a steering message, because the call it
    // would have answered was answered at the start.
    let steered = steering.take(session);
    assert_eq!(steered.len(), 1, "one report per job: {steered:?}");
    assert_eq!(steered[0].id, "job:job-1");
    assert!(
        steered[0].text.contains("finished"),
        "the report says it ended: {}",
        steered[0].text
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn stopping_a_job_through_the_registry_ends_its_process() {
    // Cancel is what `/jobs stop` and Ctrl-C reach: the job's own token, which
    // kills the process group without touching the turn.
    let dir = std::env::temp_dir().join(format!("norte-bg-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let marker = dir.join("survived");
    let jobs = Arc::new(JobRegistry::default());
    let steering = Arc::new(Steering::default());
    let background = ShellCalls::declaration(
        ShellTool::new().into_declaration(),
        Arc::clone(&jobs),
        steering,
        Arc::new(DetachRegistry::default()),
    );
    let script = format!("sleep 3; touch {}", marker.display());
    let request: ShellRequest = serde_json::from_value(serde_json::json!({
        "script": script,
        "detach": true
    }))
    .expect("request");
    let context = ToolContext::new()
        .with_session(SessionId::new())
        .with_call(CallId::new());
    let _ = background
        .executor
        .call_with_context(&context, request)
        .await
        .expect("the call answers");

    assert!(jobs.cancel("job-1"), "a running job is stoppable");
    // The process was killed, so it never wrote its marker.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!marker.exists(), "the stopped process did not finish");
    assert_eq!(jobs.get("job-1").expect("job").state, JobState::Cancelled);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_call_detached_from_outside_leaves_a_job_the_model_can_read() {
    // The `Ctrl-G` answer. The process is made to say nothing after the first
    // line, so no chunk arrives once the call is let go of: a job registered on
    // the way past a chunk would never exist, and the model would be handed a
    // call that went with nothing to read from. What registers it is the
    // result's own `detached`, whoever asked for it.
    let dir = std::env::temp_dir().join(format!("norte-detach-user-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let first = dir.join("first");
    let later = dir.join("later");
    let jobs = Arc::new(JobRegistry::default());
    let steering = Arc::new(Steering::default());
    let detaches = Arc::new(DetachRegistry::default());
    let declaration = ShellCalls::declaration(
        ShellTool::new().into_declaration(),
        Arc::clone(&jobs),
        Arc::clone(&steering),
        Arc::clone(&detaches),
    );

    let session = SessionId::new();
    let watched = first.clone();
    let registry = Arc::clone(&detaches);
    tokio::spawn(async move {
        for _ in 0..200 {
            if watched.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(registry.detach(session), "there is a call to detach");
    });

    // `echo` puts one line on the pipe and then the process says nothing more
    // until it touches its marker: no chunk follows the detach.
    let script = format!(
        "echo before; touch {}; sleep 2; touch {}",
        first.display(),
        later.display()
    );
    let request: ShellRequest =
        serde_json::from_value(serde_json::json!({ "script": script })).expect("request");
    let context = ToolContext::new()
        .with_session(session)
        .with_call(CallId::new());
    let answer = declaration
        .executor
        .call_with_context(&context, request)
        .await
        .expect("the call answers");

    // The model is told which job owns the rest, because the text it has is only
    // what arrived before the call went.
    let text: String = answer
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect();
    assert!(
        text.contains("job-1"),
        "the answer names the job the process became: {text}"
    );
    let job = jobs.get("job-1").expect("a job was registered");
    assert_eq!(job.state, JobState::Running, "the process is still running");

    // And it can be read and stopped, which is what a job is for.
    assert!(jobs.cancel("job-1"), "the job can be stopped");
    assert_eq!(jobs.get("job-1").expect("job").state, JobState::Cancelled);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(
        !later.exists(),
        "the stopped process never reached its marker"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_process_that_ends_after_the_detach_reports_once() {
    // The two ends can arrive in either order: the process may end before the
    // call has returned, and then the report would be sent with no job to send
    // it from — or sent twice, once from each end.
    let dir = std::env::temp_dir().join(format!("norte-detach-report-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let jobs = Arc::new(JobRegistry::default());
    let steering = Arc::new(Steering::default());
    let detaches = Arc::new(DetachRegistry::default());
    let declaration = ShellCalls::declaration(
        ShellTool::new().into_declaration(),
        Arc::clone(&jobs),
        Arc::clone(&steering),
        Arc::clone(&detaches),
    );

    let session = SessionId::new();
    let context = ToolContext::new()
        .with_session(session)
        .with_call(CallId::new());
    // It writes, then ends on its own: the closing chunk arrives after the call
    // is already gone.
    let request: ShellRequest =
        serde_json::from_value(serde_json::json!({ "script": "echo done", "detach": true }))
            .expect("request");
    let _ = declaration
        .executor
        .call_with_context(&context, request)
        .await
        .expect("the call answers");

    for _ in 0..50 {
        if steering.is_pending(session, "job:job-1") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        jobs.get("job-1").is_some_and(|job| job.state.is_done()),
        "the job ended when its streams closed"
    );
    let steered = steering.take(session);
    assert_eq!(steered.len(), 1, "one report per job: {steered:?}");
    assert!(
        steered[0].text.contains("done"),
        "the report carries what the process wrote: {}",
        steered[0].text
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_process_that_ends_before_the_detached_call_returns_reports_once() {
    // The two ends meet here. The process ends right after the detach, so the
    // closing chunk registers the job and reports from there; the call then
    // returns, and its own end tries to report the same job a second time. One
    // report is what the model needs, and two would be two messages about one
    // process.
    let dir = std::env::temp_dir().join(format!("norte-detach-once-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let first = dir.join("first");
    let jobs = Arc::new(JobRegistry::default());
    let steering = Arc::new(Steering::default());
    let detaches = Arc::new(DetachRegistry::default());
    let declaration = ShellCalls::declaration(
        ShellTool::new().into_declaration(),
        Arc::clone(&jobs),
        Arc::clone(&steering),
        Arc::clone(&detaches),
    );

    let session = SessionId::new();
    let watched = first.clone();
    let registry = Arc::clone(&detaches);
    let presser = tokio::spawn(async move {
        for _ in 0..200 {
            if watched.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        registry.detach(session)
    });

    // It writes, is detached, and ends at once: the whole life of the process
    // sits inside the call's own wait.
    let script = format!("echo before; touch {}; sleep 0.2", first.display());
    let request: ShellRequest =
        serde_json::from_value(serde_json::json!({ "script": script })).expect("request");
    let context = ToolContext::new()
        .with_session(session)
        .with_call(CallId::new());
    let answer = declaration
        .executor
        .call_with_context(&context, request)
        .await
        .expect("the call answers");
    assert!(presser.await.expect("the presser"), "it was detached");

    let text: String = answer
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect();
    assert!(text.contains("job-1"), "the answer names the job: {text}");

    // Let the closing chunk land, then look at what was reported.
    for _ in 0..50 {
        if jobs.get("job-1").is_some_and(|job| job.state.is_done()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        jobs.get("job-1").is_some_and(|job| job.state.is_done()),
        "the job ended with its process"
    );
    let steered = steering.take(session);
    assert_eq!(
        steered.len(),
        1,
        "one report for one process, whichever end saw it end: {steered:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn detaching_stops_waiting_and_leaves_the_process_running() {
    // The whole point of the third answer: unlike Ctrl-C, detaching does not
    // kill anything. The call returns with what arrived, the process outlives
    // it and writes what it had left.
    let dir = std::env::temp_dir().join(format!("norte-detach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let first = dir.join("first");
    let later = dir.join("later");
    let detaches = Arc::new(DetachRegistry::default());
    let declaration = ShellCalls::declaration(
        ShellTool::new().into_declaration(),
        Arc::new(JobRegistry::default()),
        Arc::new(Steering::default()),
        Arc::clone(&detaches),
    );

    // Detaching only once the first line has reached the pipe, so the assertion
    // on the captured output is not a race with startup.
    let session = SessionId::new();
    let watched = first.clone();
    let registry = Arc::clone(&detaches);
    tokio::spawn(async move {
        for _ in 0..100 {
            if watched.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(registry.detach(session), "there is a call to detach");
    });

    let script = format!(
        "echo before; touch {}; sleep 1; touch {}",
        first.display(),
        later.display()
    );
    let request: ShellRequest =
        serde_json::from_value(serde_json::json!({ "script": script })).expect("request");
    let context = ToolContext::new()
        .with_session(session)
        .with_call(CallId::new());

    let started = tokio::time::Instant::now();
    let output = declaration
        .executor
        .call_with_context(&context, request)
        .await
        .expect("the call answers");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(900),
        "the call stopped waiting instead of running to the end: {:?}",
        started.elapsed()
    );
    let structured = output.structured.expect("a shell result is structured");
    assert_eq!(structured["code"], "detached");
    assert_eq!(structured["detached"], true);

    // The process outlived the call: it writes its second marker a second
    // after the call returned, which is exactly what cancelling would prevent.
    for _ in 0..50 {
        if later.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(later.exists(), "the process kept running after the detach");

    // A later detach finds nothing: the token was released with the call.
    assert!(!detaches.detach(session), "no call is waiting now");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_finished_job_keeps_the_time_its_end_recorded() {
    // It reported the time since it *started*, so a job that took a second read
    // as two, then three: the figure grew for as long as anyone looked at it.
    let (registry, _, call, id) = registry_with_job();
    std::thread::sleep(std::time::Duration::from_millis(20));
    registry.close(call, true);
    let at_end = registry.get(&id).expect("job").elapsed_ms;

    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(
        registry.get(&id).expect("job").elapsed_ms,
        at_end,
        "a job that has ended does not keep running"
    );
    assert_eq!(
        registry.list(None)[0].elapsed_ms,
        at_end,
        "nor in a listing"
    );
}

#[test]
fn a_running_job_reports_the_time_it_has_run() {
    let (registry, _, _, id) = registry_with_job();
    let first = registry.get(&id).expect("job").elapsed_ms;
    std::thread::sleep(std::time::Duration::from_millis(30));
    let later = registry.get(&id).expect("job").elapsed_ms;
    assert!(
        later > first,
        "a running job's time grows: {first} then {later}"
    );
}

#[tokio::test]
async fn a_compaction_does_not_reset_the_session_cache_share() {
    // Two paths wrote the session's counters: `settle` after a turn, and the
    // block that reloads them when a compaction replaces the session behind the
    // record. The second copied every counter but the two cache ones, so the
    // first compaction zeroed a session's cache share and no later turn could
    // bring it back — the record was the thing that was wrong.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    // Two turns that both hit the cache, then the summary request itself.
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn(vec![
                norte::StreamEvent::Usage(cached_usage(100, 10, 90, Some(0.01))),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "first".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ])
            .turn(vec![
                norte::StreamEvent::Usage(cached_usage(80, 10, 70, Some(0.01))),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "second".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ])
            .turn_text("the handoff summary"),
    );
    let (service, _store) = persistent_service(model).await;

    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "work".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "more".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;

    let before = drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
        },
    )
    .await;
    let cached = |events: &[ServerEvent]| {
        events.iter().find_map(|event| match &event.kind {
            ServerEventKind::SessionStats {
                cache_read_tokens, ..
            } => Some(*cache_read_tokens),
            _ => None,
        })
    };
    let before = cached(&before).expect("stats before");
    assert!(before > 0, "the turns hit the cache: {before}");

    drain(
        &service,
        ClientRequestKind::Compact {
            session: key.clone(),
        },
    )
    .await;

    let after = drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
        },
    )
    .await;
    let after = cached(&after).expect("stats after");
    assert!(
        after >= before,
        "a compaction summarises, it does not un-cache: {after} < {before}"
    );
}

#[tokio::test]
async fn a_title_is_asked_for_in_a_session_of_its_own() {
    // The title comes from a request that is not the conversation: a scratch
    // session holding one instruction, thrown away after. This is the whole
    // point of the feature — a session gets a name without the naming being
    // something the model can see or answer later.
    let (service, model) = scripted_service(spec("scripted"), &["fix the footer overflow"]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");

    let title = service
        .title_session(&key, "the footer overflows when the terminal is narrow")
        .await
        .expect("a title");
    assert_eq!(title, "fix the footer overflow");

    // The session is named, and the list shows it beside its key.
    let listed = drain(&service, ClientRequestKind::ListSessions).await;
    let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
        panic!("expected a list");
    };
    assert_eq!(
        sessions[0].title.as_deref(),
        Some("fix the footer overflow")
    );

    // The request went to a session of its own: the one being named is as empty
    // as it was, and none of the naming is in it.
    let history = service
        .engine(&spec("scripted"))
        .expect("engine")
        .history(service.require_record(&key).expect("record").0)
        .await
        .expect("history");
    assert!(!history.iter().any(|message| {
        let text = message.text();
        text.contains(TITLE_PROMPT) || text.contains("fix the footer overflow")
    }));

    // And what the model was asked is the instruction plus the person's words,
    // in one user message.
    let requests = model.requests();
    assert_eq!(requests.len(), 1, "one request, one title");
    let asked = &requests[0];
    assert_eq!(asked.messages.len(), 1, "the scratch holds one message");
    let text = asked.messages[0].text();
    assert!(text.contains(TITLE_PROMPT), "the instruction is there");
    assert!(
        text.contains("the footer overflows"),
        "and the message the title is for: {text}"
    );
    assert!(
        asked.tools.is_empty(),
        "a title needs no tools: {:?}",
        asked.tools
    );
}

#[tokio::test]
async fn a_title_that_cannot_be_had_leaves_the_session_unnamed() {
    // The title is best effort by contract: it is asked for beside the turn the
    // person just started, and a model that answers with nothing is not a reason
    // to disturb it. The session keeps its key, which is what it had.
    let (service, _model) = scripted_service(spec("scripted"), &["..."]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");

    let error = service
        .title_session(&key, "do the thing")
        .await
        .expect_err("a reply that is not a name is refused");
    assert!(
        error.to_string().contains("no usable title"),
        "the reason names what happened: {error}"
    );
    let listed = drain(&service, ClientRequestKind::ListSessions).await;
    let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
        panic!("expected a list");
    };
    assert_eq!(
        sessions[0].title, None,
        "a session nobody could name is not given a name"
    );
}

#[tokio::test]
async fn titling_can_be_turned_off() {
    // The switch matters: this is a request per session, on a provider the
    // person pays for, and a host that does not want it must be able to say so.
    let mut config = config();
    config.titling.enabled = false;
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("a title"));
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
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");
    let error = service
        .title_session(&key, "do the thing")
        .await
        .expect_err("disabled");
    assert!(error.to_string().contains("disabled"), "{error}");
}

#[tokio::test]
async fn a_title_can_be_asked_of_another_model() {
    // `titling.model` names the model that writes titles, and it is free to be
    // a cheaper one than the session's. The resolution is the one a switch uses,
    // so it may also be a provider the catalogue does not list.
    let mut config = config();
    config.titling.model = Some("private/tiny".to_owned());
    config.providers.insert(
        "private".to_owned(),
        norte_code_config::ProviderConfig {
            endpoint: Some("https://example.invalid/v1".to_owned()),
            wire: Some(norte_code_config::WireKind::Chat),
            api_key_file: Some(key_file("private")),
            ..norte_code_config::ProviderConfig::default()
        },
    );
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("a cheap title"));
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
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");
    let title = service
        .title_session(&key, "do the thing")
        .await
        .expect("a title");
    assert_eq!(title, "a cheap title");
}

/// A service whose config is the one the test needs, with a scripted model that
/// is never asked to answer: these tests read the registry and the rules.
fn service_with(config: norte_code_config::Config) -> Arc<Service> {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_deltas(["Hel", "lo"]));
    Arc::new(
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
    )
}

#[tokio::test]
async fn a_tool_list_counts_the_rules_that_decide_each_tool() {
    // The list is what the footer counts and what `/tools` prints, and both
    // read it from here: deny beats allow beats the mode, which is the order
    // the policy itself applies. A list that got the order wrong would show a
    // tool as allowed while the engine refuses it.
    let mut config = config();
    config.approvals.allow = vec!["read".to_owned()];
    config.approvals.deny = vec!["write".to_owned()];
    let service = service_with(config);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");
    service
        .set_approval_rule(Some(&key), "shell", true, false)
        .await
        .expect("session rule");

    let statuses = service.tool_rules(Some(&key));
    let find = |name: &str| {
        statuses
            .iter()
            .find(|status| status.name == name)
            .unwrap_or_else(|| panic!("`{name}` is registered: {statuses:?}"))
    };
    assert_eq!(find("read").rule, ToolRule::Allow);
    assert_eq!(find("read").scope, RuleScope::Global, "from the config");
    assert_eq!(find("write").rule, ToolRule::Deny);
    assert_eq!(find("write").scope, RuleScope::Global);
    assert_eq!(find("shell").rule, ToolRule::Allow);
    assert_eq!(
        find("shell").scope,
        RuleScope::Session,
        "a rule of this session only"
    );
    // `edit` needs approval under the mode and has no rule of its own, which is
    // what leaves it in the `ask` count with no rule behind it.
    assert_eq!(find("edit").rule, ToolRule::Ask);
    assert_eq!(find("edit").scope, RuleScope::Mode);
}

#[tokio::test]
async fn a_session_rule_reaches_only_the_session_it_was_set_for() {
    // A rule somebody set for the session they are in must not show up as a
    // rule of every session: which one it is decides who else it binds.
    let service = service();
    let first = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let first = first[0].session.clone().expect("session");
    let second = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let second = second[0].session.clone().expect("session");
    service
        .set_approval_rule(Some(&first), "shell", true, false)
        .await
        .expect("session rule");

    let in_first = service.tool_rules(Some(&first));
    let command = in_first
        .iter()
        .find(|status| status.name == "shell")
        .expect("command is registered");
    assert_eq!(command.rule, ToolRule::Allow);
    assert_eq!(command.scope, RuleScope::Session);

    let in_second = service.tool_rules(Some(&second));
    let command = in_second
        .iter()
        .find(|status| status.name == "shell")
        .expect("command is registered");
    assert_eq!(
        command.rule,
        ToolRule::Ask,
        "the other session never had that rule"
    );
    assert_eq!(command.scope, RuleScope::Mode);
}

#[tokio::test]
async fn the_tool_list_is_sorted_so_a_count_and_a_reading_agree() {
    let statuses = service().tool_rules(None);
    assert!(!statuses.is_empty(), "the registry has tools");
    let mut sorted: Vec<String> = statuses.iter().map(|status| status.name.clone()).collect();
    sorted.sort();
    let listed: Vec<String> = statuses.iter().map(|status| status.name.clone()).collect();
    assert_eq!(listed, sorted, "the list reads down the names");
}

#[tokio::test]
async fn a_tool_named_in_both_lists_is_denied() {
    // Deny wins, and the tool list has to say so: naming a tool in both lists
    // is how a copied `allow` list keeps one tool out.
    let mut config = config();
    config.approvals.allow = vec!["write".to_owned()];
    config.approvals.deny = vec!["write".to_owned()];
    let service = service_with(config);
    let statuses = service.tool_rules(None);
    let write = statuses
        .iter()
        .find(|status| status.name == "write")
        .expect("write is registered");
    assert_eq!(write.rule, ToolRule::Deny);
    assert_eq!(write.scope, RuleScope::Global);
}

#[tokio::test]
async fn the_shell_is_the_tool_the_approval_mode_gates() {
    // The tool is registered as `shell`, and the mode has to name it the way
    // the registry does: a mode that lists a tool nobody has is a mode that
    // approves nothing. `Ask` is the default, so this is what the first call a
    // person ever makes meets.
    let statuses = service().tool_rules(None);
    let shell = statuses
        .iter()
        .find(|status| status.name == "shell")
        .expect("the shell is registered");
    assert_eq!(
        shell.rule,
        ToolRule::Ask,
        "`ask` mode asks before a command runs"
    );
}
#[tokio::test]
async fn every_tool_the_mode_gates_is_a_tool_that_exists() {
    // The mode used to name `command`, and no tool has answered to that since
    // the shell became one tool: `ask` — the default — let a command run
    // without asking, and nothing said so, because a name that matches nothing
    // is not an error. The names are read back out of the registry here for the
    // same reason: this is the only place that can notice.
    //
    // `skill_script` is registered only when the config lets skill scripts run,
    // so the config here is the one that turns them on: the names are the point,
    // not the defaults.
    let mut config = config();
    config.skills.allow_scripts = true;
    let statuses = service_with(config).tool_rules(None);
    let registered: Vec<&str> = statuses.iter().map(|status| status.name.as_str()).collect();
    for name in super::approvals::WRITES
        .iter()
        .chain(super::approvals::RUNS.iter())
    {
        assert!(
            registered.contains(name),
            "the mode names `{name}`, which is not a registered tool: {registered:?}"
        );
    }
}

#[tokio::test]
async fn the_mode_asks_before_the_tool_that_runs_a_command() {
    // The mode used to decide by a tool called `command`, and no tool has
    // answered to that since the shell became one: `ask` — which is what a
    // config with no `approval_mode` in it gets — let a command run without
    // asking. A wrong name in that list is silent, because a name that matches
    // nothing is not an error, so this is the test that has to notice.
    //
    // The names are read back out of the registry rather than trusted: that is
    // the whole failure, a list of names nobody has.
    let registered: Vec<String> = service()
        .tools()
        .expect("tools")
        .specs()
        .into_iter()
        .map(|spec| spec.name.to_string())
        .collect();
    for tool in &registered {
        let wanted =
            super::approvals::mode_needs_approval(norte_code_config::ApprovalMode::Ask, tool);
        let is_dangerous = matches!(tool.as_str(), "write" | "edit" | "shell" | "skill_script");
        assert_eq!(
            wanted, is_dangerous,
            "`ask` mode and the tool list disagree about `{tool}`: {registered:?}"
        );
    }
    // And the names the mode keeps are names the registry has, except for the
    // one tool that is only registered when the config turns it on: a name the
    // mode keeps for a tool that is not there is exactly the bug, and
    // `skill_script` is the one case where absence is expected.
    for name in super::approvals::WRITES
        .iter()
        .chain(super::approvals::RUNS.iter())
    {
        if *name == "skill_script" {
            continue;
        }
        assert!(
            registered.iter().any(|tool| tool == name),
            "the mode names `{name}`, which is not a registered tool: {registered:?}"
        );
    }
}

#[tokio::test]
async fn a_session_reasoning_override_survives_a_restart() {
    // `max` is not the configured default, so seeing it after the reopen is the
    // whole test: a session that comes back on `auto` is one where the person
    // has to type `/reasoning max` again every time they start the client,
    // which is what they said happens.
    let dir = std::env::temp_dir().join(format!("norte-reasoning-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("sessions.db");
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let (store, service) = sqlite_service(
        &path,
        Arc::new(ScriptedModel::new(reference.clone()).turn_text("ok")),
    )
    .await;
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");
    drain(
        &service,
        ClientRequestKind::SetReasoning {
            session: key.clone(),
            scope: ReasoningScope::Session,
            reasoning: Reasoning::Effort {
                effort: "max".to_owned(),
            },
        },
    )
    .await;
    assert_eq!(
        service.session_reasoning(&key),
        Some(Reasoning::Effort {
            effort: "max".to_owned(),
        }),
        "set in memory"
    );

    // What a restart does: a new service over the same database.
    drop(service);
    drop(store);
    let (store, reopened) = sqlite_service(
        &path,
        Arc::new(ScriptedModel::new(reference).turn_text("ok")),
    )
    .await;
    assert_eq!(
        reopened.session_reasoning(&key),
        Some(Reasoning::Effort {
            effort: "max".to_owned(),
        }),
        "and it is still there after the reopen"
    );
    drop(reopened);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_resumed_session_sends_the_effort_it_was_left_on() {
    // The override is stored, so the next turn of a resumed session must ask
    // the provider for it without the client passing anything: a person who
    // typed `/reasoning max` and restarted should not have to type it again.
    let dir = std::env::temp_dir().join(format!("norte-effort-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("sessions.db");
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let (store, service) = sqlite_service(
        &path,
        Arc::new(ScriptedModel::new(reference.clone()).turn_text("ok")),
    )
    .await;
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");
    drain(
        &service,
        ClientRequestKind::SetReasoning {
            session: key.clone(),
            scope: ReasoningScope::Session,
            reasoning: Reasoning::Effort {
                effort: "high".to_owned(),
            },
        },
    )
    .await;
    drop(service);
    drop(store);

    // Reopened, and asked for a turn with no reasoning in the request: what the
    // client does when the flag was not passed. `high` rather than `max`
    // because the scripted model declares up to high — the engine checks the
    // effort against what the model says it takes, which is the model's answer
    // and not the catalogue's, and a fixture that declares four efforts is the
    // right thing to check that against.
    let (store, reopened) = sqlite_service(
        &path,
        Arc::new(ScriptedModel::new(reference).turn_text("ok")),
    )
    .await;
    let events = drain(
        &reopened,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "hello".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            replay: false,
        },
    )
    .await;
    assert!(
        events
            .iter()
            .all(|event| !matches!(&event.kind, ServerEventKind::Error { .. })),
        "the turn runs: {events:?}"
    );
    assert_eq!(
        reopened.session_reasoning(&key),
        Some(Reasoning::Effort {
            effort: "high".to_owned(),
        }),
        "the stored override is what the turn used"
    );
    drop(reopened);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
