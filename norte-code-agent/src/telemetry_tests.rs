use super::*;
use futures_util::StreamExt;
use norte_code_protocol::config::ProviderHttpConfig;

struct Next;

#[async_trait::async_trait]
impl Continuation for Next {
    async fn call(
        &self,
        _context: &LayerContext,
        _request: ModelRequest,
    ) -> norte::Result<ModelStream> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
}

#[test]
fn counters_start_at_zero() {
    let counters = EngineCounters::default();
    assert_eq!(counters.snapshot()["requests"], 0);
}

#[test]
fn start_events_are_output_but_usage_is_not() {
    assert!(starts_output(&StreamEvent::TextDelta {
        index: 0,
        text: "hi".to_owned()
    }));
    assert!(!starts_output(&StreamEvent::Usage(Usage::default())));
    let thinking = StreamEvent::ReasoningDelta {
        index: 0,
        text: "reasoning".to_owned(),
    };
    assert!(starts_output(&thinking));
    assert!(!starts_visible_text(&thinking));
}

#[tokio::test]
async fn an_exhausted_stream_records_an_ok_row() {
    let telemetry = Arc::new(Telemetry::default());
    let spec = ModelSpec {
        provider: "test".to_owned(),
        model: "test".to_owned(),
        endpoint: "https://example.invalid/v1".to_owned(),
        schema: norte_code_wire::catalog::WireSchema::OpenAI,
        auth: None,
        api_key: norte_code_protocol::config::ApiKey::new("test"),
        capabilities: norte::Capabilities::builder().build(),
        headers: std::collections::BTreeMap::new(),
        http: ProviderHttpConfig::default(),
    };
    let layer = TelemetryLayer::new(Arc::clone(&telemetry), spec, true, false);
    let request = ModelRequest {
        model: norte::ModelRef::new(
            norte::ProviderId::new("test").expect("provider"),
            norte::ModelId::new("test").expect("model"),
        ),
        messages: Vec::new(),
        tools: Vec::new(),
        reasoning: ReasoningRequest::Automatic,
        params: norte::GenerationParams::default(),
        limits: norte::StreamLimits::default(),
        cache_boundaries: Vec::new(),
        extensions: std::collections::BTreeMap::new(),
    };
    let context = LayerContext {
        session: norte::SessionId::new(),
        turn: 1,
    };
    let mut stream = layer.wrap(&context, request, &Next).await.expect("stream");
    assert!(stream.next().await.is_none());
    let records = telemetry.take(context.session);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, RequestStatus::Ok);
    assert!(records[0].sandbox);
    assert!(!records[0].sandbox_network);
}

struct OneEventThenPending(StreamEvent);

#[async_trait::async_trait]
impl Continuation for OneEventThenPending {
    async fn call(
        &self,
        _context: &LayerContext,
        _request: ModelRequest,
    ) -> norte::Result<ModelStream> {
        let first = self.0.clone();
        Ok(Box::pin(
            futures_util::stream::once(async move { Ok(first) })
                .chain(futures_util::stream::pending()),
        ))
    }
}

struct Events(Vec<StreamEvent>);

#[async_trait::async_trait]
impl Continuation for Events {
    async fn call(
        &self,
        _context: &LayerContext,
        _request: ModelRequest,
    ) -> norte::Result<ModelStream> {
        Ok(Box::pin(futures_util::stream::iter(
            self.0.iter().cloned().map(Ok).collect::<Vec<_>>(),
        )))
    }
}

#[tokio::test]
async fn records_first_output_and_first_visible_text_separately() {
    let telemetry = Arc::new(Telemetry::default());
    let spec = ModelSpec {
        provider: "test".to_owned(),
        model: "test".to_owned(),
        endpoint: "https://example.invalid/v1".to_owned(),
        schema: norte_code_wire::catalog::WireSchema::OpenAI,
        auth: None,
        api_key: norte_code_protocol::config::ApiKey::new("test"),
        capabilities: norte::Capabilities::builder().build(),
        headers: std::collections::BTreeMap::new(),
        http: ProviderHttpConfig::default(),
    };
    let layer = TelemetryLayer::new(Arc::clone(&telemetry), spec, false, false);
    let request = ModelRequest {
        model: norte::ModelRef::new(
            norte::ProviderId::new("test").expect("provider"),
            norte::ModelId::new("test").expect("model"),
        ),
        messages: Vec::new(),
        tools: Vec::new(),
        reasoning: ReasoningRequest::Automatic,
        params: norte::GenerationParams::default(),
        limits: norte::StreamLimits::default(),
        cache_boundaries: Vec::new(),
        extensions: std::collections::BTreeMap::new(),
    };
    let context = LayerContext {
        session: norte::SessionId::new(),
        turn: 1,
    };
    let events = vec![
        StreamEvent::ReasoningDelta {
            index: 0,
            text: "thinking".to_owned(),
        },
        StreamEvent::TextDelta {
            index: 1,
            text: "visible".to_owned(),
        },
        StreamEvent::ResponseMetadata {
            http_status: Some(200),
            retry_count: Some(1),
            resolved_model: Some("test-served".to_owned()),
        },
        StreamEvent::Finish(norte::FinishReason::Stop),
    ];
    let mut stream = layer
        .wrap(&context, request, &Events(events))
        .await
        .expect("stream");
    while stream.next().await.is_some() {}
    let record = telemetry.take(context.session).pop().expect("record");
    assert!(record.first_token_ms.is_some());
    assert!(record.first_text_ms >= record.first_token_ms);
    assert_eq!(record.stream_chunk_count, Some(2));
    assert!(record.max_inter_chunk_gap_ms.is_some());
    assert_eq!(record.http_status, Some(200));
    assert_eq!(record.retry_count, Some(1));
    assert_eq!(record.resolved_model.as_deref(), Some("test-served"));
}

