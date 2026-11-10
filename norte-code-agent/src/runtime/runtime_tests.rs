use super::*;
use norte::tool::ToolStream;

/// A tool that reports output while it runs, then returns a result.
#[norte::tool(name = "streaming", description = "Reports two chunks, then returns.")]
#[derive(Clone)]
struct Streaming;

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct StreamingArgs {}

#[async_trait::async_trait]
impl norte::FnExecutor<StreamingArgs, ToolOutput> for Streaming {
    async fn call(&self, _args: StreamingArgs) -> Result<ToolOutput> {
        Ok(ToolOutput::text("done"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        _args: StreamingArgs,
    ) -> Result<ToolOutput> {
        for chunk in [b"first".to_vec(), b"second".to_vec()] {
            context.emit(ToolProgress {
                stream: ToolStream::Stdout,
                bytes: chunk,
                last: false,
            });
        }
        context.emit(ToolProgress {
            stream: ToolStream::Stdout,
            bytes: Vec::new(),
            last: true,
        });
        Ok(ToolOutput::text("done"))
    }
}

/// A tool sees a progress sink, and what it reports arrives as the engine's
/// own events, in order, between the call's lifecycle events.
#[tokio::test]
async fn a_streaming_tool_reports_through_the_engine() {
    let name = ToolName::new("streaming").expect("tool name");
    let mut registry = ToolRegistry::new();
    registry.register_tool(&Streaming).expect("register");
    let tools = Arc::new(registry);

    let (sender, mut events) = mpsc::channel(32);
    let site = CallSite {
        session: SessionId::new(),
        turn: 1,
    };
    let prepared = PreparedCall {
        call: ToolCallPart {
            call_id: CallId::new(),
            name: name.as_str().to_owned(),
            arguments: serde_json::json!({}),
            continuity: None,
        },
        tool: name,
        arguments: serde_json::json!({}),
        metadata: BTreeMap::new(),
    };

    let result = execute_call(
        tools,
        Arc::new(ObserverSet::new(Vec::new(), 16)),
        sender,
        site,
        prepared,
        CancellationToken::new(),
    )
    .await;
    assert!(!result.is_error, "the tool succeeded");

    let mut kinds = Vec::new();
    let mut chunks = Vec::new();
    while let Some(event) = events.recv().await {
        match event {
            EngineEvent::ToolOutput { bytes, last, .. } => {
                kinds.push("output");
                chunks.push((bytes, last));
            }
            EngineEvent::ToolCallStarted { .. } => kinds.push("started"),
            EngineEvent::ToolCallFinished { .. } => kinds.push("finished"),
            _ => {}
        }
    }
    assert_eq!(
        kinds,
        ["started", "output", "output", "output", "finished"],
        "the chunks arrive between the call's own events"
    );
    assert_eq!(chunks[0], (b"first".to_vec(), false));
    assert_eq!(chunks[1], (b"second".to_vec(), false));
    assert!(chunks[2].1, "the last chunk closes the stream");
}
