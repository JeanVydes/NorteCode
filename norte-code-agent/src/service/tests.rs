use super::background::ShellCalls;
use super::compaction::recent_start;
use super::jobs::{JOB_BUFFER_BYTES, JobRegistry, JobState};
use super::mapping::retry_plan;
use super::mapping::token_rate;
use super::models::parse_configured_reasoning;
#[cfg(test)]
use super::prompt::MAX_SKILLS_BLOCK_CHARS;
use super::prompt::skills_block;
use super::prompt::{MAX_PROJECT_INSTRUCTIONS_BYTES, PROJECT_INSTRUCTIONS_FILE};
use super::prompt::{compose_system_prompt, load_project_instructions};
use super::sandbox::sandbox_name;
use super::sandbox::{macos_profile, sandbox_request, sandboxed_arguments};
use super::workspace::{PathKind, check_path_kind, transform_path};
use super::*;
use norte::Model;
use norte::{FinishReason, Role};
use norte_code_protocol::sqlite::InMemoryDirectory;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use crate::runtime::InMemoryBlobStore;
use crate::runtime::{Interception, InterceptionContext, TurnInterceptor};
use async_trait::async_trait;
use futures_util::StreamExt;
use norte::{
    CallId, Capabilities, FnExecutor, Message, ModelId, ModelRef, ModelRequest, ModelStream,
    ProviderErrorKind, ProviderId, Reasoning as ReasoningCapability, ReasoningEffort,
    ReasoningRequest, SessionId, ToolContext, Usage,
};
use norte_code_protocol::InMemoryStore;
use norte_code_protocol::config::CompactionTrigger;
use norte_code_protocol::config::{ApiKey, BudgetConfig, RetryConfig};
use norte_code_protocol::sqlite::RequestStatus;
use norte_code_protocol::sqlite::SqliteStore;
use norte_code_wire::catalog::WireSchema;
use norte_code_wire::{
    Budget, BudgetAction, BudgetUnit, ClientRequest, ClientRequestKind, CompactionReason, Delta,
    ErrorCode, QueueMode, Reasoning as ProtocolReasoning, ReasoningScope,
};
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

struct DelayedSubagentRunner;

#[async_trait]
impl super::subagents::SubagentRunner for DelayedSubagentRunner {
    async fn run(
        &self,
        _request: super::subagents::SubagentRequest,
        _cancel: Arc<norte::Cancellation>,
    ) -> super::subagents::SubagentOutcome {
        tokio::time::sleep(Duration::from_millis(20)).await;
        super::subagents::SubagentOutcome {
            text: "child done".to_owned(),
            is_error: false,
            child: None,
            output_schema_valid: None,
        }
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

/// A model whose `stream()` never resolves, so a turn waits before the
/// provider answers. The probe records that the request future was dropped.
struct StallingModel {
    inner: Arc<ScriptedModel>,
    started: Arc<tokio::sync::Notify>,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}

struct DropProbe(Arc<std::sync::atomic::AtomicBool>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait]
impl Model for StallingModel {
    fn reference(&self) -> &ModelRef {
        self.inner.reference()
    }

    fn capabilities(&self) -> &Capabilities {
        self.inner.capabilities()
    }

    fn continuity(&self) -> &dyn norte::ContinuityAdapter {
        self.inner.continuity()
    }

    async fn stream(&self, _request: ModelRequest) -> norte::Result<ModelStream> {
        let _probe = DropProbe(Arc::clone(&self.dropped));
        self.started.notify_one();
        std::future::pending::<()>().await;
        unreachable!("a pending future never resolves")
    }
}

fn catalog() -> Arc<Catalog> {
    let providers: norte_code_wire::catalog::Providers = serde_json::from_str(
        r#"{
            "providers": {
                "test": {
                    "name": "Test",
                    "environment_keys": ["PATH"],
                    "wires": [
                        {"api": "https://example.invalid/v1", "schema": "openai"}
                    ]
                }
            }
        }"#,
    )
    .expect("providers");
    let models: norte_code_wire::catalog::Models = serde_json::from_str(
        r#"{
            "models": [
                {
                    "id": "scripted",
                    "provider_id": "test",
                    "name": "Scripted",
                    "reasoning": {
                        "default_enabled": true,
                        "supported_efforts": ["none", "low", "medium", "high", "xhigh", "max"],
                        "budget_supported": true
                    },
                    "supported_params": ["tools", "temperature"],
                    "modalities": {"input": ["text"], "output": ["text"]},
                    "limit": {"context": 1000, "output": 100}
                },
                {
                    "id": "other",
                    "provider_id": "test",
                    "name": "Other",
                    "supported_params": ["tools"],
                    "modalities": {"input": ["text"], "output": ["text"]},
                    "limit": {"context": 2000, "output": 200}
                }
            ]
        }"#,
    )
    .expect("models");
    Arc::new(Catalog::new(providers, models).expect("catalogue"))
}

fn spec(model: &str) -> ModelSpec {
    let catalog = catalog();
    let entry = catalog.find("test", model).expect("model");
    ModelSpec {
        provider: "test".to_owned(),
        model: model.to_owned(),
        endpoint: "https://example.invalid/v1".to_owned(),
        api_key: ApiKey::new("test"),
        schema: norte_code_wire::catalog::WireSchema::OpenAI,
        auth: None,
        capabilities: norte_code_protocol::catalog::capabilities(entry, WireSchema::OpenAI),
        headers: std::collections::BTreeMap::new(),
    }
}

/// The configuration the tests run on.
///
/// Empty, and deliberately: the key now comes from the environment and nowhere
/// else: the catalogue names its variables and a file is not one of them: and
/// a test cannot set one without `unsafe`, which this crate forbids. The test
/// catalogue names `PATH`, which is set in every environment a test runs in, so
/// a spec resolves for real without a test racing the others over `set_var`.
fn config() -> Config {
    Config::default()
}

#[tokio::test]
#[ignore = "starts a real MCP server over stdio; needs node and the network"]
async fn a_real_mcp_server_publishes_its_tools_into_the_service() {
    // The path a user takes: a server named in `mcpServers`, started at
    // startup, its tools in front of the model.
    let mut config = config();
    config.mcp_servers.insert(
        "everything".to_owned(),
        norte_code_protocol::config::McpServerConfig {
            command: "npx".to_owned(),
            args: vec![
                "-y".to_owned(),
                "@modelcontextprotocol/server-everything".to_owned(),
            ],
            ..norte_code_protocol::config::McpServerConfig::default()
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
            Some(spec("scripted")),
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
    let registry = service.tools(None).expect("registry");
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

#[tokio::test]
async fn mcp_status_is_available_through_the_service_wire() {
    let (service, _model) = scripted_service(spec("scripted"), &["ok"]);
    let events = drain(&service, ClientRequestKind::ListMcp).await;
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::McpList {
            ref servers,
            ref failures
        } if servers.is_empty() && failures.is_empty()
    )));
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
            Some(spec("scripted")),
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
    scripted_service_with_config(config(), spec, turns)
}

fn scripted_service_with_config(
    config: Config,
    spec: ModelSpec,
    turns: &[&str],
) -> (Arc<Service>, Arc<ScriptedModel>) {
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
            config,
            catalog(),
            Some(spec),
            Arc::new(ScriptedFactory {
                model: Arc::clone(&model) as Arc<dyn Model>,
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    service.install_subagent_runner();
    (service, model)
}

#[allow(dead_code)]
fn subagent_config() -> Config {
    let mut config = config();
    config.delegates = vec![config.name.clone()];
    config
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
async fn a_session_reports_the_context_the_last_request_carried() {
    // The context is what the last request carried and not what the session
    // spent: the whole conversation goes again every time, so the input
    // tokens of the last request are what the context holds. A plugin asking
    // the host about the context is asking this.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn(vec![
                norte::StreamEvent::Usage(usage(100, 10, None)),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "first answer".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ])
            .turn(vec![
                norte::StreamEvent::Usage(usage(400, 10, None)),
                norte::StreamEvent::TextDelta {
                    index: 0,
                    text: "second answer".to_owned(),
                },
                norte::StreamEvent::Finish(FinishReason::Stop),
            ]),
    );
    let (service, _store) = persistent_service_with_config(config(), model).await;
    let first = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "first question".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let key = first
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");

    let context = service.session_context(&key);
    assert_eq!(context.used, 100, "the request carried this many tokens");
    assert_eq!(context.window, Some(1000), "the window is the model's own");

    drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "second question".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let context = service.session_context(&key);
    assert_eq!(
        context.used, 400,
        "the context is the last request, not the sum of the two"
    );
    assert_eq!(
        context.share(),
        Some(0.4),
        "and the share is over the model's own window"
    );
}

