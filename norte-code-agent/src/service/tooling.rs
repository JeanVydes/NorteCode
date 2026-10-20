//! Tool activity capture and the executor wrappers the host registers.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use norte::{CallId, FnExecutor, ToolArgs, ToolContext, ToolDeclaration, ToolOutput};

/// Live arguments and results for tool calls in flight.
///
/// Norte's engine events carry no output, and a result is not in history until
/// the turn commits, so the host records both sides around the executor. The
/// renderer gets the effective arguments and the text the model will see.
#[derive(Default)]
pub(super) struct ToolActivity {
    pub(super) arguments: Mutex<BTreeMap<CallId, serde_json::Value>>,
    outputs: Mutex<BTreeMap<CallId, ToolOutput>>,
}

impl ToolActivity {
    pub(super) fn started(&self, call_id: &CallId) -> Option<serde_json::Value> {
        self.arguments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(call_id)
    }

    pub(super) fn finished(&self, call_id: &CallId) -> Option<ToolOutput> {
        self.outputs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(call_id)
    }
}

/// A tool executor that keeps the output for the renderer.
struct RecordingExecutor<A> {
    inner: Arc<dyn FnExecutor<A, ToolOutput>>,
    activity: Arc<ToolActivity>,
}

#[async_trait]
impl<A> FnExecutor<A, ToolOutput> for RecordingExecutor<A>
where
    A: ToolArgs,
{
    async fn call(&self, args: A) -> norte::Result<ToolOutput> {
        self.inner.call(args).await
    }

    async fn call_with_context(&self, context: &ToolContext, args: A) -> norte::Result<ToolOutput> {
        let result = self.inner.call_with_context(context, args).await;
        if let Some(call_id) = context.call_id {
            let output = match &result {
                Ok(output) => output.clone(),
                Err(error) => ToolOutput::error(error.to_string()),
            };
            self.activity
                .outputs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(call_id, output);
        }
        result
    }
}

/// Wraps a rich tool declaration so its output is visible to the host.
pub(super) fn recording_tool<A>(
    declaration: ToolDeclaration<A>,
    activity: Arc<ToolActivity>,
) -> ToolDeclaration<A>
where
    A: ToolArgs + serde::de::DeserializeOwned + std::fmt::Debug + 'static,
{
    ToolDeclaration {
        name: declaration.name,
        description: declaration.description,
        parameters: declaration.parameters,
        executor: Arc::new(RecordingExecutor {
            inner: declaration.executor,
            activity,
        }),
        approval: declaration.approval,
    }
}

/// The text a tool result carries, with structured output as a fallback.
pub(super) fn output_text(output: &ToolOutput) -> Option<String> {
    let text = output
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        output.structured.as_ref().map(ToString::to_string)
    } else {
        Some(text)
    }
}
