use super::*;
use futures_util::StreamExt;

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