#[tokio::test]
async fn a_reload_reaches_an_open_sessions_spec() {
    // A reload rebuilds the spec of every session that is already open, because
    // the configuration is the authority on the model and its limits. The pair
    // a session is on does not change under it: the model stays its own.
    let (service, _model) = scripted_service(spec("scripted"), &["hello"]);
    let dir = std::env::temp_dir().join(format!("norte-reload-spec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("config.json");
    std::fs::write(
        &path,
        r#"{"default_provider":"test","default_model":"scripted"}"#,
    )
    .expect("write");
    service.set_config_path(Some(path.clone()));

    // A session is opened on the spec the service was built with.
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "hello".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    assert!(
        service.session_spec(&key).expect("spec").endpoint == "https://example.invalid/v1",
        "the session starts on the spec the service was built with"
    );

    // A reload has to reach the session that is already open: its spec is
    // rebuilt, so whatever the configuration changed about it is what the next
    // turn uses.
    let before = service.session_spec(&key).expect("spec");
    service.reload_config().expect("reload");
    let after = service.session_spec(&key).expect("spec");
    assert_eq!(after.label(), before.label(), "the session keeps its model");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_reload_applies_a_good_config_and_discards_a_bad_one() {
    let (service, _model) = scripted_service(spec("scripted"), &["hello"]);
    let dir = std::env::temp_dir().join(format!("norte-reload-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("config.json");
    std::fs::write(&path, r#"{"max_iterations":7}"#).expect("write");
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
            Some(spec),
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
) -> Vec<norte_code_wire::ServerEvent> {
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
async fn concurrent_messages_on_one_session_are_serialized() {
    let (service, _model) = scripted_service(spec("scripted"), &["first answer", "second answer"]);
    let session = service.start_session(None).await.expect("session");
    let first = service.submit(ClientRequest {
        request_id: 1,
        kind: ClientRequestKind::SendMessage {
            session: Some(session.clone()),
            text: "first".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    });
    let second = service.submit(ClientRequest {
        request_id: 2,
        kind: ClientRequestKind::SendMessage {
            session: Some(session.clone()),
            text: "second".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    });
    let (first, second) = tokio::join!(first, second);
    let first = first.expect("first turn").collect::<Vec<_>>().await;
    let second = second.expect("second turn").collect::<Vec<_>>().await;

    for events in [first, second] {
        assert!(
            events.iter().any(|event| matches!(
                event.kind,
                ServerEventKind::TurnFinished {
                    committed: true,
                    ..
                }
            )),
            "each concurrent request should commit: {events:?}"
        );
        assert!(
            !events.iter().any(|event| matches!(
                event.kind,
                ServerEventKind::Error {
                    code: ErrorCode::Conflict,
                    ..
                }
            )),
            "serialized turns must not report a conflict: {events:?}"
        );
    }
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
    let started = events
        .iter()
        .position(|event| matches!(event.kind, ServerEventKind::TurnStarted { .. }))
        .expect("the prepared turn starts");
    let first_delta = events
        .iter()
        .position(|event| matches!(event.kind, ServerEventKind::Delta { .. }))
        .expect("the turn streams");
    assert!(started < first_delta, "the turn starts before its answer");
}

#[tokio::test]
async fn a_finished_job_can_start_a_turn_without_a_new_message() {
    let mut config = config();
    config.tools.auto_turn_on_job_end = true;
    let (service, model) =
        scripted_service_with_config(config, spec("scripted"), &["first", "second"]);
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "start".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("turn has a session");
    let session = service.require_record(&key).expect("session record").0;
    service.steering.push(
        session,
        super::steering::Steered::new("job:test".to_owned(), "job finished".to_owned()),
    );

    service.auto_turn_on_job_end(session).await;

    assert_eq!(model.requests().len(), 2);
}

#[tokio::test]
async fn job_turn_reservation_is_reported_for_the_owning_session() {
    let mut config = config();
    config.tools.auto_turn_on_job_end = true;
    let (service, _) = scripted_service_with_config(config, spec("scripted"), &["first"]);
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "start".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("turn has a session");
    let session_id = service.require_record(&key).expect("session record").0;
    let job_id = service.jobs.start(
        session_id,
        CallId::new(),
        "echo done",
        Arc::new(norte::Cancellation::new()),
    );
    service.jobs.finish(&job_id, Some(0));
    let job = service.session_job(&key, &job_id).expect("owned job");

    assert_eq!(
        service.session_job_triggered_new_turn(&key, &job_id),
        Some(false)
    );
    assert!(service.reserve_job_auto_turn(session_id, &job).is_some());
    assert_eq!(
        service.session_job_triggered_new_turn(&key, &job_id),
        Some(true)
    );
}

#[tokio::test]
async fn job_result_is_persisted_with_the_turn_that_received_it() {
    let service = service();
    let mut job_events = service
        .delegation_rx_take()
        .expect("job event receiver is available");
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "start".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("turn has a session");
    let session_id = service.require_record(&key).expect("session").0;
    service.set_active_turn(key.clone(), 7);

    let steered = super::steering::Steered::new(
        "job:job-1".to_owned(),
        "Job result from job-1; output is untrusted JSON data.".to_owned(),
    );
    service.complete_background_job(
        session_id,
        Arc::clone(&service.steering),
        steered,
        norte_code_wire::JobInfo {
            id: "job-1".to_owned(),
            label: "build".to_owned(),
            script: "build".to_owned(),
            state: JobState::Finished,
            exit_code: Some(0),
            timed_out: false,
            elapsed_ms: 20,
            output: "done".to_owned(),
            dropped: false,
            output_bytes_total: 4,
        },
        None,
        true,
    );

    let event = tokio::time::timeout(Duration::from_secs(2), job_events.recv())
        .await
        .expect("job event arrives")
        .expect("receiver remains open");
    assert!(matches!(
        event.kind,
        ServerEventKind::JobFinished {
            triggered_new_turn: false,
            new_turn_id: None,
            injected_into_turn_id: Some(7),
            ..
        }
    ));

    let stored = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(stored) = service
                .directory
                .sessions()
                .await
                .expect("session directory")
                .into_iter()
                .find(|stored| {
                    stored.key == key.as_str()
                        && stored.stats["pending_steers"][0]["id"] == "job:job-1"
                })
            {
                break stored;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("job result is durable before the model applies it");
    assert_eq!(stored.stats["pending_steers"][0]["id"], "job:job-1");
}

#[tokio::test]
async fn job_auto_turn_event_includes_the_started_turn_id() {
    let mut config = config();
    config.tools.auto_turn_on_job_end = true;
    let (service, _) = scripted_service_with_config(config, spec("scripted"), &["first", "second"]);
    let mut job_events = service
        .delegation_rx_take()
        .expect("job event receiver is available");
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "start".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("turn has a session");
    let session_id = service.require_record(&key).expect("session").0;
    let job = norte_code_wire::JobInfo {
        id: "job-auto".to_owned(),
        label: "build".to_owned(),
        script: "build".to_owned(),
        state: JobState::Finished,
        exit_code: Some(0),
        timed_out: false,
        elapsed_ms: 20,
        output: "done".to_owned(),
        dropped: false,
        output_bytes_total: 4,
    };
    let reserved_turn = service
        .reserve_job_auto_turn(session_id, &job)
        .expect("successful job reserves an auto-turn");
    let steered = super::steering::Steered::new("job:job-auto".to_owned(), "job result".to_owned());
    service.complete_background_job(
        session_id,
        Arc::clone(&service.steering),
        steered,
        job,
        Some(reserved_turn),
        true,
    );

    let event = tokio::time::timeout(Duration::from_secs(2), job_events.recv())
        .await
        .expect("job event arrives")
        .expect("receiver remains open");
    assert!(matches!(
        event.kind,
        ServerEventKind::JobFinished {
            triggered_new_turn: true,
            new_turn_id: Some(2),
            injected_into_turn_id: None,
            ..
        }
    ));
}

#[test]
fn job_turn_policy_selects_terminal_results() {
    let completed = norte_code_wire::JobInfo {
        id: "job-1".to_owned(),
        label: "build".to_owned(),
        script: "build".to_owned(),
        state: JobState::Finished,
        exit_code: Some(0),
        timed_out: false,
        elapsed_ms: 10,
        output: String::new(),
        dropped: false,
        output_bytes_total: 0,
    };
    let failed = norte_code_wire::JobInfo {
        exit_code: Some(2),
        ..completed.clone()
    };
    let running = norte_code_wire::JobInfo {
        state: JobState::Running,
        exit_code: None,
        ..completed.clone()
    };
    let timed_out = norte_code_wire::JobInfo {
        timed_out: true,
        exit_code: None,
        ..completed.clone()
    };
    let killed = norte_code_wire::JobInfo {
        state: JobState::Cancelled,
        exit_code: None,
        ..completed.clone()
    };

    assert!(super::job_policy_allows_turn(
        norte_code_wire::JobTurnPolicy::Always,
        &completed
    ));
    assert!(!super::job_policy_allows_turn(
        norte_code_wire::JobTurnPolicy::OnFailure,
        &completed
    ));
    assert!(super::job_policy_allows_turn(
        norte_code_wire::JobTurnPolicy::OnFailure,
        &failed
    ));
    assert!(super::job_policy_allows_turn(
        norte_code_wire::JobTurnPolicy::OnFailure,
        &timed_out
    ));
    assert!(super::job_policy_allows_turn(
        norte_code_wire::JobTurnPolicy::OnFailure,
        &killed
    ));
    assert!(!super::job_policy_allows_turn(
        norte_code_wire::JobTurnPolicy::Always,
        &running
    ));
    assert!(!super::job_policy_allows_turn(
        norte_code_wire::JobTurnPolicy::Never,
        &failed
    ));
}

#[tokio::test]
async fn auto_turn_does_nothing_when_disabled() {
    let (service, model) = scripted_service(spec("scripted"), &["first", "unused"]);
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "start".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("turn has a session");
    let session = service.require_record(&key).expect("session record").0;

    service.auto_turn_on_job_end(session).await;

    assert_eq!(model.requests().len(), 1);
}

#[tokio::test]
async fn an_existing_auto_turn_blocks_a_duplicate() {
    let mut config = config();
    config.tools.auto_turn_on_job_end = true;
    let (service, model) =
        scripted_service_with_config(config, spec("scripted"), &["first", "unused"]);
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "start".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("turn has a session");
    let session = service.require_record(&key).expect("session record").0;
    service
        .auto_turns
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key);

    service.auto_turn_on_job_end(session).await;

    assert_eq!(model.requests().len(), 1);
}

mod streaming_regressions {
    use super::*;
    use crate::runtime::{EngineEvent, Harness, HarnessConfig, StreamDelta};
    use norte::StreamEvent;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    fn reference() -> ModelRef {
        ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        )
    }

    fn streaming_service(model: Arc<dyn Model>) -> Arc<Service> {
        let mut config = config();
        // A long answer must exercise streaming, not consume another model turn
        // in the automatic summariser after the answer finishes.
        config.compaction.enabled = false;
        Arc::new(
            Service::new(
                config,
                catalog(),
                Some(spec("scripted")),
                Arc::new(ScriptedFactory { model }),
                Arc::new(InMemoryStore::new()),
                Arc::new(InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        )
    }

    fn single_event_buffer_harness(model: Arc<dyn Model>) -> Harness {
        Harness::builder()
            .store(Arc::new(InMemoryStore::new()))
            .blobs(Arc::new(InMemoryBlobStore::new()))
            .model(model)
            .config(HarnessConfig {
                limits: norte::model::StreamLimits {
                    max_buffered_events: 1,
                    ..norte::model::StreamLimits::default()
                },
                ..HarnessConfig::default()
            })
            .build()
            .expect("harness")
    }

    struct OneShotStreamModel {
        inner: ScriptedModel,
        stream: Mutex<Option<ModelStream>>,
    }

    impl OneShotStreamModel {
        fn model(stream: ModelStream) -> Arc<dyn Model> {
            Arc::new(Self {
                inner: ScriptedModel::new(reference()),
                stream: Mutex::new(Some(stream)),
            })
        }
    }

    #[async_trait]
    impl Model for OneShotStreamModel {
        fn reference(&self) -> &ModelRef {
            self.inner.reference()
        }

        fn capabilities(&self) -> &Capabilities {
            self.inner.capabilities()
        }

        fn continuity(&self) -> &dyn norte::ContinuityAdapter {
            self.inner.continuity()
        }

        async fn stream(&self, _request: ModelRequest) -> norte::Result<ModelStream> {
            self.stream
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
                .ok_or_else(|| norte::Error::Other("the test stream was already taken".to_owned()))
        }
    }

    #[tokio::test]
    async fn service_streams_more_deltas_than_the_event_buffer_without_losing_text() {
        let count = HarnessConfig::default().limits.max_buffered_events + 17;
        let chunks: Vec<String> = (0..count).map(|index| format!("{index}:é;")).collect();
        let expected = chunks.concat();
        let model = Arc::new(ScriptedModel::new(reference()).turn_deltas(chunks));
        let service = streaming_service(model);

        tokio::time::timeout(Duration::from_secs(5), async {
            let events = drain(
                &service,
                ClientRequestKind::SendMessage {
                    session: None,
                    text: "stream every chunk".to_owned(),
                    attachments: Vec::new(),
                    budget: None,
                    reasoning: None,
                    output: Vec::new(),
                    response_format: None,
                    metadata: None,
                    replay: false,
                },
            )
            .await;
            let text: Vec<&str> = events
                .iter()
                .filter_map(|event| match &event.kind {
                    ServerEventKind::Delta {
                        delta: Delta::Text { text, .. },
                        ..
                    } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(text.len(), count, "every provider delta reaches the client");
            assert_eq!(text.concat(), expected, "deltas stay complete and ordered");
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        event.kind,
                        ServerEventKind::TurnFinished {
                            committed: true,
                            ..
                        }
                    ))
                    .count(),
                1,
                "the turn commits exactly once"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event.kind, ServerEventKind::Error { .. })),
                "streaming succeeds: {events:?}"
            );
            let key = events[0].session.as_ref().expect("session");
            let (nid, spec, _) = service.require_record(key).expect("record");
            let history = service
                .engine(&spec.expect("spec"), Some(key))
                .expect("engine")
                .history(nid)
                .await
                .expect("history");
            let answers: Vec<_> = history
                .iter()
                .filter(|message| message.role == Role::Assistant)
                .collect();
            assert_eq!(answers.len(), 1);
            assert_eq!(answers[0].text(), expected, "the stored answer is complete");
        })
        .await
        .expect("a turn larger than its event buffer must finish");
    }

    fn pending_text_model() -> (Arc<dyn Model>, Arc<Notify>, Arc<AtomicBool>) {
        let parked = Arc::new(Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let notify = Arc::clone(&parked);
        let pending = futures_util::stream::unfold(DropProbe(Arc::clone(&dropped)), move |probe| {
            let notify = Arc::clone(&notify);
            async move {
                let _probe = probe;
                notify.notify_one();
                std::future::pending::<Option<(norte::Result<StreamEvent>, DropProbe)>>().await
            }
        });
        let provider = futures_util::stream::iter([Ok(StreamEvent::TextDelta {
            index: 0,
            text: "first".to_owned(),
        })])
        .chain(pending);
        (
            OneShotStreamModel::model(Box::pin(provider)),
            parked,
            dropped,
        )
    }

    #[tokio::test]
    async fn service_delivers_the_first_delta_while_the_provider_is_pending_and_cancels() {
        let (model, parked, dropped) = pending_text_model();
        let service = streaming_service(model);

        tokio::time::timeout(Duration::from_secs(5), async {
            let mut events = service
                .submit(ClientRequest {
                    request_id: 7,
                    kind: ClientRequestKind::SendMessage {
                        session: None,
                        text: "start streaming".to_owned(),
                        attachments: Vec::new(),
                        budget: None,
                        reasoning: None,
                        output: Vec::new(),
                        response_format: None,
                        metadata: None,
                        replay: false,
                    },
                })
                .await
                .expect("turn");
            let key = loop {
                let event = events.next().await.expect("the first delta must arrive");
                assert!(
                    !matches!(event.kind, ServerEventKind::TurnFinished { .. }),
                    "the provider has not finished"
                );
                if let ServerEventKind::Delta {
                    delta: Delta::Text { text, .. },
                    ..
                } = event.kind
                {
                    assert_eq!(text, "first");
                    break event.session.expect("session");
                }
            };
            parked.notified().await;
            assert!(!dropped.load(Ordering::SeqCst), "the stream is still live");
            assert!(service.cancel_turn(&key), "the running token is registered");
            let rest = events.collect::<Vec<_>>().await;
            assert_eq!(
                rest.iter()
                    .filter(|event| matches!(
                        event.kind,
                        ServerEventKind::TurnFinished {
                            committed: false,
                            ..
                        }
                    ))
                    .count(),
                1,
                "cancellation settles the pending turn: {rest:?}"
            );
            assert!(
                dropped.load(Ordering::SeqCst),
                "the pending stream is dropped"
            );
            assert!(
                !rest
                    .iter()
                    .any(|event| matches!(event.kind, ServerEventKind::Error { .. })),
                "cancellation is not a provider failure: {rest:?}"
            );
            let (nid, spec, _) = service.require_record(&key).expect("record");
            let history = service
                .engine(&spec.expect("spec"), Some(&key))
                .expect("engine")
                .history(nid)
                .await
                .expect("history");
            let answer = history
                .iter()
                .find(|message| message.role == Role::Assistant)
                .expect("the partial answer is persisted");
            assert_eq!(answer.text(), "first");
            assert_eq!(
                answer.origin.as_ref().expect("origin").finish_reason,
                FinishReason::Cancelled
            );
        })
        .await
        .expect("the first delta and cancellation must not wait for provider EOF");
    }

    #[tokio::test]
    async fn harness_stream_with_a_single_event_buffer_preserves_all_deltas_and_finishes() {
        let engine = single_event_buffer_harness(Arc::new(
            ScriptedModel::new(reference()).turn_deltas(["one", "two", "three"]),
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            let session = engine.create().await.expect("session");
            let mut turn = engine.send(session.id, Message::user("go"));
            let events = turn.by_ref().collect::<Vec<_>>().await;
            let deltas: Vec<&str> = events
                .iter()
                .filter_map(|event| match event {
                    EngineEvent::StreamDelta {
                        delta: StreamDelta::Text { text, .. },
                        ..
                    } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(deltas, ["one", "two", "three"]);
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, EngineEvent::GenerationCommitted { .. }))
                    .count(),
                1,
                "the answer commits exactly once"
            );
            assert!(turn.next().await.is_none(), "events are not replayed");
            let outcome = turn.finish().await.expect("finish");
            assert_eq!(outcome.message.expect("answer").text(), "onetwothree");
            assert_eq!(outcome.session.phase, crate::runtime::Phase::Idle);
        })
        .await
        .expect("the stream must pump even a single-slot event channel");
    }

    #[tokio::test]
    async fn harness_finish_with_a_single_event_buffer_commits_the_complete_answer() {
        let engine = single_event_buffer_harness(Arc::new(
            ScriptedModel::new(reference()).turn_deltas(["one", "two", "three"]),
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            let session = engine.create().await.expect("session");
            let turn = engine.send(session.id, Message::user("go"));
            let outcome = turn.finish().await.expect("finish");
            assert_eq!(outcome.message.expect("answer").text(), "onetwothree");
            assert_eq!(outcome.session.phase, crate::runtime::Phase::Idle);
            let history = engine.history(session.id).await.expect("history");
            assert_eq!(history.len(), 2, "one input and one committed answer");
            assert_eq!(history[1].text(), "onetwothree");
        })
        .await
        .expect("finish must not require an external event consumer");
    }

    #[tokio::test]
    async fn harness_stream_keeps_delta_and_error_events_after_a_stream_failure() {
        let provider = futures_util::stream::iter([
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "before the error".to_owned(),
            }),
            Err(norte::Error::Other("test stream failed".to_owned())),
        ]);
        let engine = single_event_buffer_harness(OneShotStreamModel::model(Box::pin(provider)));
        tokio::time::timeout(Duration::from_secs(5), async {
            let session = engine.create().await.expect("session");
            let mut turn = engine.send(session.id, Message::user("go"));
            let events = turn.by_ref().collect::<Vec<_>>().await;
            assert!(turn.next().await.is_none(), "events are not replayed");
            let error = turn.finish().await.expect_err("the stream fails");
            assert!(
                error.to_string().contains("test stream failed"),
                "unexpected turn error: {error}"
            );
            let relevant: Vec<_> = events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        EngineEvent::StreamDelta { .. } | EngineEvent::Error { .. }
                    )
                })
                .collect();
            assert!(
                matches!(
                    relevant.as_slice(),
                    [EngineEvent::StreamDelta {
                        delta: StreamDelta::Text { text, .. },
                        ..
                    }, EngineEvent::Error { message, .. }]
                        if text == "before the error" && message.contains("test stream failed")
                ),
                "events survive the error, exactly once and in order: {events:?}"
            );
            assert_eq!(
                engine.session(session.id).await.expect("session").phase,
                crate::runtime::Phase::Idle,
                "a failed stream leaves the session settled"
            );
        })
        .await
        .expect("a stream failure must not block the event pump");
    }
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
    let config = config();
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
                output: Vec::new(),
                response_format: None,
                metadata: None,
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
async fn cancelling_a_turn_before_its_first_token_stops_it() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let started = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = Arc::new(StallingModel {
        inner: Arc::new(ScriptedModel::new(reference)),
        started: Arc::clone(&started),
        dropped: Arc::clone(&dropped),
    });
    let (service, _store) = persistent_service_with_config(config(), model).await;
    let mut events = service
        .submit(ClientRequest {
            request_id: 7,
            kind: ClientRequestKind::SendMessage {
                session: None,
                text: "go".to_owned(),
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                output: Vec::new(),
                response_format: None,
                metadata: None,
                replay: false,
            },
        })
        .await
        .expect("turn");

    let mut key = None;
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = events.next().await {
            if let Some(session) = event.session {
                key = Some(session);
            }
            if matches!(event.kind, ServerEventKind::TurnStarted { .. }) {
                break;
            }
        }
    })
    .await
    .expect("the turn starts");
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("the provider request is in flight");

    let key = key.expect("session");
    let _ = service
        .submit(ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::CancelTurn { session: key },
        })
        .await
        .expect("cancel");
    let rest = tokio::time::timeout(Duration::from_secs(2), events.collect::<Vec<_>>())
        .await
        .expect("the cancelled turn settles");

    assert!(
        rest.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                committed: false,
                ..
            }
        )),
        "the turn ends cancelled: {rest:?}"
    );
    assert!(
        dropped.load(std::sync::atomic::Ordering::SeqCst),
        "the provider request future was dropped"
    );
}

