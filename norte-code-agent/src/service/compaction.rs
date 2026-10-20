//! Compaction trigger, layer and history helpers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use norte::{
    Continuation, EngineEvent, Layer, LayerContext, Message, ModelRequest, ModelStream,
    Reasoning as ReasoningCapability, ReasoningEffort, ReasoningRequest, Role, ShortCircuitReason,
};

/// The instruction that turns the old transcript into a handoff summary.
pub(super) const SUMMARY_PROMPT: &str = "\
Rewrite this conversation as a concise handoff summary for another assistant that \
will continue the work. Keep the user's goal, the decisions and constraints, the \
files touched and why, the commands run and their failures, the open todos, and \
the user's latest request. Do not invent facts and do not call tools. Output only \
the summary.";

/// The marker the compaction layer puts on the error that stops a request.
pub(super) const COMPACTION_SAFE_POINT: &str = "norte-code: compaction safe point";

/// The context a session currently occupies, in tokens.
///
/// The provider's last prompt count plus its answer is authoritative. Only when
/// no request reported usage do we fall back to a bytes/4 estimate.
pub(super) fn used_context(history: &[Message]) -> u64 {
    if let Some(used) = history
        .iter()
        .rev()
        .find_map(|message| message.origin.as_ref())
        .map(|origin| {
            origin.usage.input_tokens.unwrap_or(0) + origin.usage.output_tokens.unwrap_or(0)
        })
        .filter(|used| *used > 0)
    {
        return used;
    }
    serde_json::to_vec(history).map_or(0, |bytes| bytes.len() as u64 / 4)
}

/// Where a verbatim suffix may start without orphaning tool results.
pub(super) fn recent_start(messages: &[Message], keep: usize) -> usize {
    let mut start = messages.len().saturating_sub(keep);
    while start > 0 && messages[start].role == Role::Tool {
        start -= 1;
    }
    start
}

/// The lowest reasoning the model allows, or `Automatic` when it has none.
pub(super) fn compaction_reasoning(
    reasoning: &ReasoningCapability,
    setting: &str,
) -> ReasoningRequest {
    if !reasoning.supported {
        return ReasoningRequest::Automatic;
    }
    let setting = setting.trim().to_ascii_lowercase();
    match setting.as_str() {
        "off" | "lowest" if reasoning.can_disable => ReasoningRequest::Disabled,
        "lowest" => reasoning
            .efforts
            .iter()
            .next()
            .copied()
            .map_or(ReasoningRequest::Automatic, ReasoningRequest::Effort),
        "minimal" => ReasoningRequest::Effort(ReasoningEffort::Minimal),
        "low" => ReasoningRequest::Effort(ReasoningEffort::Low),
        "medium" => ReasoningRequest::Effort(ReasoningEffort::Medium),
        "high" => ReasoningRequest::Effort(ReasoningEffort::High),
        _ => ReasoningRequest::Automatic,
    }
}

/// Whether the next request is over the compaction threshold.
///
/// The service updates it from the provider-reported usage as each request is
/// settled; the layer reads it just before the next request. It is shared by
/// every engine built for the turn and dropped after an eager compaction.
pub(super) struct CompactionGate {
    max_context: u64,
    threshold: f64,
    keep_recent: usize,
    due: AtomicBool,
}

impl CompactionGate {
    pub(super) fn new(max_context: u64, threshold: f64, keep_recent: usize) -> Self {
        Self {
            max_context,
            threshold,
            keep_recent,
            due: AtomicBool::new(false),
        }
    }

    #[allow(clippy::cast_precision_loss)]
    pub(super) fn update(&self, used: u64) {
        let due = used as f64 >= self.max_context as f64 * self.threshold;
        self.due.store(due, Ordering::Relaxed);
    }

    pub(super) fn due(&self) -> bool {
        self.due.load(Ordering::Relaxed)
    }
}

/// Aborts a model request at a safe point instead of sending it, so the
/// service can compact the idle session and restart the turn on the new one.
pub(super) struct CompactionLayer {
    pub(super) gate: Arc<CompactionGate>,
}

#[async_trait]
impl Layer for CompactionLayer {
    fn name(&self) -> &'static str {
        "compaction"
    }

    async fn wrap(
        &self,
        context: &LayerContext<'_>,
        request: ModelRequest,
        next: &dyn Continuation,
    ) -> norte::Result<ModelStream> {
        if self.gate.due() && request.messages.len() > self.gate.keep_recent.saturating_add(1) {
            let detail = format!(
                "used context reached the {} threshold of the model window; \
                 compacting before the next request",
                self.gate.threshold
            );
            context.short_circuit(
                "compaction",
                ShortCircuitReason::Custom {
                    name: "compaction_due".to_owned(),
                    detail: detail.clone(),
                },
            );
            return Err(norte::Error::InvalidInput(format!(
                "{COMPACTION_SAFE_POINT}: {detail}"
            )));
        }
        next.call(context, request).await
    }
}

/// Whether an engine error is the compaction layer stopping before a request.
pub(super) fn is_compaction_abort(event: &EngineEvent) -> bool {
    matches!(
        event,
        EngineEvent::Error { message, .. } if is_compaction_message(message)
    )
}

pub(super) fn is_compaction_message(message: &str) -> bool {
    message.contains(COMPACTION_SAFE_POINT)
}