async fn cancelled_row(
    event: StreamEvent,
) -> (norte_code_protocol::sqlite::RequestRecord, norte::SessionId) {
    let telemetry = Arc::new(Telemetry::default());
    let spec = ModelSpec {
        provider: "test".to_owned(),
        model: "test".to_owned(),
        endpoint: "https://example.invalid/v1".to_owned(),
        schema: norte_code_wire::catalog::WireSchema::OpenAI,
        auth: None,
        api_key: norte_code_protocol::config::ApiKey::new("test"),
        capabilities: norte::Capabilities::builder().build(),
        headers: std::collections::BTreeMap::new(),
        http: ProviderHttpConfig::default(),
    };
    let layer = TelemetryLayer::new(Arc::clone(&telemetry), spec, false, false);
    let request = ModelRequest {
        model: norte::ModelRef::new(
            norte::ProviderId::new("test").expect("provider"),
            norte::ModelId::new("test").expect("model"),
        ),
        messages: Vec::new(),
        tools: Vec::new(),
        reasoning: ReasoningRequest::Automatic,
        params: norte::GenerationParams::default(),
        limits: norte::StreamLimits::default(),
        cache_boundaries: Vec::new(),
        extensions: std::collections::BTreeMap::new(),
    };
    let context = LayerContext {
        session: norte::SessionId::new(),
        turn: 1,
    };
    let mut stream = layer
        .wrap(&context, request, &OneEventThenPending(event))
        .await
        .expect("stream");
    assert!(stream.next().await.is_some());
    drop(stream);
    let records = telemetry.take(context.session);
    assert_eq!(records.len(), 1);
    (records.into_iter().next().expect("record"), context.session)
}

#[tokio::test]
async fn a_cancel_before_usage_keeps_the_row_without_a_cost() {
    let (record, _) = cancelled_row(StreamEvent::TextDelta {
        index: 0,
        text: "thinking".to_owned(),
    })
    .await;
    assert_eq!(record.status, RequestStatus::Cancelled);
    assert_eq!(record.input_tokens, None);
    assert_eq!(record.output_tokens, None);
    assert_eq!(record.cost_usd, None);
}

#[tokio::test]
async fn a_cancel_after_usage_keeps_the_provider_cost() {
    let mut usage = Usage::default();
    usage.input_tokens = Some(120);
    usage.output_tokens = Some(30);
    usage
        .vendor
        .insert("cost".to_owned(), serde_json::json!(0.25));
    let (record, _) = cancelled_row(StreamEvent::Usage(usage)).await;
    assert_eq!(record.status, RequestStatus::Cancelled);
    assert_eq!(record.input_tokens, Some(120));
    assert_eq!(record.output_tokens, Some(30));
    assert_eq!(record.cost_usd, Some(0.25));
    assert!(record.metadata.get("usage").is_some());
}

fn request_record(id: &str, north_session: &str) -> RequestRecord {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "session": "logical",
        "north_session": north_session,
        "turn": 1,
        "provider": "test",
        "model": "test",
        "endpoint": null,
        "started_at": "2026-11-20T00:00:00Z",
        "duration_ms": 1,
        "cache_hit": false,
        "tool_calls": [],
        "params": {},
        "extensions": {},
        "metadata": {},
        "status": "ok"
    }))
    .expect("request record")
}

#[test]
fn the_ring_forgets_the_oldest_record_when_it_is_full() {
    let telemetry = Telemetry::default();
    for index in 0..=MAX_TELEMETRY_RECORDS {
        telemetry.push(request_record(&index.to_string(), "session"));
    }

    let session = norte::SessionId::new();
    telemetry.push(request_record("last", &session.to_string()));

    let records = telemetry.take(session);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, "last");
    assert_eq!(
        telemetry.records.lock().expect("records").len(),
        MAX_TELEMETRY_RECORDS - 1
    );
}

#[test]
fn clearing_telemetry_removes_records_and_resets_counters() {
    let telemetry = Telemetry::default();
    telemetry.push(request_record("row", "session"));
    telemetry
        .counters
        .requests
        .store(4, std::sync::atomic::Ordering::Relaxed);

    telemetry.clear();

    assert!(telemetry.records.lock().expect("records").is_empty());
    assert_eq!(telemetry.counters.snapshot()["requests"], 0);
}
