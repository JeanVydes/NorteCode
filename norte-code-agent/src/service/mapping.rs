//! Protocol mapping, usage metrics and the budget layer.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use norte::ToolStream;
use norte::harness::Harness;
use norte::harness::StreamDelta;
use norte::{
    CallId, Continuation, Layer, LayerContext, ModelRef, ModelRequest, ModelStream,
    ProviderErrorKind, SessionId, ShortCircuitReason, StreamEvent, Usage,
};
use norte_code_config::{BudgetConfig, RetryConfig};
use norte_code_cost::{BudgetBook, CostRecord};
use norte_code_protocol::{
    Budget, BudgetUnit, Delta, ErrorCode, ServerEvent, ServerEventKind, SessionKey,
};
use tokio::sync::mpsc;

use super::EventStream;

/// A stream carrying one event.
pub(super) fn single(
    request_id: u64,
    session: Option<SessionKey>,
    kind: ServerEventKind,
) -> EventStream {
    Box::pin(futures_util::stream::once(async move {
        ServerEvent::new(request_id, session, kind)
    }))
}

/// Sends an event and reports whether the client is still reading.
pub(super) async fn emit(tx: &mpsc::Sender<ServerEvent>, event: ServerEvent) -> bool {
    tx.send(event).await.is_ok()
}

/// The provider-reported cost, in USD, when the payload carried one.
pub(super) fn reported_cost(usage: &Usage) -> Option<f64> {
    usage.vendor.get("cost").and_then(serde_json::Value::as_f64)
}

/// Per-request figures the telemetry layer measured.
pub(super) struct RequestMetrics {
    pub(super) duration_ms: u64,
    pub(super) first_token_ms: Option<u64>,
    pub(super) tokens_per_second: Option<f64>,
    pub(super) cache_hit: bool,
    pub(super) tools: Vec<String>,
}

pub(super) fn request_metrics(record: &norte_code_store_sqlite::RequestRecord) -> RequestMetrics {
    RequestMetrics {
        duration_ms: record.duration_ms,
        first_token_ms: record.first_token_ms,
        tokens_per_second: tokens_per_second(record),
        cache_hit: record.cache_hit,
        tools: record.tool_calls.clone(),
    }
}

pub(super) fn tokens_per_second(record: &norte_code_store_sqlite::RequestRecord) -> Option<f64> {
    token_rate(
        record.output_tokens,
        record.duration_ms,
        record.first_token_ms,
    )
}

#[allow(clippy::cast_precision_loss)]
pub(super) fn token_rate(
    output: Option<u64>,
    duration_ms: u64,
    first_token_ms: Option<u64>,
) -> Option<f64> {
    let output = output.filter(|tokens| *tokens > 0)?;
    let first = first_token_ms.unwrap_or(0);
    let generation = duration_ms.saturating_sub(first);
    // A provider that delivers the whole answer in one final chunk reports the
    // first and last token almost together. Dividing by that sliver would
    // produce absurd rates, so fall back to the whole request when the window
    // is too small to be meaningful.
    let window = if generation >= 50 {
        generation
    } else {
        duration_ms
    };
    if window == 0 {
        return None;
    }
    Some(output as f64 * 1_000.0 / window as f64)
}

/// The text of a failed tool result, read from the committed history.
pub(super) async fn tool_error(
    engine: &Harness,
    session: SessionId,
    call_id: &norte::CallId,
) -> Option<String> {
    let history = engine.history(session).await.ok()?;
    for message in history.iter().rev() {
        for part in &message.parts {
            if let norte::Part::ToolResult(result) = part {
                if result.call_id == *call_id && result.is_error {
                    let text = result
                        .parts
                        .iter()
                        .filter_map(norte::Part::as_text)
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Some(text);
                }
            }
        }
    }
    None
}

