# Test without a network

## The rule

Hermetic tests never call a provider. Norte Code uses Norte's testing crate:

- `ScriptedModel` pops a queued turn and records the requests it received.
- `ProbeModel` records requests while delegating or returning scripted answers.
- `RecordingObserver` collects engine events.

The service tests build a real `Service` with an in-memory store, an in-memory
blob store and a `ScriptedFactory` that returns the scripted model. The model id
still resolves through the catalogue, so capability checks are real.

## A scripted turn

```rust,ignore
use std::sync::Arc;

use norte_testing::ScriptedModel;
use serde_json::json;

let model = Arc::new(
    ScriptedModel::new(reference)
        .turn_tool_call(call_id, "read", &json!({"path": "src/lib.rs"}))
        .turn_text("The crate exposes the service."),
);
```

The first request asks for a tool; the second answers with text. Assert the
number of requests and the tool names the model saw:

```rust,ignore
assert_eq!(model.request_count(), 2);
```

## In-memory stores

Use `InMemoryStore` and `InMemoryBlobStore` for turn logic. Use a temporary
SQLite file when the test is about durability, sessions, effects or
telemetry; `SqliteStore::in_memory` is enough for most of those.

## What to assert

- The event order that matters: `TurnStarted`, tool events, `Committed`,
  `TurnFinished`.
- The final text, concatenated from `Delta` events.
- Usage and reported cost: missing cost must stay missing, not become zero.
- Budget stop: the turn ends uncommitted and the session is resumable.
- Compaction rollback: a failed summary leaves the old session active.

## Live tests

A test that needs a real provider is `#[ignore]`d and key-gated. Do not make the
default suite depend on a network or on an API key. The CI job runs the hermetic
suite; a live probe is an explicit local action.

## Running

```sh
cargo test -p norte-code-agent
cargo test -p norte-code-cli
cargo test --workspace
```

The workspace suite is broad; run a single test with its full path when
debugging:

```sh
cargo test -p norte-code-agent a_turn_streams_text_and_commits -- --nocapture
```
