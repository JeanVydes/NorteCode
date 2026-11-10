//! Mapping one engine event onto the wire and settling what it cost.

use chrono::Utc;
use norte::FinishReason;
use norte::domain::part::Part;
use std::collections::HashMap;

use crate::runtime::EngineEvent;
use norte_code_protocol::costs::CostRecord;
use norte_code_protocol::sqlite::RequestStatus;
use norte_code_wire::{ServerEvent, ServerEventKind};

use super::Service;
use super::compaction::used_context;
use super::mapping::{
    ToolResultText, call_arguments, emit, reported_cost, request_metrics, shorten_tool_output,
    stream_label, to_delta, to_usage, tool_result_index,
};
use super::tooling::output_text;
use super::turn_context::TurnContext;
use super::turn_helpers::{budget_notice_kind, is_detached};
use super::turn_history::from_todo_item;

impl Service {
    /// Maps one engine event and answers whether to keep going (the client is
    /// still reading).
    #[allow(clippy::too_many_lines)]
    pub(super) async fn handle(
        &self,
        ctx: &mut TurnContext<'_>,
        event: &EngineEvent,
        tool_results: &mut Option<HashMap<String, ToolResultText>>,
    ) -> bool {
        let (engine, request_id, key, nid, tx, gate) = (
            ctx.engine,
            ctx.request_id,
            ctx.key,
            ctx.nid,
            ctx.tx,
            ctx.gate,
        );
        let stats = &mut *ctx.stats;
        let kind = match event {
            EngineEvent::StreamDelta { delta, .. } => {
                to_delta(delta).map(|delta| ServerEventKind::Delta {
                    turn: stats.turn,
                    delta,
                })
            }
            EngineEvent::PartCompleted {
                index,
                part: Part::Media(media),
                ..
            } => super::media_wire::media_part(media).map(|media| {
                ServerEventKind::MediaPartCompleted {
                    turn: stats.turn,
                    index: *index,
                    media,
                }
            }),
            // A tool that streams reports as it reads. It rides the same
            // channel as the call's own events, so the client sees the output
            // arrive instead of waiting for the call to finish.
            EngineEvent::ToolOutput {
                call_id,
                tool,
                stream,
                bytes,
                last,
                ..
            } => {
                let text = String::from_utf8_lossy(bytes).into_owned();
                Some(ServerEventKind::ToolOutput {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    stream: stream_label(*stream),
                    // The tool redacted before it stored the bytes; the lossy
                    // conversion is for a chunk that splits a code point, which
                    // the reader already avoids by cutting on character
                    // boundaries.
                    text,
                    last: *last,
                })
            }
            EngineEvent::ToolCallStarted { call_id, tool, .. } => {
                let arguments = if let Some(arguments) = self.activity.started(call_id) {
                    Some(arguments)
                } else {
                    call_arguments(engine, nid, *call_id).await
                };
                let source = self.activity.source(call_id);
                Some(ServerEventKind::ToolStarted {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    arguments,
                    source,
                })
            }
            EngineEvent::ToolCallFinished {
                call_id,
                tool,
                is_error,
                duration_ms,
                ..
            } => {
                self.effects
                    .finished(nid, *call_id, tool.as_str(), *is_error);
                let recorded = self.activity.finished(call_id);
                let fallback = if recorded.as_ref().and_then(output_text).is_none() {
                    if tool_results.is_none() {
                        *tool_results = tool_result_index(engine, nid).await;
                    }
                    tool_results
                        .as_ref()
                        .and_then(|results| results.get(&call_id.to_string()))
                        .filter(|result| result.is_error == *is_error)
                        .map(|result| result.text.clone())
                } else {
                    None
                };
                let error = if *is_error {
                    if let Some(text) = recorded.as_ref().and_then(output_text) {
                        Some(text)
                    } else {
                        fallback.clone()
                    }
                } else {
                    None
                };
                let (output, truncated) = if *is_error {
                    (None, false)
                } else {
                    let text = recorded.as_ref().and_then(output_text).or(fallback);
                    text.map_or((None, false), |text| {
                        let (text, truncated) = shorten_tool_output(text);
                        (Some(text), truncated)
                    })
                };
                // A detached call is an error and not a failure, and only the
                // result knows which: its structured form says `code: detached`.
                let detached = recorded.as_ref().is_some_and(is_detached);
                // The task list is the model's, written through `todo`, and a
                // client that only hears about it on the next replay shows it
                // late: after the person switches sessions and comes back.
                // Sending the list with the call that changed it puts it on
                // screen at the moment the model writes it.
                if tool.as_str() == "todo" {
                    let todos = self.todo.list(Some(nid));
                    if !todos.is_empty()
                        && !emit(
                            tx,
                            ServerEvent::new(
                                request_id,
                                Some(key.clone()),
                                ServerEventKind::SessionTodos {
                                    todos: todos.iter().map(from_todo_item).collect(),
                                },
                            ),
                        )
                        .await
                    {
                        return false;
                    }
                }
                Some(ServerEventKind::ToolFinished {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    is_error: *is_error,
                    error,
                    output,
                    truncated,
                    detached,
                    duration_ms: *duration_ms,
                })
            }
            EngineEvent::CommitSucceeded { version, .. } => {
                // The answer is committed, so the budget already knows what it
                // cost: a crossing that happened there is reported now, not at
                // the next request.
                for notice in self.budget_notices(key, false) {
                    let kind = budget_notice_kind(notice);
                    if !emit(tx, ServerEvent::new(request_id, Some(key.clone()), kind)).await {
                        return false;
                    }
                }
                Some(ServerEventKind::Committed {
                    turn: stats.turn,
                    version: *version,
                })
            }
            EngineEvent::LayerShortCircuited { layer, .. } => {
                if layer == "budget" {
                    self.budget_stopped(key)
                        .map(|status| ServerEventKind::BudgetExceeded {
                            scope: status.scope,
                            unit: status.unit,
                            spent: status.spent,
                            limit: status.limit,
                            action: status.action,
                        })
                } else {
                    None
                }
            }
            EngineEvent::RequestReconstructed { .. } => {
                // The summary waits for the commit, but the eager gate has to
                // see the context this request is about to carry before it is
                // sent; this event is the last point at which it can.
                if let Some(gate) = gate
                    && let Ok(history) = engine.history(nid).await
                {
                    gate.update(used_context(&history));
                }
                None
            }
            EngineEvent::TurnStopped { reason, .. } => Some(ServerEventKind::TurnStopped {
                reason: reason.clone(),
            }),
            EngineEvent::InterceptionApplied { messages, .. } => {
                // The engine applied the steering messages, so the durable copy
                // in the session is no longer pending.
                let applied = self.steering.applied(nid, *messages);
                let ids: Vec<String> = applied.iter().map(|steered| steered.id.clone()).collect();
                // The text travels with the ids so a client can print what
                // reached the request, which for a message the human did not
                // type (a finished job's report) is the only way to see it.
                let texts: Vec<String> =
                    applied.iter().map(|steered| steered.text.clone()).collect();
                self.clear_applied_steers(key, &applied).await;
                Some(ServerEventKind::SteerApplied {
                    ids,
                    messages: texts,
                })
            }
            _ => None,
        };
        if let Some(kind) = kind {
            return emit(tx, ServerEvent::new(request_id, Some(key.clone()), kind)).await;
        }
        true
    }

