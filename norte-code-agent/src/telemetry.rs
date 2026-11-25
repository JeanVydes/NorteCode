//! Local request telemetry.
//!
//! Norte's observers see every engine event but deliberately do not carry the
//! provider's usage payload. The request row therefore comes from a layer that
//! wraps the model stream: it sees the exact request, every stream event
//! (including `Usage`) and the moment the stream ends. An observer records the
//! engine-event counters beside it, which is useful when a provider never
//! reports usage.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Instant;

use crate::runtime::{EngineEvent, Observer};
use chrono::{DateTime, Utc};
use futures_core::Stream;
use norte::{
    Continuation, Layer, LayerContext, ModelRequest, ModelStream, ReasoningRequest, StreamEvent,
    Usage,
};
use norte_code_protocol::sqlite::RequestRecord;
use norte_code_protocol::sqlite::RequestStatus;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::provider::ModelSpec;

const MAX_TELEMETRY_RECORDS: usize = 2_000;

/// The vendor extension that names an upstream provider, when a host pins one.
const ROUTE_EXTENSION: &str = "openai.chat:provider";

/// Engine-event counters that an observer can update without blocking.
#[derive(Debug, Default)]
pub struct EngineCounters {
    requests: AtomicU64,
    stream_deltas: AtomicU64,
    tool_calls: AtomicU64,
    commits: AtomicU64,
    errors: AtomicU64,
}

/// What the engine counters held at one moment.
#[derive(serde::Serialize)]
struct EngineSnapshot {
    requests: u64,
    stream_deltas: u64,
    tool_calls: u64,
    commits: u64,
    errors: u64,
}

impl EngineCounters {
    /// A snapshot for diagnostics.
    #[must_use]
    pub fn snapshot(&self) -> Value {
        serde_json::to_value(EngineSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            stream_deltas: self.stream_deltas.load(Ordering::Relaxed),
            tool_calls: self.tool_calls.load(Ordering::Relaxed),
            commits: self.commits.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        })
        .unwrap_or(Value::Null)
    }

    fn clear(&self) {
        self.requests.store(0, Ordering::Relaxed);
        self.stream_deltas.store(0, Ordering::Relaxed);
        self.tool_calls.store(0, Ordering::Relaxed);
        self.commits.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
    }
}

/// Shared request records and engine counters.
#[derive(Debug, Default)]
pub struct Telemetry {
    records: Mutex<VecDeque<RequestRecord>>,
    counters: EngineCounters,
}

impl Telemetry {
    /// Records one request, forgetting the oldest one when the ring is full.
    pub fn push(&self, record: RequestRecord) {
        let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        if records.len() == MAX_TELEMETRY_RECORDS {
            records.pop_front();
        }
        records.push_back(record);
    }

    /// Clears request samples and aggregate counters.
    pub fn clear(&self) {
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.counters.clear();
    }