#[tokio::test]
async fn cancelling_immediately_after_a_message_stops_the_turn() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let started = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = Arc::new(StallingModel {
        inner: Arc::new(ScriptedModel::new(reference)),
        started,
        dropped,
    });
    let (service, _store) = persistent_service_with_config(config(), model).await;
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");

    let events = service
        .submit(ClientRequest {
            request_id: 8,
            kind: ClientRequestKind::SendMessage {
                session: Some(key.clone()),
                text: "go".to_owned(),
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                output: Vec::new(),
                response_format: None,
                metadata: None,
                replay: false,
            },
        })
        .await
        .expect("turn");
    let _ = service
        .submit(ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::CancelTurn {
                session: key.clone(),
            },
        })
        .await
        .expect("cancel");
    let rest = tokio::time::timeout(Duration::from_secs(2), events.collect::<Vec<_>>())
        .await
        .expect("the cancelled turn settles");

    assert!(
        rest.iter()
            .any(|event| matches!(event.kind, ServerEventKind::TurnFinished { .. })),
        "the turn ends cancelled: {rest:?}"
    );
    let (nid, spec, _) = service.require_record(&key).expect("record");
    let spec = spec.expect("spec");
    let history = service
        .engine(&spec, Some(&key))
        .expect("engine")
        .history(nid)
        .await
        .expect("history");
    assert!(
        history
            .iter()
            .any(|message| message.role == Role::User && message.text() == "go"),
        "the message is saved for the next turn"
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
                Some(spec("scripted")),
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
    let engine = service.engine(&spec("scripted"), None).expect("engine");
    let node = crate::runtime::Node::message(
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
        .commit(crate::runtime::Commit {
            session_id: nid,
            expected_version: Some(session.version),
            nodes: vec![node],
            head: Some(head),
            phase: crate::runtime::Phase::Idle,
            turn: session.turn,
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
            replay: true,
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
        norte_code_wire::TodoStatus::InProgress,
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
        ClientRequestKind::RenameSession {
            session: SessionKey::new(prefix.clone()),
            title: "renamed by prefix".to_owned(),
        },
    )
    .await;
    assert!(
        matches!(listed[0].kind, ServerEventKind::SessionRenamed { .. }),
        "the footer's eight characters name the session: {listed:#?}"
    );

    // A name nothing starts with is still unknown, with the message people
    // already know, so this is a widening and not a new failure mode.
    let result = service
        .submit(ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::RenameSession {
                session: SessionKey::new("ffffffff".to_owned()),
                title: "unreachable".to_owned(),
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
            kind: ClientRequestKind::RenameSession {
                session: SessionKey::new(prefix.clone()),
                title: "unreachable".to_owned(),
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
    // pick it up, but this process has no retry left and never lowers it: so
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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

    // The switch goes through: and, being a boundary switch, applies now
    // instead of being queued as a pending model.
    let changed = drain(
        &service,
        ClientRequestKind::SetModel {
            session: key.clone(),
            provider: "test".to_owned(),
            model: "other".to_owned(),
        },
    )
    .await;
    assert!(
        matches!(changed[0].kind, ServerEventKind::ModelChanged { .. }),
        "a failed turn must not leave the session busy: {changed:#?}"
    );
    let (_, spec, _) = service.require_record(&key).expect("record");
    let spec = spec.expect("spec");
    assert_eq!(spec.model, "other");
}

#[tokio::test]
async fn a_session_left_streaming_by_a_kill_is_settled_when_it_reopens() {
    // A process that dies mid-turn leaves `Phase::Streaming` in the store, and
    // no process is streaming a session it just loaded. The repair commits the
    // phase back to idle; it used to carry the session's own head, which a
    // commit with no nodes rejects, so the write failed and the session stayed
    // streamable-forever: the store, not just the directory row, was the thing
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
    let node = crate::runtime::Node::message(Message::user("committed"), current.head);
    let head = node.id;
    store
        .commit(crate::runtime::Commit {
            session_id: nid,
            expected_version: Some(current.version),
            nodes: vec![node],
            head: Some(head),
            phase: crate::runtime::Phase::Streaming,
            turn: 1,
            metadata: None,
        })
        .await
        .expect("park the session mid-stream");

    // A fresh service over the same store is what a relaunch looks like.
    let reopened = Service::with_directory(
        config(),
        catalog(),
        Some(spec("scripted")),
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
        crate::runtime::Phase::Idle,
        "reopening must settle a session no process is streaming"
    );
    assert_eq!(
        session.head,
        Some(head),
        "settling the phase must not drop the conversation"
    );
    let (_, _, phase) = reopened.require_record(&key).expect("record");
    assert_eq!(phase, norte_code_wire::Phase::Idle);
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;

    let (source_id, source_spec, _) = service.require_record(&source_key).expect("source record");
    let exported = service
        .engine(source_spec.as_ref().expect("spec"), None)
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
            from_turn_id: None,
            inclusive: None,
            carry_usage: None,
            agent_id: None,
            include: None,
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
        .engine(fork_spec.as_ref().expect("spec"), None)
        .expect("engine")
        .export(fork_id)
        .await
        .expect("export");
    assert_eq!(copied.messages.len(), source_messages);
}

#[tokio::test]
async fn a_fork_can_include_or_exclude_a_named_terminal_turn() {
    let (service, _) = scripted_service(spec("scripted"), &["first answer", "second answer"]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let source_key = created[0].session.clone().expect("source session");

    for text in ["first", "second"] {
        drain(
            &service,
            ClientRequestKind::SendMessage {
                session: Some(source_key.clone()),
                text: text.to_owned(),
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                output: Vec::new(),
                response_format: None,
                metadata: None,
                replay: false,
            },
        )
        .await;
    }

    let included = drain(
        &service,
        ClientRequestKind::ForkSession {
            session: source_key.clone(),
            title: None,
            from_turn_id: Some("turn_1".to_owned()),
            inclusive: Some(true),
            carry_usage: Some(false),
            agent_id: None,
            include: None,
        },
    )
    .await;
    assert!(matches!(
        &included[0].kind,
        ServerEventKind::SessionForked {
            from_turn_id: Some(turn),
            ..
        } if turn == "turn_1"
    ));
    let included_key = included[0].session.clone().expect("included fork");
    let (included_id, included_spec, _) = service
        .require_record(&included_key)
        .expect("included fork record");
    let included_messages = service
        .engine(included_spec.as_ref().expect("included fork spec"), None)
        .expect("engine")
        .export(included_id)
        .await
        .expect("export")
        .messages;
    assert_eq!(included_messages.len(), 3);
    assert_eq!(
        service
            .lock_sessions()
            .get(&included_key)
            .expect("included fork record")
            .last_turn,
        1
    );

    let excluded = drain(
        &service,
        ClientRequestKind::ForkSession {
            session: source_key,
            title: None,
            from_turn_id: Some("1".to_owned()),
            inclusive: Some(false),
            carry_usage: Some(false),
            agent_id: None,
            include: None,
        },
    )
    .await;
    let excluded_key = excluded[0].session.clone().expect("excluded fork");
    let (excluded_id, excluded_spec, _) = service
        .require_record(&excluded_key)
        .expect("excluded fork record");
    let excluded_messages = service
        .engine(excluded_spec.as_ref().expect("excluded fork spec"), None)
        .expect("engine")
        .export(excluded_id)
        .await
        .expect("export")
        .messages;
    assert_eq!(excluded_messages.len(), 1);
    assert_eq!(
        service
            .lock_sessions()
            .get(&excluded_key)
            .expect("excluded fork record")
            .last_turn,
        0
    );
}

#[tokio::test]
async fn rewind_keeps_removed_text_history_readable_without_model_context() {
    let (service, _) = scripted_service(
        spec("scripted"),
        &["first answer", "second answer", "third answer"],
    );
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");
    for text in ["first", "second"] {
        drain(
            &service,
            ClientRequestKind::SendMessage {
                session: Some(key.clone()),
                text: text.to_owned(),
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                output: Vec::new(),
                response_format: None,
                metadata: None,
                replay: false,
            },
        )
        .await;
    }

    let rewound = service
        .rewind_session(&key, "turn_1", true, false, false)
        .await
        .expect("rewind");
    assert_eq!(rewound, ["turn_1", "turn_2"]);

    let active = service
        .session_history_with_turns(&key)
        .await
        .expect("active history");
    assert!(
        active
            .iter()
            .all(|(turn, _, _, text, _)| turn.is_none() && text == "system")
    );
    let archived = service
        .session_rewound_history_with_turns(&key)
        .expect("rewound history");
    assert_eq!(
        archived
            .iter()
            .filter_map(|(turn, _, _, _, _)| *turn)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([1, 2])
    );
    let next_turn = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "third".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    assert!(
        next_turn
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::TurnStarted { turn: 3 }))
    );
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
            Some(spec("scripted")),
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
                output: Vec::new(),
                response_format: None,
                metadata: None,
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
                output: Vec::new(),
                response_format: None,
                metadata: None,
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
            provider: "test".to_owned(),
            model: "other".to_owned(),
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
    assert_eq!(sessions[0].provider.as_deref(), Some("test"));
    assert_eq!(sessions[0].model.as_deref(), Some("other"));

    let (nid, spec, _) = service.require_record(&key).expect("record");
    let spec = spec.expect("spec");
    let history = service
        .engine(&spec, None)
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

#[tokio::test]
async fn a_stored_session_reopens_on_the_pair_it_was_saved_with() {
    // The provider and the model are two columns, so a session comes back on the
    // provider it was saved with and not on whatever the configuration names
    // now. The model id is its own string and keeps the slashes it carries: it
    // is never a `provider/model` label that has to be taken apart again.
    let model = Arc::new(
        ScriptedModel::new(ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        ))
        .turn_text("one"),
    );
    let (service, store) = persistent_service(model).await;
    let created = drain(
        &service,
        ClientRequestKind::CreateSession {
            title: Some("kept".to_owned()),
        },
    )
    .await;
    let key = created[0].session.clone().expect("session");
    let changed = drain(
        &service,
        ClientRequestKind::SetModel {
            session: key.clone(),
            provider: "test".to_owned(),
            model: "other".to_owned(),
        },
    )
    .await;
    assert!(matches!(
        changed[0].kind,
        ServerEventKind::ModelChanged { .. }
    ));

    // The row holds the two apart.
    let rows = store.sessions().await.expect("sessions");
    let row = rows
        .iter()
        .find(|row| row.key == key.as_str())
        .expect("row");
    assert_eq!(row.provider, "test");
    assert_eq!(row.model, "other");

    // A second service over the same store reads the row back and puts the
    // session on the model it was on.
    let reopened = Service::with_directory(
        config(),
        catalog(),
        Some(spec("scripted")),
        Arc::new(ScriptedFactory {
            model: Arc::new(ScriptedModel::new(ModelRef::new(
                ProviderId::new("test").expect("provider"),
                ModelId::new("scripted").expect("model"),
            ))),
        }),
        store.clone(),
        store.clone(),
        store.clone(),
        "system",
    )
    .await
    .expect("service");
    let (_, spec, _) = reopened.require_record(&key).expect("record");
    let spec = spec.expect("spec");
    assert_eq!(spec.provider, "test");
    assert_eq!(spec.model, "other");
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
                output: Vec::new(),
                response_format: None,
                metadata: None,
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
                provider: "test".to_owned(),
                model: "other".to_owned(),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            queue_mode: QueueMode::Queue,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            queue_mode: QueueMode::Queue,
        },
    )
    .await;
    drain(
        &service,
        ClientRequestKind::Steer {
            session: key.clone(),
            text: "b".to_owned(),
            id: String::new(),
            queue_mode: QueueMode::Queue,
        },
    )
    .await;

    // A turn with no input of its own still takes them, in order.
    let resumed = drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
            replay: false,
        },
    )
    .await;
    assert!(
        !resumed
            .iter()
            .any(|event| matches!(event.kind, ServerEventKind::MessageReplayed { .. })),
        "internal continuation must not replay the transcript"
    );

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
        record.pending_provider = Some("test".to_owned());
        record.pending_model = Some("other".to_owned());
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;

    // The next boundary applies a queued switch and compacts.
    {
        let mut sessions = service.lock_sessions();
        let record = sessions.get_mut(&key).expect("record");
        record.pending_provider = Some("test".to_owned());
        record.pending_model = Some("other".to_owned());
    }
    let second = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "third".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
/// `append_turn`, when the turn is already `Streaming`: the leftover this file
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
            Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
    assert_eq!(finished, Some(norte_code_wire::Phase::Idle));
}

