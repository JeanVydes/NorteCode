//! Local request telemetry.
//!
//! Norte's observers see every engine event but deliberately do not carry the
//! provider's usage payload. The request row therefore comes from a layer that
//! wraps the model stream: it sees the exact request, every stream event
//! (including `Usage`) and the moment the stream ends. An observer records the
//! engine-event counters beside it, which is useful when a provider never
//! reports usage.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Instant;

use chrono::{DateTime, Utc};
use futures_core::Stream;
use norte::{
    Continuation, EngineEvent, Layer, LayerContext, ModelRequest, ModelStream, Observer,
    ReasoningRequest, StreamEvent, Usage,
};
use norte_code_store_sqlite::RequestRecord;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::provider::ModelSpec;

/// Engine-event counters that an observer can update without blocking.
#[derive(Debug, Default)]
pub struct EngineCounters {
    requests: AtomicU64,
    stream_deltas: AtomicU64,
    tool_calls: AtomicU64,
    commits: AtomicU64,
    errors: AtomicU64,
}

impl EngineCounters {
    /// A snapshot for diagnostics.
    #[must_use]
    pub fn snapshot(&self) -> Value {
        json!({
            "requests": self.requests.load(Ordering::Relaxed),
            "stream_deltas": self.stream_deltas.load(Ordering::Relaxed),
            "tool_calls": self.tool_calls.load(Ordering::Relaxed),
            "commits": self.commits.load(Ordering::Relaxed),
            "errors": self.errors.load(Ordering::Relaxed),
        })
    }
}

/// Shared request records and engine counters.
#[derive(Debug, Default)]
pub struct Telemetry {
    records: Mutex<Vec<RequestRecord>>,
    counters: EngineCounters,
}

impl Telemetry {
    /// Records one request.
    pub fn push(&self, record: RequestRecord) {
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(record);
    }

    /// Takes every completed request for a Norte session, oldest first.
    #[must_use]
    pub fn take(&self, north: norte::SessionId) -> Vec<RequestRecord> {
        let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        let mut taken = Vec::new();
        let mut kept = Vec::with_capacity(records.len());
        for record in records.drain(..) {
            if record.north_session == north.to_string() {
                taken.push(record);
            } else {
                kept.push(record);
            }
        }
        *records = kept;
        taken
    }

    /// The engine-event counters.
    #[must_use]
    pub fn counters(&self) -> &EngineCounters {
        &self.counters
    }

    /// A snapshot of the engine-event counters.
    #[must_use]
    pub fn counter_snapshot(&self) -> Value {
        self.counters.snapshot()
    }
}

/// Watches engine events without blocking the turn.
#[derive(Debug)]
pub struct TelemetryObserver {
    telemetry: Arc<Telemetry>,
}

impl TelemetryObserver {
    /// An observer that updates `telemetry`.
    #[must_use]
    pub fn new(telemetry: Arc<Telemetry>) -> Self {
        Self { telemetry }
    }
}

