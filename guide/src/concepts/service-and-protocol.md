# The service and the wire

## One process, two halves

Norte Code is split into a service library and clients:

```text
CLI / your program
        │
        │  ClientRequest { request_id, kind }
        ▼
norte-code-protocol
        │
        ▼
Service  ──►  Norte engine  ──►  provider
        │
        │  ServerEvent stream
        ▼
CLI / your program
```

`Service` lives in `norte-code-agent`. It owns the engines, the logical session
map, the agent loop, approvals, compaction, budgets, telemetry and the SQLite
directory. It is **not** a daemon: there is no listener, socket or HTTP route in
v1. The CLI builds a `Service` with `Service::with_directory` and then calls:

```rust,ignore
let stream = service
    .submit(ClientRequest {
        request_id: 1,
        kind: ClientRequestKind::SendMessage {
            session: None,
            text: "Explain src/lib.rs".to_owned(),
            budget: None,
            reasoning: None,
        },
    })
    .await?;

while let Some(event) = stream.next().await {
    // ServerEvent { request_id, session, kind }
}
```

`submit` returns `EventStream`, a boxed `Stream<Item = ServerEvent>`. Some
requests answer with one event; a turn emits many, ending in `TurnFinished` or
`Error`.

## Why the wire exists

The protocol crate has no Norte dependency. That is deliberate:

- Clients do not compile the engine or learn its types.
- Norte IDs stay behind the service boundary. A client sees a `SessionKey`, not
  a Norte `SessionId`.
- The protocol can be versioned independently. `PROTOCOL_VERSION` is `1`, and
  request and event frames deny unknown fields.
- The same events can feed the CLI today and a remote transport later
  without a second mapping.

The service maps Norte events and records into protocol events in one place.
Clients never receive a raw engine event.

## Request and event shape

Every request has a client-chosen `request_id`; every event echoes it. An event
that concerns a session also carries its `SessionKey`.

```json
{"request_id":7,"kind":{"type":"send_message","session":null,"text":"hi"}}
```

```json
{"request_id":7,"session":"…","kind":{"event":"turn_started","turn":1}}
```

The full request and event vocabulary is in the [protocol reference](../reference/protocol.md).
The important categories are:

| Category | Examples |
| --- | --- |
| Session lifecycle | `SessionCreated`, `SessionList`, `SessionForked`, `SessionDeleted` |
| A turn | `TurnStarted`, `Delta`, `Committed`, `TurnFinished` |
| Tool activity | `ToolStarted`, `ToolFinished` |
| Questions | `QuestionAsked`, `QuestionAnswered` |
| Accounting | `RequestReconstructed`, `CostUpdated`, `BudgetWarning`, `BudgetExceeded` |
| Control | `ModelChanged`, `RetryScheduled`, `Error` |

## What is deliberately not here

There is no `Service::start_server` and no background daemon lifecycle. An
in-process service keeps approval and question state in the same process as the
terminal that answers it, and the SQLite directory is the durability boundary.
If a remote transport is added later, it should wrap this request/event API
rather than grow a second agent implementation.