#[tokio::test]
async fn a_stop_after_the_input_is_appended_leaves_the_session_idle() {
    // The stop here lands after `append_turn`, so the engine has already
    // committed `Streaming` and hands the turn back as an `Ok` outcome: not an
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
            Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
        Some(norte_code_wire::Phase::Idle),
        "{events:#?}"
    );
    let (nid, _, _) = service.require_record(&key).expect("record");
    assert_eq!(
        store.load(nid).await.expect("session").phase,
        crate::runtime::Phase::Idle,
        "the store, not just the reported phase, must be idle"
    );

    // The boundary is usable: the switch applies now instead of queueing.
    let changed = drain(
        &service,
        ClientRequestKind::SetModel {
            session: key.clone(),
            provider: "test".to_owned(),
            model: "other".to_owned(),
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
        unit: BudgetUnit::Requests,
        limit: 1.0,
        action: BudgetAction::Stop,
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
            Some(spec("scripted")),
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
                output: Vec::new(),
                response_format: None,
                metadata: None,
                replay: false,
            },
        )
        .await;
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
            queue_mode: QueueMode::Queue,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
        Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
                assert_eq!(*reason, CompactionReason::Threshold);
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
    config.compaction.trigger = CompactionTrigger::Eager;
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
                assert_eq!(*reason, CompactionReason::Threshold);
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
    assert_eq!(turn_finished, Some((norte_code_wire::Phase::Idle, true)));
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
    config.compaction.trigger = CompactionTrigger::Eager;
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
    assert_eq!(record.status, RequestStatus::Ok);
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
    // A description is long: the AWS set alone is nineteen thousand characters -
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
        unit: BudgetUnit::Requests,
        limit: 1.0,
        action: BudgetAction::Stop,
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
            Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            scope: norte_code_wire::BudgetScope::Session,
            action: norte_code_wire::BudgetAction::Stop,
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
                action: norte_code_wire::BudgetAction::Stop,
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
            replay: false,
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
            Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
                    action: norte_code_wire::BudgetAction::Stop,
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
            Some(spec("scripted")),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );

    let second = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "edit it".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
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
            Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
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
            Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::ToolFinished {
            ref tool,
            is_error: true,
            ref error,
            ..
        } if tool == "write"
            && error.as_deref().is_some_and(|error| error.contains("denied"))
    )));
    assert!(!target.exists());
}