/// The text a tool returned, whether it succeeded or failed.
pub(super) async fn tool_result_text(
    engine: &Harness,
    session: SessionId,
    call_id: &norte::CallId,
) -> Option<String> {
    let history = engine.history(session).await.ok()?;
    for message in history.iter().rev() {
        for part in &message.parts {
            if let norte::Part::ToolResult(result) = part {
                if result.call_id == *call_id {
                    return Some(
                        result
                            .parts
                            .iter()
                            .filter_map(norte::Part::as_text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
            }
        }
    }
    None
}

/// The most tool output the wire carries for a verbose renderer.
const TOOL_OUTPUT_LIMIT: usize = 4_000;

/// Shortens a tool result on a character boundary.
pub(super) fn shorten_tool_output(text: String) -> (String, bool) {
    if text.chars().count() <= TOOL_OUTPUT_LIMIT {
        return (text, false);
    }
    let mut shortened: String = text.chars().take(TOOL_OUTPUT_LIMIT).collect();
    shortened.push_str("\n… [truncated]");
    (shortened, true)
}

pub(super) fn to_usage(usage: &Usage) -> norte_code_protocol::Usage {
    norte_code_protocol::Usage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        cost_usd: reported_cost(usage),
        vendor: usage.vendor.clone(),
    }
}

/// Converts a validated config budget to the wire shape.
///
/// Config validation already rejects unknown units and actions; a programmatic
/// `Config` can still carry one, so an unusable budget is dropped rather than
/// guessed at.
pub(super) fn configured_budget(budget: Option<&BudgetConfig>) -> Option<Budget> {
    let budget = budget?;
    let unit = match budget.unit.as_str() {
        "usd" => BudgetUnit::Usd,
        "tokens" => BudgetUnit::Tokens,
        "requests" => BudgetUnit::Requests,
        _ => return None,
    };
    let action = match budget.action.as_str() {
        "warn" => norte_code_protocol::BudgetAction::Warn,
        "ask" => norte_code_protocol::BudgetAction::Ask,
        "stop" => norte_code_protocol::BudgetAction::Stop,
        _ => return None,
    };
    Some(Budget {
        unit,
        limit: budget.limit,
        action,
    })
}

/// Stops a model request before it is sent when a budget is exhausted.
///
/// Norte has no price feed, so this is a check against the ledger's measured
/// spend, not a worst-case estimate. The layer also records what each request
/// cost, in the engine's own path: the gate must see the cost of the request
/// it just let through before it decides about the next one.
pub(super) struct BudgetLayer {
    pub(super) book: Arc<Mutex<BudgetBook>>,
}

#[async_trait]
impl Layer for BudgetLayer {
    fn name(&self) -> &'static str {
        "budget"
    }

    async fn wrap(
        &self,
        context: &LayerContext,
        request: ModelRequest,
        next: &dyn Continuation,
    ) -> norte::Result<ModelStream> {
        let stop = {
            let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
            book.stop()
        };
        if let Some(status) = stop {
            let unit = match status.unit {
                BudgetUnit::Usd => "USD",
                BudgetUnit::Tokens => "tokens",
                BudgetUnit::Requests => "requests",
            };
            let detail = format!(
                "{:?} budget: spent {:.6} of {:.6} {unit}; raise the budget and resume",
                status.scope, status.spent, status.limit
            );
            // The layer names the refusal; the harness records it as a
            // `LayerShortCircuited` event and propagates this error unchanged.
            return Err(norte::Error::ShortCircuited {
                layer: self.name().to_owned(),
                reason: ShortCircuitReason::Custom {
                    name: "budget_exceeded".to_owned(),
                    detail,
                },
            });
        }
        let model = request.model.clone();
        // The request counts as soon as it is let through, and what it cost
        // when the provider reports it: the gate that measures the budget
        // must know of both before it decides about the next request.
        {
            let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
            book.count_request();
        }
        let stream = next.call(context, request).await?;
        Ok(account_usage(
            stream,
            Arc::clone(&self.book),
            context.session,
            context.turn,
            model,
        ))
    }
}

/// Wraps a model stream so the budget book learns what the request cost.
///
/// The usage is recorded as it flows, before the answer is committed, so the
/// gate that lets the next request through already knows what this one cost.
fn account_usage(
    stream: ModelStream,
    book: Arc<Mutex<BudgetBook>>,
    session: SessionId,
    turn: u64,
    model: ModelRef,
) -> ModelStream {
    let mut recorded = false;
    Box::pin(futures_util::StreamExt::map(stream, move |event| {
        if !recorded && let Ok(StreamEvent::Usage(usage)) = &event {
            recorded = true;
            let mut book = book.lock().unwrap_or_else(PoisonError::into_inner);
            book.record_usage(&CostRecord {
                session: session.to_string(),
                turn,
                index: 0,
                model: model.to_string(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                reasoning_tokens: usage.reasoning_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
                cost_usd: reported_cost(usage),
                recorded_at: Utc::now(),
            });
        }
        event
    }))
}

/// The arguments a pending call would run with, for showing a human what they
/// are approving.
///
/// The approval record is authoritative: when a policy rewrote the arguments,
/// it holds the rewritten object, while history still has what the model wrote.
/// Older records predate that field, so history is the fallback. Missing
/// history is not an error: the fields are optional.
pub(super) async fn call_arguments(
    engine: &Harness,
    session: SessionId,
    call_id: CallId,
) -> Option<serde_json::Value> {
    if let Ok(records) = engine.pending_approvals(session).await {
        if let Some(arguments) = records
            .into_iter()
            .find(|record| record.call_id == call_id)
            .and_then(|record| record.arguments)
        {
            return Some(arguments);
        }
    }
    let history = engine.history(session).await.ok()?;
    history
        .iter()
        .flat_map(|message| message.tool_calls())
        .find(|call| call.call_id == call_id)
        .map(|call| call.arguments.clone())
}

pub(super) fn to_phase(phase: norte::harness::Phase) -> norte_code_protocol::Phase {
    match phase {
        norte::harness::Phase::Streaming => norte_code_protocol::Phase::Streaming,
        norte::harness::Phase::AwaitingTool => norte_code_protocol::Phase::AwaitingTool,
        norte::harness::Phase::AwaitingApproval => norte_code_protocol::Phase::AwaitingApproval,
        _ => norte_code_protocol::Phase::Idle,
    }
}

/// Classifies an engine failure for the client.
///
/// A lost store race is not a provider error: the engine read version N, but
/// another writer committed N+1 before this turn did. The turn did not commit,
/// so the client should resume instead of resending its message.
pub(super) fn error_code(error: &norte::Error) -> ErrorCode {
    match error {
        norte::Error::StoreConflict { .. } => ErrorCode::Conflict,
        // A capability the model lacks is the request's fault, not the
        // provider's: the wire was never reached, and retrying it would spend
        // the same refusal again. Reporting it as `Provider` read as a flaky
        // vendor and hid the one thing worth acting on — the wrong model, or a
        // capability the host never declared.
        norte::Error::Capability { .. } => ErrorCode::Capability,
        norte::Error::InvalidInput(_) | norte::Error::InvalidId(_) => ErrorCode::InvalidRequest,
        _ => ErrorCode::Provider,
    }
}

/// Plans a bounded retry for a transient provider failure.
///
/// The wait is the provider's `retry_after` when it gave one, and otherwise a
/// capped exponential backoff. Permanent failures (quota, authentication,
/// malformed requests) return `None`: retrying them only spends more.
pub(super) fn retry_plan(
    error: &norte::Error,
    config: &RetryConfig,
    used: u32,
) -> Option<(Duration, String)> {
    if used >= config.max_attempts {
        return None;
    }
    let transient = matches!(
        error.provider_kind(),
        Some(
            ProviderErrorKind::RateLimited
                | ProviderErrorKind::ModelOverloaded
                | ProviderErrorKind::ServerError
                | ProviderErrorKind::Transport
        )
    );
    if !transient {
        return None;
    }
    let cap = Duration::from_millis(config.max_delay_ms);
    let delay = error.retry_after().map_or_else(
        || {
            let factor = 1u64.checked_shl(used.min(20)).unwrap_or(u64::MAX);
            Duration::from_millis(config.backoff_ms.saturating_mul(factor)).min(cap)
        },
        |after| after.min(cap),
    );
    Some((delay, error.to_string()))
}

/// The wire name of the stream a chunk came from.
pub(super) fn stream_label(stream: ToolStream) -> &'static str {
    match stream {
        ToolStream::Stdout => "stdout",
        ToolStream::Stderr => "stderr",
        // `ToolStream` is `#[non_exhaustive]`: a stream added later is one this
        // client has no name for, and an unnamed one is better than a panic.
        _ => "other",
    }
}

pub(super) fn to_delta(delta: &StreamDelta) -> Option<Delta> {
    Some(match delta {
        StreamDelta::Text { index, text } => Delta::Text {
            index: *index,
            text: text.clone(),
        },
        StreamDelta::Reasoning { index, text } => Delta::Reasoning {
            index: *index,
            text: text.clone(),
        },
        StreamDelta::Refusal { index, text } => Delta::Refusal {
            index: *index,
            text: text.clone(),
        },
        StreamDelta::ToolArguments {
            index,
            call_id,
            arguments,
        } => Delta::ToolArguments {
            index: *index,
            call_id: call_id.to_string(),
            arguments: arguments.clone(),
        },
        StreamDelta::Media { index, bytes, last } => Delta::Media {
            index: *index,
            bytes: u64::try_from(*bytes).unwrap_or(u64::MAX),
            last: *last,
        },
        // Continuity, safety ratings and logprobs are not answer text; the TUI
        // has nothing to render for them.
        _ => return None,
    })
}