impl Observer for TelemetryObserver {
    fn name(&self) -> &'static str {
        "telemetry"
    }

    fn observe(&self, event: &EngineEvent) {
        let counters = self.telemetry.counters();
        match event {
            EngineEvent::RequestReconstructed { .. } => {
                counters.requests.fetch_add(1, Ordering::Relaxed);
            }
            EngineEvent::StreamDelta { .. } => {
                counters.stream_deltas.fetch_add(1, Ordering::Relaxed);
            }
            EngineEvent::ToolCallStarted { .. } => {
                counters.tool_calls.fetch_add(1, Ordering::Relaxed);
            }
            EngineEvent::CommitSucceeded { .. } => {
                counters.commits.fetch_add(1, Ordering::Relaxed);
            }
            EngineEvent::Error { .. } => {
                counters.errors.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

/// The layer that turns one model stream into one telemetry row.
#[derive(Debug)]
pub struct TelemetryLayer {
    telemetry: Arc<Telemetry>,
    spec: ModelSpec,
    sandbox: bool,
    sandbox_network: bool,
}

impl TelemetryLayer {
    /// A layer for `spec` that records into `telemetry`.
    #[must_use]
    pub fn new(
        telemetry: Arc<Telemetry>,
        spec: ModelSpec,
        sandbox: bool,
        sandbox_network: bool,
    ) -> Self {
        Self {
            telemetry,
            spec,
            sandbox,
            sandbox_network,
        }
    }
}

#[async_trait::async_trait]
impl Layer for TelemetryLayer {
    fn name(&self) -> &'static str {
        "telemetry"
    }

    async fn wrap(
        &self,
        context: &LayerContext<'_>,
        request: ModelRequest,
        next: &dyn Continuation,
    ) -> norte::Result<ModelStream> {
        let started_at = Utc::now();
        let started = Instant::now();
        let metadata =
            RequestMetadata::from_request(&request, &self.spec, self.sandbox, self.sandbox_network);
        match next.call(context, request).await {
            Ok(stream) => Ok(Box::pin(TelemetryStream {
                inner: stream,
                telemetry: Arc::clone(&self.telemetry),
                spec: self.spec.clone(),
                session: context.session,
                turn: context.turn,
                started,
                started_at,
                first_token: None,
                last_token: None,
                usage: None,
                finish_reason: None,
                tool_calls: Vec::new(),
                status: None,
                error: None,
                finished: false,
                metadata,
            })),
            Err(error) => {
                let mut record = metadata.record(
                    context.session,
                    context.turn,
                    &self.spec,
                    started_at,
                    started.elapsed(),
                    "error",
                );
                record.error = Some(error.to_string());
                self.telemetry.push(record);
                Err(error)
            }
        }
    }
}

struct RequestMetadata {
    reasoning: Option<String>,
    sandbox: bool,
    sandbox_network: bool,
    params: Value,
    extensions: Value,
    metadata: Value,
}

impl RequestMetadata {
    fn from_request(
        request: &ModelRequest,
        spec: &ModelSpec,
        sandbox: bool,
        sandbox_network: bool,
    ) -> Self {
        let bytes = serde_json::to_vec(&request.messages).map_or(0, |bytes| bytes.len());
        Self {
            reasoning: Some(reasoning_label(request.reasoning)),
            sandbox,
            sandbox_network,
            params: serde_json::to_value(&request.params).unwrap_or(Value::Null),
            extensions: serde_json::to_value(&request.extensions).unwrap_or(Value::Null),
            metadata: json!({
                "wire": format!("{:?}", spec.wire),
                "messages": request.messages.len(),
                "tool_declarations": request.tools.len(),
                "cache_boundaries": request.cache_boundaries.len(),
                "estimated_input_tokens": u64::try_from(bytes / 4).unwrap_or(0),
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        session: norte::SessionId,
        turn: u64,
        spec: &ModelSpec,
        started_at: DateTime<Utc>,
        elapsed: std::time::Duration,
        status: &str,
    ) -> RequestRecord {
        RequestRecord {
            id: Uuid::new_v4().to_string(),
            session: String::new(),
            north_session: session.to_string(),
            turn,
            provider: spec.provider.clone(),
            model: spec.model.clone(),
            started_at,
            duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            first_token_ms: None,
            last_token_ms: None,
            input_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            estimated_input_tokens: self
                .metadata
                .get("estimated_input_tokens")
                .and_then(Value::as_u64),
            cache_hit: false,
            tool_calls: Vec::new(),
            reasoning: self.reasoning.clone(),
            sandbox: self.sandbox,
            sandbox_network: self.sandbox_network,
            params: self.params.clone(),
            extensions: self.extensions.clone(),
            metadata: self.metadata.clone(),
            cost_usd: None,
            finish_reason: None,
            status: status.to_owned(),
            error: None,
        }
    }
}

struct TelemetryStream {
    inner: ModelStream,
    telemetry: Arc<Telemetry>,
    spec: ModelSpec,
    session: norte::SessionId,
    turn: u64,
    started: Instant,
    started_at: DateTime<Utc>,
    first_token: Option<Duration>,
    last_token: Option<Duration>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
    tool_calls: Vec<String>,
    status: Option<&'static str>,
    error: Option<String>,
    finished: bool,
    metadata: RequestMetadata,
}

use std::time::Duration;

impl TelemetryStream {
    fn finish(&mut self, status: &'static str) {
        if self.finished {
            return;
        }
        self.finished = true;
        let elapsed = self.started.elapsed();
        let mut record = self.metadata.record(
            self.session,
            self.turn,
            &self.spec,
            self.started_at,
            elapsed,
            status,
        );
        record.first_token_ms = self
            .first_token
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        record.last_token_ms = self
            .last_token
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        record.tool_calls.clone_from(&self.tool_calls);
        if let Some(usage) = &self.usage {
            record.input_tokens = usage.input_tokens;
            record.output_tokens = usage.output_tokens;
            record.reasoning_tokens = usage.reasoning_tokens;
            record.cache_read_tokens = usage.cache_read_tokens;
            record.cache_write_tokens = usage.cache_write_tokens;
            record.cache_hit = usage.cache_read_tokens.unwrap_or(0) > 0;
            record.cost_usd = usage.vendor.get("cost").and_then(Value::as_f64);
            record.metadata["usage"] = serde_json::to_value(usage).unwrap_or(Value::Null);
        }
        record.finish_reason.clone_from(&self.finish_reason);
        record.error.clone_from(&self.error);
        record.metadata["engine_events"] = self.telemetry.counter_snapshot();
        self.telemetry.push(record);
    }
}

impl Stream for TelemetryStream {
    type Item = norte::Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => {
                if starts_output(&event) {
                    let elapsed = self.started.elapsed();
                    if self.first_token.is_none() {
                        self.first_token = Some(elapsed);
                    }
                    self.last_token = Some(elapsed);
                }
                match &event {
                    StreamEvent::Usage(usage) => self.usage = Some(usage.clone()),
                    StreamEvent::Finish(reason) => self.finish_reason = Some(finish_label(reason)),
                    StreamEvent::ToolCallStart { name, .. } => self.tool_calls.push(name.clone()),
                    _ => {}
                }
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.status = Some("error");
                self.error = Some(error.to_string());
                self.finish("error");
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                let status = self.status.unwrap_or("ok");
                self.finish(status);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for TelemetryStream {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(self.status.unwrap_or("cancelled"));
        }
    }
}

fn starts_output(event: &StreamEvent) -> bool {
    matches!(
        event,
        StreamEvent::TextDelta { .. }
            | StreamEvent::ReasoningDelta { .. }
            | StreamEvent::RefusalDelta { .. }
            | StreamEvent::ToolCallStart { .. }
            | StreamEvent::MediaChunk { .. }
    )
}

fn finish_label(reason: &norte::FinishReason) -> String {
    match reason {
        norte::FinishReason::Stop => "stop".to_owned(),
        norte::FinishReason::ToolCalls => "tool_calls".to_owned(),
        norte::FinishReason::Length => "length".to_owned(),
        norte::FinishReason::ContentFilter => "content_filter".to_owned(),
        norte::FinishReason::Refusal => "refusal".to_owned(),
        norte::FinishReason::Cancelled => "cancelled".to_owned(),
        norte::FinishReason::Other(other) => other.clone(),
        _ => "other".to_owned(),
    }
}

fn reasoning_label(reasoning: ReasoningRequest) -> String {
    match reasoning {
        ReasoningRequest::Automatic => "auto".to_owned(),
        ReasoningRequest::Disabled => "off".to_owned(),
        ReasoningRequest::Effort(effort) => format!("{effort:?}").to_lowercase(),
        ReasoningRequest::Budget(tokens) => format!("budget {tokens}"),
        _ => "unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    struct Next;

    #[async_trait::async_trait]
    impl Continuation for Next {
        async fn call(
            &self,
            _context: &LayerContext<'_>,
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
            wire: crate::provider::Wire::Chat,
            endpoint: None,
            api_key: norte_code_config::ApiKey::new("test"),
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
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let sink: &dyn norte::EventSink = &sender;
        let context = LayerContext {
            session: norte::SessionId::new(),
            turn: 1,
            events: sink,
        };
        let mut stream = layer.wrap(&context, request, &Next).await.expect("stream");
        assert!(stream.next().await.is_none());
        let records = telemetry.take(context.session);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "ok");
        assert!(records[0].sandbox);
        assert!(!records[0].sandbox_network);
    }

    struct OneEventThenPending(StreamEvent);

    #[async_trait::async_trait]
    impl Continuation for OneEventThenPending {
        async fn call(
            &self,
            _context: &LayerContext<'_>,
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
    ) -> (norte_code_store_sqlite::RequestRecord, norte::SessionId) {
        let telemetry = Arc::new(Telemetry::default());
        let spec = ModelSpec {
            provider: "test".to_owned(),
            model: "test".to_owned(),
            wire: crate::provider::Wire::Chat,
            endpoint: None,
            api_key: norte_code_config::ApiKey::new("test"),
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
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let sink: &dyn norte::EventSink = &sender;
        let context = LayerContext {
            session: norte::SessionId::new(),
            turn: 1,
            events: sink,
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
        assert_eq!(record.status, "cancelled");
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
        assert_eq!(record.status, "cancelled");
        assert_eq!(record.input_tokens, Some(120));
        assert_eq!(record.output_tokens, Some(30));
        assert_eq!(record.cost_usd, Some(0.25));
        assert!(record.metadata.get("usage").is_some());
    }
}
