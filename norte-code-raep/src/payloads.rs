//! Canonical RAEP payloads emitted by the `NorteCode` adapter.

use norte_code_protocol::ServerEventKind;
use serde::Serialize;
use serde_json::{Value, json};

/// Converts a `NorteCode` event into the stable payload understood by RAEP
/// clients.
///
/// The complete `NorteCode` event remains available in the envelope extension.
/// That keeps the adapter lossless while preventing clients from depending on
/// `NorteCode`'s request/session wrapper for the common event families.
#[allow(clippy::too_many_lines)]
pub(crate) fn canonical_event_payload(kind: &ServerEventKind) -> Value {
    let payload = match kind {
        ServerEventKind::TurnStarted { turn } => CanonicalEvent::TurnStarted { turn: *turn },
        ServerEventKind::Delta { turn, delta } => CanonicalEvent::TokenDelta {
            turn: *turn,
            delta: serde_json::to_value(delta).expect("protocol delta is serializable"),
        },
        ServerEventKind::ToolStarted {
            turn,
            call_id,
            tool,
            arguments,
            source,
        } => CanonicalEvent::ToolCallStarted {
            turn: *turn,
            call_id,
            tool,
            arguments,
            source,
        },
        ServerEventKind::ToolOutput {
            turn,
            call_id,
            tool,
            stream,
            text,
            last,
        } => CanonicalEvent::JobProgressDelta {
            turn: *turn,
            call_id,
            tool,
            stream: serde_json::to_value(stream).expect("output stream is serializable"),
            text,
            last: *last,
        },
        ServerEventKind::ToolFinished {
            turn,
            call_id,
            tool,
            is_error,
            error,
            output,
            truncated,
            detached,
            duration_ms,
        } => CanonicalEvent::ToolCallFinished {
            turn: *turn,
            call_id,
            tool,
            is_error: *is_error,
            error,
            output,
            truncated: *truncated,
            detached: *detached,
            duration_ms: *duration_ms,
        },
        ServerEventKind::JobList { jobs } => CanonicalEvent::JobList {
            jobs: serde_json::to_value(jobs).expect("job list is serializable"),
        },
        ServerEventKind::JobsStopped { stopped } => {
            CanonicalEvent::JobsStopped { stopped: *stopped }
        }
        ServerEventKind::QuestionAsked {
            turn,
            question,
            prompt,
            options,
            multiple,
        } => CanonicalEvent::QuestionAsked {
            turn: *turn,
            question,
            prompt,
            options,
            multiple: *multiple,
        },
        ServerEventKind::QuestionAnswered { question } => CanonicalEvent::ApprovalResolved {
            approval_id: question,
        },
        ServerEventKind::TurnFinished {
            turn,
            phase,
            committed,
        } => CanonicalEvent::TurnStateChanged {
            turn: Some(*turn),
            state: json!({"state": "finished", "phase": phase, "committed": committed}),
        },
        ServerEventKind::TurnStopped { reason } => CanonicalEvent::TurnStateChanged {
            turn: None,
            state: json!({"state": "cancelled", "reason": reason}),
        },
        ServerEventKind::RequestReconstructed {
            turn,
            index,
            usage,
            cost_usd,
            truncated,
            duration_ms,
            first_token_ms,
            tokens_per_second,
            cache_hit,
            tools,
        } => CanonicalEvent::OutputPartFinished {
            turn: *turn,
            index: *index,
            usage: serde_json::to_value(usage).expect("usage is serializable"),
            cost_usd: *cost_usd,
            truncated: *truncated,
            duration_ms: *duration_ms,
            first_token_ms: *first_token_ms,
            tokens_per_second: *tokens_per_second,
            cache_hit: *cache_hit,
            tools,
        },
        ServerEventKind::BudgetWarning {
            scope,
            unit,
            spent,
            limit,
        } => CanonicalEvent::BudgetWarning {
            scope: serde_json::to_value(scope).expect("budget scope is serializable"),
            unit: serde_json::to_value(unit).expect("budget unit is serializable"),
            spent: *spent,
            limit: *limit,
        },
        ServerEventKind::BudgetExceeded {
            scope,
            unit,
            spent,
            limit,
            action,
        } => CanonicalEvent::BudgetExceeded {
            scope: serde_json::to_value(scope).expect("budget scope is serializable"),
            unit: serde_json::to_value(unit).expect("budget unit is serializable"),
            spent: *spent,
            limit: *limit,
            action: serde_json::to_value(action).expect("budget action is serializable"),
        },
        ServerEventKind::RetryScheduled {
            turn,
            attempt,
            delay_ms,
            reason,
        } => CanonicalEvent::ProviderRetry {
            turn: *turn,
            state: "scheduled",
            attempt: *attempt,
            delay_ms: Some(*delay_ms),
            reason: Some(reason),
        },
        ServerEventKind::RetryStarted { turn, attempt } => CanonicalEvent::ProviderRetry {
            turn: *turn,
            state: "started",
            attempt: *attempt,
            delay_ms: None,
            reason: None,
        },
        ServerEventKind::Error { code, message } => CanonicalEvent::Error {
            code: serde_json::to_value(code).expect("error code is serializable"),
            message,
        },
        other => CanonicalEvent::NorteCode {
            payload: serde_json::to_value(other).expect("protocol event is serializable"),
        },
    };
    serde_json::to_value(payload).expect("canonical event is serializable")
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum CanonicalEvent<'a> {
    /// A turn began.
    TurnStarted { turn: u64 },
    /// A model token delta.
    TokenDelta { turn: u64, delta: Value },
    /// A tool call began.
    ToolCallStarted {
        turn: u64,
        call_id: &'a str,
        tool: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: &'a Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: &'a Option<String>,
    },
    /// A detached tool/job produced output.
    JobProgressDelta {
        turn: u64,
        call_id: &'a str,
        tool: &'a str,
        stream: Value,
        text: &'a str,
        last: bool,
    },
    /// A tool call ended.
    ToolCallFinished {
        turn: u64,
        call_id: &'a str,
        tool: &'a str,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: &'a Option<String>,
        truncated: bool,
        detached: bool,
        duration_ms: u64,
    },
    /// The jobs currently known by the session.
    JobList { jobs: Value },
    /// The result of stopping jobs.
    JobsStopped { stopped: usize },
    /// A question requiring client input.
    QuestionAsked {
        turn: u64,
        question: &'a str,
        prompt: &'a str,
        options: &'a Vec<String>,
        multiple: bool,
    },
    /// An approval/question was resolved.
    ApprovalResolved { approval_id: &'a str },
    /// A turn changed lifecycle state.
    TurnStateChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        turn: Option<u64>,
        state: Value,
    },
    /// Provider usage and cost for one output part.
    OutputPartFinished {
        turn: u64,
        index: u64,
        usage: Value,
        cost_usd: Option<f64>,
        truncated: bool,
        duration_ms: u64,
        first_token_ms: Option<u64>,
        tokens_per_second: Option<f64>,
        cache_hit: bool,
        tools: &'a Vec<String>,
    },
    /// A budget warning.
    BudgetWarning {
        scope: Value,
        unit: Value,
        spent: f64,
        limit: f64,
    },
    /// A budget was exhausted.
    BudgetExceeded {
        scope: Value,
        unit: Value,
        spent: f64,
        limit: f64,
        action: Value,
    },
    /// A provider retry changed state.
    ProviderRetry {
        turn: u64,
        state: &'static str,
        attempt: u32,
        delay_ms: Option<u64>,
        reason: Option<&'a String>,
    },
    /// A protocol error.
    Error { code: Value, message: &'a str },
    /// An event not yet normalized by this adapter.
    NorteCode { payload: Value },
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte_code_protocol::types::OutputStream;

    #[test]
    fn tool_output_uses_canonical_job_payload_and_preserves_wire_event() {
        let payload = canonical_event_payload(&ServerEventKind::ToolOutput {
            turn: 3,
            call_id: "call-1".to_owned(),
            tool: "shell".to_owned(),
            stream: OutputStream::Stdout,
            text: "hello".to_owned(),
            last: true,
        });

        assert_eq!(payload["event"], "job_progress_delta");
        assert_eq!(payload["turn"], 3);
        assert_eq!(payload["call_id"], "call-1");
        assert_eq!(payload["stream"], "stdout");
        assert_eq!(payload["last"], true);
    }

    #[test]
    fn unnormalized_events_keep_their_typed_event_name() {
        let payload = canonical_event_payload(&ServerEventKind::SessionDeleted);

        assert_eq!(payload["event"], "norte_code");
        assert_eq!(payload["payload"]["event"], "session_deleted");
    }
}
