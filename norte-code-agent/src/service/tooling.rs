//! Tool activity capture and the executor wrappers the host registers.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use norte::{AnyFunction, CallId, FnExecutor, ToolArgs, ToolContext, ToolDeclaration, ToolOutput};
use norte_code_plugin_api::HostEffects;
use norte_code_wire::{PluginId, PluginMessageRejection};

use super::plugin_messages::{PluginMessagePolicy, PluginMessages};

/// The largest file the host snapshots for an edit preview.
const EDIT_MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Live arguments and results for tool calls in flight.
///
/// Norte's engine events carry no output, and a result is not in history until
/// the turn commits, so the host records both sides around the executor. The
/// renderer gets the effective arguments and the text the model will see.
#[derive(Debug, Default)]
pub(super) struct ToolActivity {
    pub(super) arguments: Mutex<BTreeMap<CallId, serde_json::Value>>,
    /// Files captured immediately before an `edit` runs, for the CLI preview.
    sources: Mutex<BTreeMap<CallId, String>>,
    outputs: Mutex<BTreeMap<CallId, ToolOutput>>,
}

impl ToolActivity {
    pub(super) fn started(&self, call_id: &CallId) -> Option<serde_json::Value> {
        self.arguments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(call_id)
    }

    pub(super) fn source(&self, call_id: &CallId) -> Option<String> {
        self.sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(call_id)
    }

    pub(super) fn snapshot(&self, call_id: CallId, source: String) {
        self.sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(call_id, source);
    }

    /// Records the reason a rule refused a call.
    ///
    /// A refused call never reaches an executor, so nothing else writes its
    /// error down; without this the failure a renderer is told about would
    /// arrive without the reason the visitor needs. The engine reports the
    /// call as finished right after the interceptor returns, and that event is
    /// where this is read.
    pub(super) fn refused(&self, call_id: CallId, reason: &str) {
        self.outputs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(call_id, ToolOutput::error(reason.to_owned()));
    }

    pub(super) fn finished(&self, call_id: &CallId) -> Option<ToolOutput> {
        self.outputs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(call_id)
    }
}

/// Captures the text an `edit` is about to change, before the executor runs.
///
/// The client renders `ToolStarted` asynchronously, so reading the file then
/// can race with a fast edit. The host snapshots it at the metadata callback
/// instead, which Norte invokes immediately before execution. Only a regular
/// file inside the workspace and under the preview limit is read; anything
/// else is left without a preview rather than reading an arbitrary path.
pub(super) fn edit_source(root: &Path, arguments: &serde_json::Value) -> Option<String> {
    let path = arguments.get("path")?.as_str()?;
    let root = root.canonicalize().ok()?;
    let candidate = Path::new(path);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let candidate = candidate.canonicalize().ok()?;
    if !candidate.starts_with(&root) {
        return None;
    }
    // Check before opening: opening a FIFO can block, and the edit tool would
    // reject it anyway.
    let metadata = std::fs::metadata(&candidate).ok()?;
    if !metadata.is_file() || metadata.len() > EDIT_MAX_FILE_BYTES {
        return None;
    }
    let file = std::fs::File::open(candidate).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > EDIT_MAX_FILE_BYTES {
        return None;
    }
    let mut text = String::new();
    file.take(EDIT_MAX_FILE_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
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
    }
}

/// An erased tool whose output the host records for the renderer.
///
/// Plugin tools arrive as `Arc<dyn AnyFunction>`, so the typed
/// [`recording_tool`] cannot wrap them. This mirrors it on the erased surface:
/// every call delegates to the plugin and the result is kept under the call id
/// until the engine's `ToolFinished` event is rendered.
pub(super) struct RecordingFunction {
    inner: Arc<dyn AnyFunction>,
    activity: Arc<ToolActivity>,
    messages: Arc<PluginMessages>,
    plugin_id: PluginId,
    policy: PluginMessagePolicy,
}

impl RecordingFunction {
    pub(super) fn new(
        inner: Arc<dyn AnyFunction>,
        activity: Arc<ToolActivity>,
        messages: Arc<PluginMessages>,
        plugin_id: PluginId,
        policy: PluginMessagePolicy,
    ) -> Self {
        Self {
            inner,
            activity,
            messages,
            plugin_id,
            policy,
        }
    }
}

impl fmt::Debug for RecordingFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordingFunction")
            .field("name", &self.inner.name())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl AnyFunction for RecordingFunction {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> &schemars::Schema {
        self.inner.parameters_schema()
    }

    fn kind(&self) -> norte::ToolKind {
        self.inner.kind()
    }

    fn declaration(&self) -> Option<serde_json::Value> {
        self.inner.declaration()
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: &serde_json::Value,
    ) -> norte::Result<ToolOutput> {
        let mut result = self.inner.execute(context, arguments).await;
        let effects = match &mut result {
            Ok(output) => match HostEffects::extract(output) {
                Ok(effects) => effects,
                Err(error) => {
                    tracing::warn!(plugin_id = %self.plugin_id, %error, "a plugin tool returned invalid host effects");
                    if let (Some(session), Some(call_id)) = (context.session, context.call_id) {
                        self.messages.reject(
                            session,
                            &self.plugin_id,
                            call_id,
                            PluginMessageRejection::InvalidEnvelope,
                        );
                    }
                    None
                }
            },
            Err(_) => None,
        };
        if let (Some(session), Some(call_id)) = (context.session, context.call_id)
            && let Some(effects) = effects
        {
            self.messages.enqueue(
                session,
                &self.plugin_id,
                call_id,
                vec![effects],
                &self.policy,
            );
        }
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

#[cfg(test)]
#[path = "tooling_tests.rs"]
mod tests;
