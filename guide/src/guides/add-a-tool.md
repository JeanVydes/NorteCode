# Add a host tool

This is a repository guide. Norte Code has no plugin registry; a host tool is a
crate change plus a registration in the service.

## 1. Implement the tool

Put the implementation in `norte-code-tools` (or a new workspace crate) and
follow the conventions of the existing tools:

- strict arguments with `#[serde(deny_unknown_fields)]`;
- a `JsonSchema` argument type;
- errors returned as `ToolOutput::error` for user mistakes, not engine errors;
- bounded output;
- no path escape through `..` or a symlink.

The shape is:

```rust,ignore
use std::sync::Arc;

use async_trait::async_trait;
use norte::{FnExecutor, Result, ToolDeclaration, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WordCountArgs {
    /// The text to count.
    pub text: String,
}

pub struct WordCount;

impl WordCount {
    pub fn into_declaration(self) -> ToolDeclaration<WordCountArgs> {
        ToolDeclaration::new(
            "word_count",
            "Count the words in text. Example: {\"text\": \"the sky is blue\"}.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<WordCountArgs, ToolOutput> for WordCount {
    async fn call(&self, args: WordCountArgs) -> Result<ToolOutput> {
        Ok(ToolOutput::json(serde_json::json!({
            "words": args.text.split_whitespace().count(),
        })))
    }
}
```

Write the description for the model: say what the arguments are, give one
example object, and keep internal references out of it.

## 2. Register it

`Service::tools` rebuilds the registry for every engine. Add the declaration
there:

```rust,ignore
self.register_tool(&mut tools, WordCount.into_declaration())?;
```

`register_tool` wraps the executor so the renderer can recover the effective
arguments and the text the model will see.

## 3. Decide the policy

Read-only tools normally need no approval. Effectful tools need more:

- Add the tool name to `JOURNALED_TOOLS` in `service/effects.rs` if a crash could
  repeat a side effect.
- Make sure the approval policy asks for it, or document why it is safe to run
  unattended.
- Resolve paths through the workspace policy; do not pass raw model paths to the
  filesystem.
- Register any secrets with `with_secrets` if the tool can print them.

## 4. Test it

Use `norte-testing`'s `ScriptedModel` to produce the tool call and an in-memory
store. Assert both the tool output and the request the model saw:

```rust,ignore
let model = Arc::new(
    ScriptedModel::new(reference)
        .turn_tool_call(call_id, "word_count", &json!({"text": "the sky is blue"}))
        .turn_text("Three words."),
);
```

The service tests in `norte-code-agent/src/service/tests.rs` are the best
examples of the full wiring: catalogue, config, model factory, in-memory store and
event drain.

## 5. Document it

Add the tool to [Tools and approvals](../concepts/tools-and-approvals.md) and, if
it has a user-facing switch, to the [configuration reference](../reference/configuration.md).
