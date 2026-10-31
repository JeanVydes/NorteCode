# Protocol

`norte-code-protocol` is the client wire. It is versioned serde JSON with
`PROTOCOL_VERSION = 1`; request and event frames use `deny_unknown_fields`.
Nothing in this crate depends on Norte.

## Frames

```rust
pub struct ClientRequest {
    pub request_id: u64,
    pub kind: ClientRequestKind,
}

pub struct ServerEvent {
    pub request_id: u64,
    pub session: Option<SessionKey>,
    pub kind: ServerEventKind,
}
```

`request_id` is chosen by the client and echoed on every answer. `session` is
present when the event concerns a logical session. A `SessionKey` is Norte
Code's stable id, not a Norte `SessionId`.

## Requests

| Request | Purpose |
| --- | --- |
| `CreateSession { title }` | Create a logical session. |
| `ListSessions` | List known sessions. |
| `SwitchSession { session }` | Make a session current for the caller. |
| `ForkSession { session, title }` | Copy a session's history. |
| `RenameSession { session, title }` | Rename a session. |
| `DeleteSession { session }` | Delete a session. |
| `SendMessage { session, text, budget, reasoning }` | Start a turn; creates a session when `session` is `None`. |
| `Steer { session, text, id }` | Steer a message into the running turn. The name is optional. |
| `CancelTurn { session }` | Ask the running turn to stop. |
| `Resume { session }` | Continue a parked or interrupted session. |
| `AnswerQuestion { session, question, answer }` | Answer a parked question. |
| `SetModel { session, model }` | Switch the model at a turn boundary. |
| `SetReasoning { session, scope, reasoning }` | Set reasoning for the session or one message. |
| `SetBudget { session, budget }` | Set the session budget. |
| `Compact { session }` | Compact now. |
| `ListModels` | List catalogue models. |
| `RefreshCatalog` | Refresh the catalogue. |
| `ListSkills` | List installed skills. |
| `RefreshSkills` | Re-scan the skill roots. |
| `CostReport { session }` | Report ledger totals, optionally for one session. |

`SetReasoning` validates the effort or budget against the session's current model
and persists it; `session` applies to every later turn until changed, while
`message` applies to the next turn only. `Steer` persists a message and hands it
to the running turn, which appends it to the history before its next request.
Neither request emits an event on its own, so an empty response is the
acknowledgement.

`SetModel` applies the switch right away when the session is idle and answers
with `ModelChanged`. When the session is busy it is accepted, persisted and
applied at the next turn boundary; the answer is `ModelQueued` and the switch
emits `ModelChanged` when it is applied.

`Steer` reaches the request the session is already making. The message is
persisted and the engine appends it to the history before the next request of
the running turn, so the model sees it in that request. The client may name the
message; the engine reports it with `SteerApplied`, which carries the names it
applied. Whatever the turn never reached waits for the next turn boundary,
where the service delivers it.

A host that registers its own interceptor may end a turn on purpose. The engine
reports it with `TurnStopped`, which carries the reason; the turn ends without
committing and the client reads it as a decision, not as a failure.

## Events

| Event | Purpose |
| --- | --- |
| `SessionCreated`, `SessionList`, `SessionDeleted`, `SessionRenamed`, `SessionForked`, `SessionSwitched` | Session lifecycle. |
| `TurnStarted` | A turn started. |
| `Delta { turn, delta }` | Text, reasoning or refusal delta. |
| `ToolStarted`, `ToolFinished` | Tool activity, arguments, output, error and duration. |
| `RequestReconstructed` | Usage, reported cost, timing, cache, truncation and tools for one request. |
| `Committed` | A commit landed at `version`. |
| `CompactionStarted`, `CompactionFinished` | Compaction progress and the new Norte session id. |
| `BudgetWarning`, `BudgetExceeded`, `BudgetChanged` | Budget crossing and changes. |
| `CostUpdated`, `CostNotReported` | Ledger totals and unpriced requests. |
| `ModelChanged`, `ModelQueued`, `ReasoningDowngraded` | Model switches. `ModelQueued` says the switch is accepted and pending a turn boundary. The third is wire vocabulary for a host that clamps; this service refuses an incompatible setting instead. |
| `SteerApplied` | The engine applied steering messages to the request already in flight, naming the ones it applied. |
| `TurnStopped` | The host ended the turn on purpose, with a reason. |
| `CatalogUpdated`, `ModelList` | Catalogue refresh and model list. |
| `SkillList`, `SkillsUpdated` | Skill discovery and refresh. |
| `QuestionAsked`, `QuestionAnswered` | A parked question and its answer. |
| `RetryScheduled` | A transient failure will be retried on the same session. |
| `TurnFinished` | The turn ended with a phase and a `committed` flag. |
| `Error` | A terminal failure. |

## Deltas

`Delta` is tagged by `delta`:

- `text { index, text }`
- `reasoning { index, text }`
- `refusal { index, text }`

Append text deltas in arrival order. Do not assume a single delta contains the
whole answer.

## Stability

New optional fields use `#[serde(default, skip_serializing_if = "Option::is_none")]`
so older clients keep working. A breaking change to the shape bumps
`PROTOCOL_VERSION`. The service maps Norte types once; clients never see a Norte
id or event.

The Rust types, including every field and doc comment, are the authority. This
page is an index, not a second definition.