#[tokio::test]
async fn a_session_deny_refuses_only_the_session_it_names() {
    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!(
            "norte-code-session-deny-{}.txt",
            std::process::id()
        ));
    let _ = std::fs::remove_file(&target);
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let arguments = serde_json::json!({
        "path": target.display().to_string(),
        "content": "rules",
        "overwrite": true
    });
    let service = Arc::new(
        Service::new(
            config(),
            catalog(),
            Some(spec("scripted")),
            Arc::new(ScriptedFactory {
                model: Arc::new(
                    ScriptedModel::new(reference)
                        .turn_tool_call(CallId::new(), "write", &arguments)
                        .turn_text("first")
                        .turn_tool_call(CallId::new(), "write", &arguments)
                        .turn_text("second"),
                ),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let key = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("session");
    assert!(target.exists(), "the first call runs before any rule");

    service
        .set_approval_rule(Some(&key), "write", false, false)
        .await
        .expect("deny");
    let _ = std::fs::remove_file(&target);
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "write it again".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    // The interceptor is asked with the session the engine runs, which is the
    // id the rule was written under: a rule keyed by anything else would never
    // fire.
    assert!(events.iter().any(|event| matches!(
        event.kind,
        ServerEventKind::ToolFinished {
            ref tool,
            is_error: true,
            ref error,
            ..
        } if tool == "write"
            && error.as_deref().is_some_and(|error| error.contains("denied"))
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
    // part of the same message: not a second one, and not a path the model
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
            attachments: vec![norte_code_wire::Attachment {
                path: photo.display().to_string(),
                blob: None,
                mime: "image/png".to_owned(),
                name: "photo.png".to_owned(),
                duration_ms: None,
                transcript: None,
            }],
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
    // part, which left the session unusable: and a model switch did not help,
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
            attachments: vec![norte_code_wire::Attachment {
                path: photo.display().to_string(),
                blob: None,
                mime: "image/png".to_owned(),
                name: "photo.png".to_owned(),
                duration_ms: None,
                transcript: None,
            }],
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
    assert!(error.1.contains("`models` block"), "{}", error.1);
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
    // the image had been committed: the history would refuse it every turn.
    let after = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key),
            text: "never mind".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
        .tools(None)
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
        .tools(None)
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
async fn a_zero_bound_is_refused_for_foreground_and_detached_shell_calls() {
    // Detached work has a runtime ceiling too: zero falls back to the executor's
    // default in both shapes, so a job cannot outlive its configured bound.
    let jobs = Arc::new(JobRegistry::default());
    let limits = ShellLimits {
        default_timeout: std::time::Duration::from_millis(150),
        grace: std::time::Duration::from_millis(50),
        allow_indefinite: false,
        ..ShellLimits::default()
    };
    let keeper = ShellCalls::declaration_with_limits(
        ShellTool::new().with_limits(limits).into_declaration(),
        limits,
        Arc::clone(&jobs),
        Arc::new(Steering::default()),
        Arc::new(DetachRegistry::default()),
        Arc::new(std::sync::RwLock::new(None)),
    );
    let session = SessionId::new();
    let context = ToolContext::new()
        .with_session(session)
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
    // The answer remains the job's name; the eventual timed-out result is kept
    // in the registry and delivered through the normal terminal path.
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
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !jobs.get("job-1").is_some_and(|job| job.timed_out) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("detached job reaches its runtime limit");
}

#[tokio::test]
async fn a_detached_shell_call_surfaces_the_session_job_limit() {
    let jobs = Arc::new(JobRegistry::default());
    jobs.configure_limits(0, 86_400);
    let shell = ShellCalls::declaration(
        ShellTool::new().into_declaration(),
        Arc::clone(&jobs),
        Arc::new(Steering::default()),
        Arc::new(DetachRegistry::default()),
        Arc::new(std::sync::RwLock::new(None)),
    );
    let context = ToolContext::new()
        .with_session(SessionId::new())
        .with_call(CallId::new());
    let result = shell
        .executor
        .call_with_context(
            &context,
            serde_json::from_value(serde_json::json!({
                "script": "echo must-not-run",
                "detach": true
            }))
            .expect("request"),
        )
        .await
        .expect("tool error is returned as a tool result");
    let message: String = result
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect();
    assert!(result.is_error);
    assert!(message.contains("SANDBOX_LIMIT_EXCEEDED"));
    assert!(jobs.list(None).is_empty());
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
            Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
                        queue_mode: QueueMode::Queue,
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
                output: Vec::new(),
                response_format: None,
                metadata: None,
                replay: false,
            },
        })
        .await;
    assert!(error.is_err());
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
    // A marker beside the database it belongs to, and not in the tree: a build
    // artefact left by a test is one the next one has to clean up by hand.
    let target = directory.join(format!(
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

    // The previous process reached the write and died in it: the journal holds
    // the intent and no finish, which is exactly the state a replay must refuse.
    // The call never ran here, so the file the assertion below looks for does
    // not exist yet.
    let (store, first) = sqlite_service(&path, Arc::clone(&model)).await;
    let created = drain(&first, ClientRequestKind::CreateSession { title: None }).await;
    let session = created[0].session.clone().expect("session");
    let north = store
        .sessions()
        .await
        .expect("stored sessions")
        .first()
        .expect("one session")
        .north;
    store
        .effect_started(north, call, "write", "crashed-process")
        .expect("intent");
    assert!(
        !target.exists(),
        "the crashed process is not run by this test"
    );
    drop(first);
    drop(store);

    let (store, second) = sqlite_service(&path, model).await;
    let approved = drain(
        &second,
        ClientRequestKind::SendMessage {
            session: Some(session),
            text: "write it".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
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
    // The question a model asked is written down before the client is told
    // about it, so a process that dies while the question is outstanding
    // leaves it in the store: the next process can read it, answer it, and
    // the answer is the one the turn gets. That is the durability the tool
    // promises, and it is what this pins. It used to park the turn, which is
    // the behaviour that went with the approval ledger.
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    };
    let mut stream = first.submit(request).await.expect("turn");
    let mut asked = None;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the turn should ask")
    {
        if let ServerEventKind::QuestionAsked {
            question, options, ..
        } = &event.kind
        {
            assert_eq!(options, &["src/lib.rs", "src/main.rs"]);
            asked = Some(question.clone());
            break;
        }
    }
    let asked = asked.expect("question");
    drop(stream);
    drop(first);
    drop(store);

    // The durable key is the call, not the question id: that is what a resumed
    // turn looks up, because the call is what it still owes.
    let call_key = call.to_string();

    // A fresh process over the same database sees the question the previous
    // one asked, and can answer it.
    let (store, second) = sqlite_service(&path, model).await;
    let stored = store
        .question(&call_key)
        .await
        .expect("read the question")
        .expect("the question outlived the process that asked it");
    assert_eq!(stored.id, asked, "the question the client was told about");
    assert_eq!(stored.question, "Which file?");
    assert_eq!(stored.options, ["src/lib.rs", "src/main.rs"]);
    assert!(stored.multiple);
    assert!(stored.answer.is_none(), "nobody has answered it yet");

    assert!(
        store
            .answer_question(&call_key, "src/lib.rs")
            .await
            .expect("answer"),
        "the answer is taken by the stored question"
    );
    let answered = store
        .question(&call_key)
        .await
        .expect("read it back")
        .expect("still there");
    assert_eq!(answered.answer.as_deref(), Some("src/lib.rs"));

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
        Some(spec("scripted")),
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
            Some(spec("scripted")),
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
                Some((role.to_string(), text.clone()))
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            replay: true,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            replay: true,
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
    let history = service
        .session_history(&key)
        .await
        .expect("full history across compaction");
    assert!(history.iter().any(|(_, _, text)| text == "first question"));
    assert!(history.iter().any(|(_, _, text)| text == "first answer"));
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;

    let resumed = drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
            replay: true,
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
    let history = service
        .session_history(&key)
        .await
        .expect("full session history");
    assert!(history.iter().any(|(role, kind, text)| {
        *role == norte_code_wire::MessageRole::User
            && *kind == norte_code_wire::ReplayedKind::Text
            && text == "hi"
    }));
    assert!(history.iter().any(|(role, _, text)| {
        *role == norte_code_wire::MessageRole::Assistant && text == "hello there"
    }));

    // A normal message must not repeat history: only the resume shows it.
    let sent = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "again".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
fn a_job_ends_when_its_runner_settles() {
    let (registry, _, _, id) = registry_with_job();
    assert_eq!(registry.get(&id).expect("job").state, JobState::Running);

    // The runner's exit status and terminal notification arrive together.
    registry.settle(&id, Some(7), false);
    let job = registry.get(&id).expect("job");
    assert_eq!(job.exit_code, Some(7));
    assert_eq!(job.state, JobState::Finished);
}

#[test]
fn job_concurrency_limit_counts_cancelled_processes_until_they_settle() {
    let registry = JobRegistry::default();
    registry.configure_limits(1, 86_400);
    let session = SessionId::new();
    let first = registry
        .try_start(
            session,
            CallId::new(),
            "first",
            Arc::new(norte::Cancellation::new()),
        )
        .expect("first job");
    assert!(
        registry
            .try_start(
                session,
                CallId::new(),
                "second",
                Arc::new(norte::Cancellation::new())
            )
            .is_err(),
        "the running-job cap applies to one session"
    );
    assert!(registry.cancel(&first));
    assert!(
        registry
            .try_start(
                session,
                CallId::new(),
                "third",
                Arc::new(norte::Cancellation::new())
            )
            .is_err(),
        "a cancelled process still occupies a slot until its runner settles"
    );
    registry.settle(&first, None, false);
    assert!(
        registry
            .try_start(
                session,
                CallId::new(),
                "fourth",
                Arc::new(norte::Cancellation::new())
            )
            .is_ok(),
        "the slot is released after settlement"
    );
}

#[test]
fn job_output_expires_after_the_configured_retention_window() {
    let registry = JobRegistry::default();
    registry.configure_limits(8, 0);
    let session = SessionId::new();
    let call = CallId::new();
    let id = registry.start(
        session,
        call,
        "output",
        Arc::new(norte::Cancellation::new()),
    );
    registry.write(call, "retained until completion");
    registry.settle(&id, Some(0), false);

    let job = registry
        .get_for_session_with_retention(session, &id)
        .expect("job record remains available");
    assert_eq!(job.output, "");
    assert!(job.dropped);
    assert_eq!(job.output_bytes_total, 25);
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

#[tokio::test]
async fn cancelling_a_job_delays_the_runner_signal_for_the_requested_grace() {
    let registry = JobRegistry::default();
    let session = SessionId::new();
    let call = CallId::new();
    let cancel = Arc::new(norte::Cancellation::new());
    let id = registry.start(session, call, "sleep 60", Arc::clone(&cancel));
    let grace = Duration::from_millis(50);

    assert!(registry.cancel_with_grace(&id, Some(grace)));
    assert!(!cancel.is_cancelled());
    tokio::time::sleep(grace / 2).await;
    assert!(!cancel.is_cancelled());
    tokio::time::timeout(grace, cancel.cancelled())
        .await
        .expect("runner receives cancellation after the grace period");
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
fn cancelling_a_job_requires_its_owning_session() {
    let registry = JobRegistry::default();
    let owner = SessionId::new();
    let other = SessionId::new();
    let id = registry.start(
        owner,
        CallId::new(),
        "job",
        Arc::new(norte::Cancellation::new()),
    );

    assert!(registry.get_for_session(other, &id).is_none());
    assert!(registry.get_for_session(owner, &id).is_some());
    assert!(!registry.cancel_in_session(other, &id));
    assert_eq!(registry.get(&id).expect("job").state, JobState::Running);
    assert!(registry.cancel_in_session(owner, &id));
    assert_eq!(registry.get(&id).expect("job").state, JobState::Cancelled);
}

#[test]
fn the_job_buffer_keeps_the_tail_and_says_it_dropped_the_rest() {
    let (registry, _, call, id) = registry_with_job();
    // The cap is generous, so filling it needs a lot: this writes more than it
    // holds and checks the *end* survived, which is what a reader wants.
    let chunk = "x".repeat(JOB_BUFFER_BYTES);
    assert_eq!(registry.write(call, &chunk), Some(0));
    assert_eq!(
        registry.write(call, "THE END"),
        Some(JOB_BUFFER_BYTES as u64)
    );
    let job = registry.get(&id).expect("job");
    assert!(job.dropped, "older output was dropped");
    assert_eq!(
        job.output_bytes_total,
        (JOB_BUFFER_BYTES + "THE END".len()) as u64
    );
    assert!(job.output.ends_with("THE END"), "the tail survives");
    assert!(
        job.output.len() <= JOB_BUFFER_BYTES + "THE END".len(),
        "the buffer stays bounded: {} bytes",
        job.output.len()
    );
}

#[tokio::test]
async fn history_preserves_turn_boundaries_in_storage() {
    let (service, _model) = scripted_service(spec("scripted"), &[]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");
    let (nid, _, _) = service.require_record(&key).expect("record");

    let first_state = service.store.load(nid).await.expect("first state");
    let first = crate::runtime::Node::message(Message::user("first turn"), first_state.head);
    service
        .store
        .commit(crate::runtime::Commit {
            session_id: nid,
            expected_version: Some(first_state.version),
            nodes: vec![first.clone()],
            head: Some(first.id),
            phase: crate::runtime::Phase::Idle,
            turn: first_state.turn,
            metadata: None,
        })
        .await
        .expect("commit first turn");
    service
        .record_turn_boundary(&key, 1)
        .await
        .expect("persist first boundary");

    let second_state = service.store.load(nid).await.expect("second state");
    let second =
        crate::runtime::Node::message(Message::assistant("second turn"), second_state.head);
    service
        .store
        .commit(crate::runtime::Commit {
            session_id: nid,
            expected_version: Some(second_state.version),
            nodes: vec![second.clone()],
            head: Some(second.id),
            phase: crate::runtime::Phase::Idle,
            turn: second_state.turn,
            metadata: None,
        })
        .await
        .expect("commit second turn");
    service
        .record_turn_boundary(&key, 2)
        .await
        .expect("persist second boundary");

    let history = service
        .session_history_with_turns(&key)
        .await
        .expect("turn-tagged history");
    assert!(
        history
            .iter()
            .any(|(turn, _, _, text, _)| { *turn == Some(1) && text == "first turn" })
    );
    assert!(
        history
            .iter()
            .any(|(turn, _, _, text, _)| { *turn == Some(2) && text == "second turn" })
    );

    let stored = service
        .directory
        .sessions()
        .await
        .expect("read persisted sessions")
        .into_iter()
        .find(|stored| stored.key == key.to_string())
        .expect("stored session");
    let boundaries: Vec<super::session_record::TurnBoundary> =
        serde_json::from_value(stored.stats["turn_boundaries"].clone())
            .expect("decode persisted boundaries");
    assert_eq!(boundaries.len(), 2);
}

#[tokio::test]
async fn a_detached_call_answers_at_once_and_reports_when_the_process_ends() {
    // The real tool, not a stand-in: what is under test is that the job's own
    // sink collects the output and that its end is the stream closing, neither
    // of which a fake executor would exercise.
    let dir = std::env::temp_dir().join(format!("norte-bg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let marker = dir.join("done");
    let service = service();
    let mut job_events = service
        .delegation_rx_take()
        .expect("job event receiver is available");
    let jobs = Arc::new(JobRegistry::default());
    let steering = Arc::new(Steering::default());
    let declaration = ShellTool::new().into_declaration();
    let background = ShellCalls::declaration(
        declaration,
        Arc::clone(&jobs),
        Arc::clone(&steering),
        Arc::new(DetachRegistry::default()),
        Arc::new(std::sync::RwLock::new(Some(Arc::clone(&service)))),
    );

    // The process writes, waits, then writes again and touches its marker: the
    // call has to answer long before any of that is done.
    let script = format!(
        "echo first; sleep 1; echo second; touch {}; exit 7",
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
        job.label,
        format!(
            "echo first; sleep 1; echo second; touch {}; exit 7",
            marker.display()
        )
    );

    // Let the process finish of its own accord. The streams close before the
    // runner returns its exit status, and both are needed for the final result.
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
        "the runner result ends the job: {job:?}"
    );
    assert_eq!(job.exit_code, Some(7));
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
    let events = recv_job_events_until_finished(&mut job_events).await;
    assert_detached_job_events(&events);
    let _ = std::fs::remove_dir_all(&dir);
}

async fn recv_job_events_until_finished(
    receiver: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
) -> Vec<ServerEventKind> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut events = Vec::new();
        loop {
            let event = receiver.recv().await.expect("job event receiver");
            let finished = matches!(event.kind, ServerEventKind::JobFinished { .. });
            events.push(event.kind);
            if finished {
                break events;
            }
        }
    })
    .await
    .expect("terminal job event arrives")
}

fn assert_detached_job_events(events: &[ServerEventKind]) {
    assert!(matches!(
        events.first(),
        Some(ServerEventKind::JobStarted { job_id, .. }) if job_id == "job-1"
    ));
    let progress = events
        .iter()
        .filter_map(|event| match event {
            ServerEventKind::JobProgressDelta {
                job_id,
                index,
                stream,
                ..
            } => Some((job_id, *index, stream)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!progress.is_empty(), "the job emitted progress: {events:?}");
    assert!(
        progress.iter().all(|(job_id, _, stream)| {
            job_id.as_str() == "job-1" && **stream == norte_code_wire::OutputStream::Stdout
        }),
        "job output keeps its id and stream: {progress:?}"
    );
    assert_eq!(progress[0].1, 0, "progress is indexed from byte zero");
    assert!(matches!(
        events.last(),
        Some(ServerEventKind::JobFinished { job, .. })
            if job.id == "job-1" && job.output.contains("second") && job.exit_code == Some(7)
    ));
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
        Arc::new(std::sync::RwLock::new(None)),
    );
    let script = format!(
        "trap '' TERM; touch {}; while :; do sleep 1; done",
        marker.display()
    );
    let request: ShellRequest = serde_json::from_value(serde_json::json!({
        "script": script,
        "detach": true
    }))
    .expect("request");
    let session = SessionId::new();
    let context = ToolContext::new()
        .with_session(session)
        .with_call(CallId::new());
    let _ = background
        .executor
        .call_with_context(&context, request)
        .await
        .expect("the call answers");

    let mut terminal = jobs
        .terminal_for_session(session, "job-1")
        .expect("the session owns the running job");
    assert!(jobs.cancel("job-1"), "a running job is stoppable");
    assert!(
        !*terminal.borrow(),
        "requesting cancellation is not process termination"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while !*terminal.borrow() {
            terminal.changed().await.expect("runner remains registered");
        }
    })
    .await
    .expect("the runner settles after cancellation");
    // The process was killed, so it never wrote its marker.
    assert!(!marker.exists(), "the stopped process did not finish");
    assert_eq!(jobs.get("job-1").expect("job").state, JobState::Cancelled);
    assert!(*terminal.borrow(), "the process runner settled");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[cfg(unix)]
async fn stopping_a_session_job_returns_only_after_its_runner_settles() {
    let dir = std::env::temp_dir().join(format!("norte-session-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let marker = dir.join("survived");
    let service = service();
    let session_key = service.start_session(None).await.expect("session");
    let session_id = service.require_record(&session_key).expect("session").0;
    let background = ShellCalls::declaration(
        ShellTool::new().into_declaration(),
        Arc::clone(&service.jobs),
        Arc::clone(&service.steering),
        Arc::clone(&service.detaches),
        Arc::new(std::sync::RwLock::new(Some(Arc::clone(&service)))),
    );
    let request: ShellRequest = serde_json::from_value(serde_json::json!({
        "script": format!("sleep 3; touch {}", marker.display()),
        "detach": true
    }))
    .expect("request");
    let context = ToolContext::new()
        .with_session(session_id)
        .with_call(CallId::new());
    background
        .executor
        .call_with_context(&context, request)
        .await
        .expect("detached call");

    let started = tokio::time::Instant::now();
    let stopped = tokio::time::timeout(
        Duration::from_secs(5),
        service.stop_session_job_with_grace(&session_key, "job-1", Some(Duration::from_millis(50))),
    )
    .await;
    let result = stopped
        .unwrap_or_else(|_| {
            panic!(
                "job runner did not settle: {:?}",
                service.session_job(&session_key, "job-1")
            )
        })
        .expect("owned job reaches a terminal state");
    assert_eq!(result.state, JobState::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the requested grace overrides the default two seconds: {:?}",
        started.elapsed()
    );
    assert!(
        !marker.exists(),
        "the command was stopped before its final side effect"
    );
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
        Arc::new(std::sync::RwLock::new(None)),
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
    assert!(
        text.contains("before"),
        "the detached answer retains output produced before Ctrl-G: {text}"
    );
    let job = jobs.get("job-1").expect("a job was registered");
    assert_eq!(job.state, JobState::Running, "the process is still running");

    // And it can be read and stopped, which is what a job is for.
    let mut terminal = jobs
        .terminal_for_session(session, "job-1")
        .expect("the session owns the running job");
    assert!(jobs.cancel("job-1"), "the job can be stopped");
    assert_eq!(jobs.get("job-1").expect("job").state, JobState::Cancelled);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !*terminal.borrow() {
            terminal.changed().await.expect("runner remains registered");
        }
    })
    .await
    .expect("the detached runner reports its terminal result");
    let job = jobs.get("job-1").expect("job");
    assert_eq!(job.state, JobState::Cancelled);
    assert!(
        job.exit_code.is_none(),
        "a cancelled process exit is not its natural exit status"
    );
    assert!(
        !later.exists(),
        "the stopped process never reached its marker"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_ctrl_g_detached_job_reports_its_final_exit_code() {
    let dir = std::env::temp_dir().join(format!("norte-detach-status-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let marker = dir.join("started");
    let jobs = Arc::new(JobRegistry::default());
    let steering = Arc::new(Steering::default());
    let detaches = Arc::new(DetachRegistry::default());
    let declaration = ShellCalls::declaration(
        ShellTool::new().into_declaration(),
        Arc::clone(&jobs),
        Arc::clone(&steering),
        Arc::clone(&detaches),
        Arc::new(std::sync::RwLock::new(None)),
    );

    let session = SessionId::new();
    let watched = marker.clone();
    let registry = Arc::clone(&detaches);
    tokio::spawn(async move {
        for _ in 0..200 {
            if watched.exists() {
                assert!(
                    registry.detach(session),
                    "there is a foreground call to detach"
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("shell command did not start");
    });

    let script = format!("echo before; touch {}; sleep 0.2; exit 7", marker.display());
    let request: ShellRequest =
        serde_json::from_value(serde_json::json!({ "script": script })).expect("request");
    let context = ToolContext::new()
        .with_session(session)
        .with_call(CallId::new());
    let answer = declaration
        .executor
        .call_with_context(&context, request)
        .await
        .expect("Ctrl-G returns a job answer");
    let answer_text: String = answer
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect();
    assert!(answer_text.contains("before"));
    assert!(answer_text.contains("job-1"));

    let mut terminal = jobs
        .terminal_for_session(session, "job-1")
        .expect("the session owns the detached job");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !*terminal.borrow() {
            terminal.changed().await.expect("runner remains registered");
        }
    })
    .await
    .expect("the job reports the runner's terminal result");
    let job = jobs.get("job-1").expect("job");
    assert_eq!(job.state, JobState::Finished);
    assert_eq!(job.exit_code, Some(7));
    assert!(
        steering.is_pending(session, "job:job-1"),
        "the completed result is delivered to the session"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_process_that_ends_after_the_detach_reports_once() {
    // The two ends can arrive in either order: the process may end before the
    // call has returned, and then the report would be sent with no job to send
    // it from: or sent twice, once from each end.
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
        Arc::new(std::sync::RwLock::new(None)),
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
    assert_eq!(jobs.get("job-1").expect("job").exit_code, Some(0));
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
        Arc::new(std::sync::RwLock::new(None)),
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
    assert_eq!(jobs.get("job-1").expect("job").exit_code, Some(0));
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
    let jobs = Arc::new(JobRegistry::default());
    let declaration = ShellCalls::declaration(
        ShellTool::new().into_declaration(),
        Arc::clone(&jobs),
        Arc::new(Steering::default()),
        Arc::clone(&detaches),
        Arc::new(std::sync::RwLock::new(None)),
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
    let detached_text: String = output
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect();
    assert!(detached_text.contains("before"));
    assert!(detached_text.contains("job-1"));

    // The process outlived the call: it writes its second marker a second
    // after the call returned, which is exactly what cancelling would prevent.
    for _ in 0..50 {
        if later.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(later.exists(), "the process kept running after the detach");
    let job = jobs
        .get("job-1")
        .expect("the detached job remains registered");
    assert_eq!(job.state, JobState::Finished);
    assert_eq!(job.exit_code, Some(0));

    // A later detach finds nothing: the token was released with the call.
    assert!(!detaches.detach(session), "no call is waiting now");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_finished_job_keeps_the_time_its_end_recorded() {
    // It reported the time since it *started*, so a job that took a second read
    // as two, then three: the figure grew for as long as anyone looked at it.
    let (registry, _, _, id) = registry_with_job();
    std::thread::sleep(std::time::Duration::from_millis(20));
    registry.settle(&id, Some(0), false);
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
    // bring it back: the record was the thing that was wrong.
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;

    let before = drain(
        &service,
        ClientRequestKind::Resume {
            session: key.clone(),
            replay: true,
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
            replay: true,
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
    assert!(find("read").allowed);
    assert_eq!(find("read").scope, RuleScope::Global, "from the config");
    assert!(!find("write").allowed);
    assert_eq!(find("write").scope, RuleScope::Global);
    assert!(find("shell").allowed);
    assert_eq!(
        find("shell").scope,
        RuleScope::Session,
        "a rule of this session only"
    );
    // A tool nobody wrote a rule for runs, which is what the default scope says
    // and what the panel prints.
    assert!(find("edit").allowed);
    assert_eq!(find("edit").scope, RuleScope::Default);
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
    assert!(command.allowed);
    assert_eq!(command.scope, RuleScope::Session);

    let in_second = service.tool_rules(Some(&second));
    let command = in_second
        .iter()
        .find(|status| status.name == "shell")
        .expect("command is registered");
    assert!(
        command.allowed,
        "the other session never had that rule, so the default applies"
    );
    assert_eq!(command.scope, RuleScope::Default);
}

/// A service over a given configuration, for the tests that need one.
fn service_with(config: Config) -> Arc<Service> {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_deltas(["Hel", "lo"]));
    Arc::new(
        Service::new(
            config,
            catalog(),
            Some(spec("scripted")),
            Arc::new(ScriptedFactory { model }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    )
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
    assert!(!write.allowed);
    assert_eq!(write.scope, RuleScope::Global);
}

#[tokio::test]
async fn the_shell_is_a_tool_and_no_rule_names_it() {
    // With no rule, a call runs: the gate refuses what somebody wrote down, and
    // nothing else. What the shell may do is the workspace confine's business,
    // and this is the list a person reads before writing the rule.
    let statuses = service().tool_rules(None);
    let shell = statuses
        .iter()
        .find(|status| status.name == "shell")
        .expect("the shell is registered");
    assert!(shell.allowed);
    assert_eq!(shell.scope, RuleScope::Default);
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
    // because the scripted model declares up to high: the engine checks the
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
            output: Vec::new(),
            response_format: None,
            metadata: None,
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

#[tokio::test]
async fn a_session_keeps_the_tools_it_was_made_with() {
    // The decision this pins: the tool declarations are part of the prompt
    // prefix, so a set that changed under a live session would invalidate the
    // provider's cached prefix and re-bill the whole conversation. A tool
    // disabled after a session exists does not reach back into it, and a tool
    // enabled after it does not reach forward into it either. The configuration
    // decides what the *next* session gets and nothing else.
    let mut config = config();
    config.tools.disabled = vec!["grep".to_owned()];
    let service = service_with(config);

    // A session made while `grep` was withheld does not have it.
    let first = service.start_session(None).await.expect("session");
    let names = |key: Option<&SessionKey>| {
        service
            .tools(key)
            .expect("registry")
            .specs()
            .iter()
            .map(|spec| spec.name.to_string())
            .collect::<Vec<_>>()
    };
    let made_without = names(Some(&first));
    assert!(
        !made_without.iter().any(|name| name == "grep"),
        "the tool was withheld when the session was made: {made_without:?}"
    );

    // Re-enabling it does not reach the session that already exists.
    let mut next = service.config();
    next.tools.disabled.clear();
    service.replace_config(next);
    let still_without = names(Some(&first));
    assert!(
        !still_without.iter().any(|name| name == "grep"),
        "the session keeps the set it was made with: {still_without:?}"
    );

    // A session made now does have it: the configuration decides what the next
    // one gets.
    let second = service.start_session(None).await.expect("session");
    let made_with = names(Some(&second));
    assert!(
        made_with.iter().any(|name| name == "grep"),
        "a session made after the change gets the tool: {made_with:?}"
    );
}

#[tokio::test]
async fn a_withheld_tool_is_not_registered_at_all() {
    // Not registered means not in the schema the model is shown and not in the
    // registry a call is looked up in: the model cannot name it, and a call
    // that arrived anyway would find nothing to run.
    let mut config = config();
    config.tools.disabled = vec!["todo".to_owned(), "read".to_owned()];
    let service = service_with(config);
    let registry = service.tools(None).expect("registry");
    let names: Vec<String> = registry
        .specs()
        .iter()
        .map(|spec| spec.name.to_string())
        .collect();
    assert!(
        !names.iter().any(|name| name == "todo" || name == "read"),
        "a withheld tool is absent from the registry: {names:?}"
    );
    // And the ones nobody named are still there.
    assert!(names.iter().any(|name| name == "write"), "{names:?}");
}

/// A scripted model that answers one text per turn, for a test that needs its
/// own config rather than the shared one.
#[tokio::test]
async fn a_session_whose_tools_were_taken_away_still_runs_its_turns() {
    // The rule the configuration promises: the list bites when a session is
    // *made*, and a session that already exists keeps working. Its history can
    // hold calls to a tool that no longer exists anywhere: the conversation is
    // messages the engine replays, not a lookup in the registry: so a turn
    // after the change runs exactly as it did before it.
    let mut before = config();
    before.tools.disabled = vec!["todo".to_owned()];
    let service = service_with(before);
    let key = service.start_session(None).await.expect("session");
    assert!(
        !service
            .tools(Some(&key))
            .expect("registry")
            .specs()
            .iter()
            .any(|spec| spec.name.as_str() == "todo"),
        "the session was made without it"
    );

    // The tool comes back in the configuration: the session keeps what it had.
    let mut after = service.config();
    after.tools.disabled.clear();
    service.replace_config(after);

    // And a turn in that session still runs to a committed answer.
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: Some(key.clone()),
            text: "still there?".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    let mut text = String::new();
    let mut committed = false;
    for event in &events {
        match &event.kind {
            ServerEventKind::Delta {
                delta: Delta::Text { text: chunk, .. },
                ..
            } => text.push_str(chunk),
            ServerEventKind::TurnFinished {
                committed: done, ..
            } => committed = *done,
            _ => {}
        }
    }
    assert_eq!(
        text, "Hello",
        "the turn ran in a session made without `todo`"
    );
    assert!(committed);
    // The session is still the same one and still in the list.
    assert!(
        service.sessions().iter().any(|info| info.session == key),
        "the session survived"
    );
}

#[tokio::test]
async fn the_request_summary_follows_the_answer_it_describes() {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("the answer"));
    let (service, _store) = persistent_service(model).await;
    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "hello".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;

    let last_text = events
        .iter()
        .rposition(|event| {
            matches!(
                &event.kind,
                ServerEventKind::Delta {
                    delta: Delta::Text { text, .. },
                    ..
                } if !text.is_empty()
            )
        })
        .expect("the answer streams");
    let summary = events
        .iter()
        .position(|event| matches!(event.kind, ServerEventKind::RequestReconstructed { .. }))
        .expect("the request is summarised");
    assert!(
        last_text < summary,
        "the summary belongs under the answer, not above it: text {last_text}, summary {summary}"
    );
}

#[test]
fn child_session_result_carries_validated_structured_output() {
    let outcome = super::subagents::SubagentOutcome {
        text: r#"{"answer":42}"#.to_owned(),
        is_error: false,
        child: None,
        output_schema_valid: Some(true),
    };

    let result = super::subagents::child_result(&outcome);

    assert_eq!(result["status"], "completed");
    assert_eq!(result["output"], serde_json::json!({"answer": 42}));
    assert_eq!(result["output_schema_valid"], true);
    assert_eq!(result["summary"], r#"{"answer":42}"#);
}

#[tokio::test]
async fn a_parent_delegates_one_turn_and_gets_the_child_answer() {
    // The basic shape: the parent model calls `spawn_agent` once, the child
    // runs one turn and answers, and the parent receives the child's text.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let call = CallId::new();
    let model = Arc::new(
        ScriptedModel::new(reference)
            // Turn 1 (parent): call spawn_agent.
            .turn(vec![
                norte::StreamEvent::ToolCallStart {
                    index: 0,
                    call_id: call,
                    name: "spawn_agent".to_owned(),
                },
                norte::StreamEvent::ToolCallDelta {
                    index: 0,
                    call_id: call,
                    name: None,
                    arguments: serde_json::json!({
                        "task": "What is 2 + 2?"
                    })
                    .to_string(),
                },
                norte::StreamEvent::Finish(norte::FinishReason::ToolCalls),
            ])
            // Turn 2 (child): answer.
            .turn_text("4")
            // Turn 3 (parent): finish.
            .turn_text("The child said 4."),
    );
    let config = subagent_config();
    let service = Arc::new(
        Service::new(
            config,
            catalog(),
            Some(spec("scripted")),
            Arc::new(ScriptedFactory {
                model: Arc::clone(&model) as Arc<dyn Model>,
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    service.install_subagent_runner();

    let events = drain(
        &service,
        ClientRequestKind::SendMessage {
            session: None,
            text: "delegate".to_owned(),
            attachments: Vec::new(),
            budget: None,
            reasoning: None,
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    )
    .await;
    // The parent's finished message carries the child's answer: the scripted
    // model consumed the turns in order (parent tool call, child answer,
    // parent finish), so the child's "4" reached the parent as the result.
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                committed: true,
                ..
            }
        )),
        "the turn finished with the child's answer: {events:?}"
    );
    let parent = events
        .iter()
        .find_map(|event| event.session.clone())
        .expect("parent session");
    let children = service.children_of(&parent);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].state, super::ChildSessionState::Idle);
}

#[tokio::test]
async fn a_detached_subagent_is_a_job_that_reports_and_can_be_stopped() {
    // With `detach: true` the tool returns while the child is still running,
    // then the job retains the child's result.
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("unused"));
    let service = Arc::new(
        Service::new(
            subagent_config(),
            catalog(),
            Some(spec("scripted")),
            Arc::new(ScriptedFactory {
                model: model as Arc<dyn Model>,
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let parent = SessionId::new();
    let call_id = CallId::new();
    let tool = super::subagents::SubagentTool::new(
        Arc::new(DelayedSubagentRunner),
        Arc::clone(&service.jobs),
        Arc::clone(&service.steering),
        Arc::clone(&service.detaches),
        Arc::new(std::sync::RwLock::new(None)),
        3,
    );
    let context = ToolContext::new().with_session(parent).with_call(call_id);
    let _result = tool
        .call_with_context(
            &context,
            super::subagents::SubagentArgs {
                prompt: "slow task".to_owned(),
                title: None,
                detach: true,
                output_schema: None,
                lifetime: "turn".to_owned(),
                agent_id: None,
                context_mode: None,
            },
        )
        .await
        .expect("detached spawn result");

    let jobs = service.jobs.list(Some(parent));
    assert_eq!(jobs.len(), 1, "one subagent job: {jobs:?}");
    assert_eq!(
        jobs[0].state,
        JobState::Running,
        "the tool returned promptly"
    );
    assert!(
        jobs[0].label.contains("subagent"),
        "the label names it a subagent: {:?}",
        jobs[0]
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if service
                .jobs
                .list(Some(parent))
                .first()
                .is_some_and(|job| job.state == JobState::Finished)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("detached child eventually finishes");
    let jobs = service.jobs.list(Some(parent));
    assert_eq!(jobs.len(), 1, "one subagent job: {jobs:?}");
    assert_eq!(jobs[0].state, JobState::Finished, "it finished: {jobs:?}");
    assert!(
        jobs[0].output.contains("child done"),
        "the job output holds the child's answer: {:?}",
        jobs[0]
    );
    // A finished job cannot be stopped again.
    assert!(
        !service.jobs.cancel(&jobs[0].id),
        "a finished job is not cancellable"
    );
}

#[tokio::test]
async fn too_many_subagents_is_an_error_the_model_reads() {
    // The cap is a semaphore, not a queue: past it, the call is an error
    // the model reads rather than a run queued behind an unbounded fan-out.
    use super::subagents::SubagentTool;
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("slow"));
    let service = Arc::new(
        Service::new(
            subagent_config(),
            catalog(),
            Some(spec("scripted")),
            Arc::new(ScriptedFactory {
                model: Arc::clone(&model) as Arc<dyn Model>,
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    service.install_subagent_runner();
    let runner = service
        .subagent_runner
        .read()
        .unwrap()
        .clone()
        .expect("runner");
    let tool = SubagentTool::new(
        runner,
        Arc::clone(&service.jobs),
        Arc::clone(&service.steering),
        Arc::clone(&service.detaches),
        Arc::new(std::sync::RwLock::new(None)),
        1,
    );
    // Take the one slot.
    assert!(tool.gate_for_test().acquire().is_ok());
    // The next one is an error, not a queue.
    let error = tool.gate_for_test().acquire().expect_err("capped");
    assert!(
        error.contains("too many subagents"),
        "the error names the cap: {error}"
    );
}

async fn service_with_child_session() -> (Arc<Service>, SessionKey, SessionKey) {
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
    let mut child_config = subagent_config();
    child_config.system_prompt = Some("child instructions".to_owned());
    child_config.tools.disabled = vec!["write".to_owned(), "todo".to_owned()];
    let service = Arc::new(
        Service::new(
            child_config,
            catalog(),
            Some(spec("scripted")),
            Arc::new(ScriptedFactory {
                model: Arc::clone(&model) as Arc<dyn Model>,
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    service.install_subagent_runner();

    let parent = service.create_session(None).await.expect("parent session");
    let (child_agent_id, child_spec, child_disabled, child_budget, child_system_prompt) = service
        .subagent_settings(&parent, None)
        .await
        .expect("child settings");
    let child = service
        .create_session_child(
            super::session_record::ChildSessionMetadata {
                parent: parent.clone(),
                tool_call_id: "call-1".to_owned(),
                task_summary: "test task".to_owned(),
                lifetime: "turn".to_owned(),
                agent_profile: child_agent_id,
                system_prompt: child_system_prompt,
                cancel: Arc::new(norte::Cancellation::new()),
            },
            child_spec,
            child_disabled,
            child_budget,
        )
        .await
        .expect("child session");
    (service, parent, child)
}

#[tokio::test]
async fn a_child_session_never_sees_the_subagent_tool() {
    let (service, parent, child) = service_with_child_session().await;
    let parent_budget = service.budget_book(&parent).await.expect("parent budget");
    let child_budget = service.budget_book(&child).await.expect("child budget");
    assert!(
        Arc::ptr_eq(&parent_budget, &child_budget),
        "child turns use the parent's budget ledger"
    );
    let child_model = service.session_spec(&child).expect("child model");
    assert_eq!(child_model.provider, "test");
    assert_eq!(child_model.model, "scripted");
    let (child_id, child_spec, _) = service.require_record(&child).expect("child record");
    let child_history = service
        .engine(&child_spec.expect("child spec"), Some(&child))
        .expect("child engine")
        .history(child_id)
        .await
        .expect("child history");
    assert_eq!(
        child_history.first().map(norte::Message::text),
        Some("child instructions".to_owned())
    );

    let parent_tools = service.tools(Some(&parent)).expect("parent tools").specs();
    let parent_names: Vec<&str> = parent_tools.iter().map(|tool| tool.name.as_str()).collect();
    assert!(
        parent_names.contains(&"spawn_agent"),
        "the parent has spawn_agent: {parent_names:?}"
    );
    assert!(
        !parent_names.contains(&"subagent"),
        "the internal subagent tool is gone: {parent_names:?}"
    );

    let child_tools = service.tools(Some(&child)).expect("child tools").specs();
    let child_names: Vec<&str> = child_tools.iter().map(|tool| tool.name.as_str()).collect();
    assert!(
        !child_names.contains(&"subagent"),
        "the child never has subagent: {child_names:?}"
    );
    assert!(
        child_names.contains(&"spawn_agent"),
        "the child keeps spawn_agent, which enforces the depth: {child_names:?}"
    );
    assert!(
        !child_names.contains(&"ask_user"),
        "the child never has ask_user: {child_names:?}"
    );
    assert!(
        !child_names.contains(&"browser_use"),
        "the child never has browser_use: {child_names:?}"
    );
    assert!(
        child_names.contains(&"read"),
        "allow keeps read: {child_names:?}"
    );
    assert!(
        !child_names.contains(&"write"),
        "allow withholds write: {child_names:?}"
    );
    assert!(
        !child_names.contains(&"todo"),
        "deny withholds todo: {child_names:?}"
    );
}

#[tokio::test]
async fn suspended_child_message_queue_is_bounded() {
    let (service, _, child) = service_with_child_session().await;
    service
        .set_delegator_suspended(&child, true)
        .await
        .expect("suspend child");
    for index in 0..20 {
        service
            .hold_delegator_message(&child, format!("held message {index}"))
            .await
            .expect("message is within the queue limit");
    }
    assert!(matches!(
        service
            .hold_delegator_message(&child, "one too many".to_owned())
            .await,
        Err(ServiceError::Delegation(message)) if message.contains("at most 20")
    ));
}

#[tokio::test]
async fn child_registry_restores_persisted_delegation_state() {
    let (service, parent, child) = service_with_child_session().await;
    service
        .set_delegator_suspended(&child, true)
        .await
        .expect("suspend child");
    service
        .hold_delegator_message(&child, "held".to_owned())
        .await
        .expect("hold message");

    service.restore_child_registry();

    let restored = service
        .children
        .get(&parent, &child)
        .expect("restored child");
    assert!(restored.suspended);
    assert_eq!(restored.held, ["held"]);
    assert_eq!(restored.tool_call_id, "call-1");
}

/// A plugin's catalogue is as real as the files': a pair only the plugin
/// offers is resolved, and an empty file catalogue is not a hard stop.
#[tokio::test]
async fn a_plugin_offered_model_resolves_without_the_files() {
    let plugin_providers: norte_code_wire::catalog::Providers = serde_json::from_str(
        r#"{
            "providers": {
                "plugin": {
                    "name": "Plugin",
                    "environment_keys": ["PATH"],
                    "wires": [
                        {"api": "https://plugin.invalid/v1", "schema": "openai"}
                    ]
                }
            }
        }"#,
    )
    .expect("plugin providers");
    let plugin_models: norte_code_wire::catalog::Models = serde_json::from_str(
        r#"{
            "models": [
                {
                    "id": "injected",
                    "provider_id": "plugin",
                    "name": "Injected",
                    "supported_params": ["tools"],
                    "modalities": {"input": ["text"], "output": ["text"]},
                    "limit": {"context": 4000, "output": 400}
                }
            ]
        }"#,
    )
    .expect("plugin models");

    let empty = Arc::new(Catalog::default());
    let service = Arc::new(
        Service::new(
            config(),
            empty,
            None,
            Arc::new(ScriptedFactory {
                model: Arc::new(ScriptedModel::new(ModelRef::new(
                    ProviderId::new("test").expect("provider"),
                    ModelId::new("scripted").expect("model"),
                ))),
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    service.set_plugin_catalogue(plugin_providers, plugin_models);

    let key = service.create_session(None).await.expect("session");
    let events = drain(
        &service,
        ClientRequestKind::SetModel {
            session: key,
            provider: "plugin".to_owned(),
            model: "injected".to_owned(),
        },
    )
    .await;

    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            ServerEventKind::ModelChanged { provider, model }
                if provider == "plugin" && model == "injected"
        )),
        "a plugin-provided model must resolve: {events:?}"
    );
}

/// With no provider from either source, the error says what to do and does
/// not blame a file the plugin could have replaced.
#[tokio::test]
async fn an_empty_catalogue_names_the_setup_step() {
    let service = Arc::new(
        Service::new(
            config(),
            Arc::new(Catalog::default()),
            None,
            Arc::new(ScriptedFactory {
                model: Arc::new(ScriptedModel::new(ModelRef::new(
                    ProviderId::new("test").expect("provider"),
                    ModelId::new("scripted").expect("model"),
                ))),
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );
    let error = service
        .default_spec_for_test("missing", "whatever")
        .expect_err("no provider");
    let text = error.to_string();
    assert!(text.contains("providers.json"), "{text}");
    assert!(text.contains("plugin"), "{text}");
}

/// A rewind that removes a turn containing a tool call proceeds instead of
/// refusing forever: no tool declares its side effects yet, so the old rule
/// blocked every rewind that crossed a call and nothing was ever reported.
#[tokio::test]
async fn a_rewind_that_crosses_a_tool_call_succeeds() {
    let (service, _) = scripted_service(spec("scripted"), &["answer one", "answer two"]);
    let created = drain(&service, ClientRequestKind::CreateSession { title: None }).await;
    let key = created[0].session.clone().expect("session");
    for text in ["first", "second"] {
        drain(
            &service,
            ClientRequestKind::SendMessage {
                session: Some(key.clone()),
                text: text.to_owned(),
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                output: Vec::new(),
                response_format: None,
                metadata: None,
                replay: false,
            },
        )
        .await;
    }

    // Archive a tool call in the removed range, the way a real turn records
    // one, and confirm the rewind is allowed to run over it.
    {
        let mut sessions = service.lock_sessions();
        let record = sessions.get_mut(&key).expect("record");
        record.rewound_history.push((
            1,
            norte_code_wire::MessageRole::Tool,
            norte_code_wire::ReplayedKind::ToolCall,
            "shell {\"command\":\"echo hi\"}".to_owned(),
            0,
        ));
    }

    let rewound = service
        .rewind_session(&key, "turn_1", true, false, false)
        .await
        .expect("a tool call must not block the rewind");
    assert_eq!(rewound, ["turn_1", "turn_2"]);

    let calls = service.rewound_tool_calls(&key, &rewound);
    assert!(
        calls.iter().any(|call| call.tool_name == "shell"),
        "the removed tool call is reported: {calls:?}"
    );
}