    /// Takes every completed request for a Norte session, oldest first.
    #[must_use]
    pub fn take(&self, north: norte::SessionId) -> Vec<RequestRecord> {
        let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *records)
            .into_iter()
            .partition(|record| record.north_session == north.to_string());
        *records = kept.into();
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
        context: &LayerContext,
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
                first_text: None,
                last_token: None,
                last_chunk: None,
                chunk_count: 0,
                max_chunk_gap: None,
                http_status: None,
                retry_count: None,
                resolved_model: None,
                usage: None,
                finish_reason: None,
                tool_calls: Vec::new(),
                status: None,
                error: None,
                error_kind: None,
                retry_after_ms: None,
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
                    RequestStatus::Error,
                );
                record.error = Some(error.to_string());
                record.error_kind = error_kind_label(&error);
                record.retry_after_ms = error
                    .retry_after()
                    .map(|wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX));
                if let norte::Error::Provider(provider) = &error {
                    record.http_status = provider.status;
                    record.retry_count = Some(provider.transport_retries);
                }
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
                "wire": format!("{:?}", spec.schema),
                // A count and an estimate, never the messages themselves.
                "messages": request.messages.len(),
                "tool_declarations": request.tools.len(),
                "cache_boundaries": request.cache_boundaries.len(),
                "estimated_input_tokens": u64::try_from(bytes / 4).unwrap_or(0),
                // Which upstream the request asked for, and which one served it.
                // `requested` is the vendor extension as it went out: for
                // OpenRouter, `openai.chat:provider`. `served` is what the
                // response named, and stays null until Norte exposes it.
                "route": json!({
                    "requested": request.extensions.get(ROUTE_EXTENSION),
                    "served": Value::Null,
                }),
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
        status: RequestStatus,
    ) -> RequestRecord {
        RequestRecord {
            id: Uuid::new_v4().to_string(),
            session: String::new(),
            north_session: session.to_string(),
            turn,
            provider: spec.provider.clone(),
            model: spec.model.clone(),
            endpoint: Some(spec.endpoint.clone()),
            started_at,
            duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            first_token_ms: None,
            first_text_ms: None,
            last_token_ms: None,
            http_status: None,
            retry_count: None,
            stream_chunk_count: None,
            max_inter_chunk_gap_ms: None,
            resolved_model: None,
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
            status,
            error: None,
            error_kind: None,
            retry_after_ms: None,
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
    first_text: Option<Duration>,
    last_token: Option<Duration>,
    last_chunk: Option<Duration>,
    chunk_count: u64,
    max_chunk_gap: Option<Duration>,
    http_status: Option<u16>,
    retry_count: Option<u32>,
    resolved_model: Option<String>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
    tool_calls: Vec<String>,
    status: Option<RequestStatus>,
    error: Option<String>,
    error_kind: Option<String>,
    retry_after_ms: Option<u64>,
    finished: bool,
    metadata: RequestMetadata,
}

use std::time::Duration;

impl TelemetryStream {
    fn finish(&mut self, status: RequestStatus) {
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
        record.first_text_ms = self
            .first_text
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        record.http_status = self.http_status;
        record.retry_count = self.retry_count;
        record.stream_chunk_count = Some(self.chunk_count);
        record.max_inter_chunk_gap_ms = self
            .max_chunk_gap
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        record.resolved_model = self
            .resolved_model
            .as_ref()
            .filter(|model| *model != &self.spec.model)
            .cloned();
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
        record.error_kind.clone_from(&self.error_kind);
        record.retry_after_ms = self.retry_after_ms;
        record.metadata["engine_events"] = self.telemetry.counter_snapshot();
        self.telemetry.push(record);
    }
}

impl Stream for TelemetryStream {
    type Item = norte::Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => {
                let elapsed = self.started.elapsed();
                if starts_output(&event) {
                    if self.first_token.is_none() {
                        self.first_token = Some(elapsed);
                    }
                    self.last_token = Some(elapsed);
                }
                if starts_visible_text(&event) && self.first_text.is_none() {
                    self.first_text = Some(elapsed);
                }
                if is_stream_chunk(&event) {
                    if let Some(previous) = self.last_chunk {
                        self.max_chunk_gap = Some(
                            self.max_chunk_gap
                                .map_or(elapsed.saturating_sub(previous), |gap| {
                                    gap.max(elapsed.saturating_sub(previous))
                                }),
                        );
                    }
                    self.last_chunk = Some(elapsed);
                    self.chunk_count = self.chunk_count.saturating_add(1);
                }
                match &event {
                    StreamEvent::Usage(usage) => self.usage = Some(usage.clone()),
                    StreamEvent::Finish(reason) => self.finish_reason = Some(finish_label(reason)),
                    StreamEvent::ToolCallStart { name, .. } => self.tool_calls.push(name.clone()),
                    StreamEvent::ResponseMetadata {
                        http_status,
                        retry_count,
                        resolved_model,
                    } => {
                        self.http_status = http_status.or(self.http_status);
                        self.retry_count = retry_count.or(self.retry_count);
                        if let Some(model) =
                            resolved_model.as_ref().filter(|model| !model.is_empty())
                        {
                            self.resolved_model = Some(model.clone());
                        }
                    }
                    _ => {}
                }
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.status = Some(RequestStatus::Error);
                self.error = Some(error.to_string());
                self.error_kind = error_kind_label(&error);
                self.retry_after_ms = error
                    .retry_after()
                    .map(|wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX));
                if let norte::Error::Provider(provider) = &error {
                    self.http_status = provider.status.or(self.http_status);
                    self.retry_count = Some(provider.transport_retries);
                }
                self.finish(RequestStatus::Error);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                let status = self.status.unwrap_or(RequestStatus::Ok);
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
            self.finish(self.status.unwrap_or(RequestStatus::Cancelled));
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

fn starts_visible_text(event: &StreamEvent) -> bool {
    matches!(
        event,
        StreamEvent::TextDelta { text, .. } | StreamEvent::RefusalDelta { text, .. }
            if !text.is_empty()
    )
}

fn is_stream_chunk(event: &StreamEvent) -> bool {
    matches!(
        event,
        StreamEvent::TextDelta { text, .. }
            | StreamEvent::ReasoningDelta { text, .. }
            | StreamEvent::RefusalDelta { text, .. }
            if !text.is_empty()
    ) || matches!(
        event,
        StreamEvent::ToolCallDelta { arguments, .. } if !arguments.is_empty()
    ) || matches!(
        event,
        StreamEvent::MediaChunk { bytes, .. } if !bytes.is_empty()
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

/// The typed kind of a provider failure, as it goes into the telemetry row.
///
/// A report should count rate limits without parsing error messages, so the
/// variant is stored beside the text. A non-provider error has no kind.
fn error_kind_label(error: &norte::Error) -> Option<String> {
    use norte::ProviderErrorKind as Kind;
    let kind = match error {
        norte::Error::Provider(provider) => provider.kind,
        _ => return None,
    };
    Some(
        match kind {
            Kind::RateLimited => "rate_limited",
            Kind::QuotaExceeded => "quota_exceeded",
            Kind::InvalidRequest => "invalid_request",
            Kind::Authentication => "authentication",
            Kind::Permission => "permission",
            Kind::NotFound => "not_found",
            Kind::ContentFilter => "content_filter",
            Kind::ModelOverloaded => "model_overloaded",
            Kind::ServerError => "server_error",
            Kind::Transport => "transport",
            Kind::Decode => "decode",
            Kind::Cancelled => "cancelled",
            _ => "other",
        }
        .to_owned(),
    )
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
#[path = "telemetry_tests.rs"]
mod tests;
