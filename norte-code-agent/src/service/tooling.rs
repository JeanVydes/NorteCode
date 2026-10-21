//! Tool activity capture and the executor wrappers the host registers.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use norte::{CallId, FnExecutor, ToolArgs, ToolContext, ToolDeclaration, ToolOutput};

/// The largest file the host snapshots for an edit preview.
const EDIT_MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Live arguments and results for tool calls in flight.
///
/// Norte's engine events carry no output, and a result is not in history until
/// the turn commits, so the host records both sides around the executor. The
/// renderer gets the effective arguments and the text the model will see.
#[derive(Default)]
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
        const MAX_SNAPSHOTS: usize = 16;
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        // A denied or cancelled approval never reaches `ToolStarted`, so its
        // snapshot is not removed by the renderer. Bound the map instead of
        // letting repeated denials retain megabytes of file text.
        if sources.len() >= MAX_SNAPSHOTS {
            if let Some(oldest) = sources.keys().next().copied() {
                sources.remove(&oldest);
            }
        }
        sources.insert(call_id, source);
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

#[cfg(test)]
mod tests {
    use super::edit_source;

    #[test]
    fn edit_source_reads_only_regular_files_inside_the_workspace() {
        let root = std::env::temp_dir().join(format!("norte-edit-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp root");
        let inside = root.join("note.txt");
        std::fs::write(&inside, "one\ntwo\n").expect("temp file");

        let arguments = serde_json::json!({"path": "note.txt"});
        assert_eq!(
            edit_source(&root, &arguments).as_deref(),
            Some("one\ntwo\n")
        );

        let outside =
            std::env::temp_dir().join(format!("norte-edit-outside-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&outside, "secret").expect("outside file");
        let arguments = serde_json::json!({"path": outside});
        assert!(edit_source(&root, &arguments).is_none());

        let arguments = serde_json::json!({"path": ".."});
        assert!(edit_source(&root, &arguments).is_none());

        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}