    /// Reports every model request committed since the last look.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn settle(&self, ctx: &mut TurnContext<'_>, final_check: bool) -> bool {
        let (engine, request_id, key, nid, tx, gate) = (
            ctx.engine,
            ctx.request_id,
            ctx.key,
            ctx.nid,
            ctx.tx,
            ctx.gate,
        );
        let stats = &mut *ctx.stats;
        let Ok(history) = engine.history(nid).await else {
            return true;
        };
        if let Some(gate) = gate {
            gate.update(used_context(&history));
        }
        for record in self.telemetry_for(key, nid) {
            if record.status == RequestStatus::Ok {
                // The telemetry waits for the message it belongs to: the
                // provider answered before the message was committed.
                stats.pending.push_back(record);
            } else {
                self.record_request(key, &record).await;
            }
        }
        for message in history.iter().skip(stats.billed) {
            let Some(origin) = &message.origin else {
                continue;
            };
            stats.requests += 1;
            let cost = reported_cost(&origin.usage);
            if let Some(cost) = cost {
                stats.cost += cost;
            } else {
                stats.unpriced += 1;
            }
            stats.input_tokens += origin.usage.input_tokens.unwrap_or(0);
            stats.output_tokens += origin.usage.output_tokens.unwrap_or(0);
            stats.cache_read_tokens += origin.usage.cache_read_tokens.unwrap_or(0);
            stats.cache_write_tokens += origin.usage.cache_write_tokens.unwrap_or(0);
            // The last request's input tokens are the context: the whole
            // conversation goes again every time, so what it carried is what
            // the session holds.
            if let Some(input) = origin.usage.input_tokens {
                stats.context_used = input;
            }
            let record = CostRecord {
                session: key.to_string(),
                turn: stats.turn,
                index: stats.requests,
                model: self.model_label(key),
                input_tokens: origin.usage.input_tokens,
                output_tokens: origin.usage.output_tokens,
                reasoning_tokens: origin.usage.reasoning_tokens,
                cache_read_tokens: origin.usage.cache_read_tokens,
                cache_write_tokens: origin.usage.cache_write_tokens,
                cost_usd: cost,
                recorded_at: Utc::now(),
            };
            if let Err(error) = self.directory.record_cost(&record).await {
                tracing::warn!(session = %key, %error, "could not record request cost");
            }
            let telemetry = stats
                .pending
                .pop_front()
                .unwrap_or_else(|| self.request_from_usage(key, nid, stats.turn, &origin.usage));
            let metrics = request_metrics(&telemetry);
            self.record_request(key, &telemetry).await;
            let notices = self.budget_notices(key, final_check);
            let event = ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::RequestReconstructed {
                    turn: stats.turn,
                    index: stats.requests,
                    usage: to_usage(&origin.usage),
                    cost_usd: cost,
                    truncated: matches!(origin.finish_reason, FinishReason::Length),
                    duration_ms: metrics.duration_ms,
                    first_token_ms: metrics.first_token_ms,
                    tokens_per_second: metrics.tokens_per_second,
                    cache_hit: metrics.cache_hit,
                    tools: metrics.tools,
                    provider: (!telemetry.provider.is_empty()).then(|| telemetry.provider.clone()),
                    model: (!telemetry.model.is_empty()).then(|| telemetry.model.clone()),
                },
            );
            if !emit(tx, event).await {
                stats.billed = history.len();
                return false;
            }
            for notice in notices {
                let kind = budget_notice_kind(notice);
                if !emit(tx, ServerEvent::new(request_id, Some(key.clone()), kind)).await {
                    stats.billed = history.len();
                    return false;
                }
            }
        }
        stats.billed = history.len();
        true
    }
}
