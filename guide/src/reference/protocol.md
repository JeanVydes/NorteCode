# Protocol

The request/event wire a client speaks. Versioned serde JSON
(`PROTOCOL_VERSION = 1`); frames use `deny_unknown_fields`. The Rust types
are the authority — this page is an index, not a second definition.

## Frames

```rust
pub struct ClientRequest {
    pub request_id: u64,        // chosen by the client, echoed on every answer
    pub kind: ClientRequestKind,
}

pub struct ServerEvent {
    pub request_id: u64,
    pub session: Option<SessionKey>,  // the session's stable key, when relevant
    pub kind: ServerEventKind,
}
```

## Requests

| Request | Purpose |
| --- | --- |
| `CreateSession { title }` | Create a logical session. |
| `ListSessions` | List known sessions. |
| `SwitchSession { session }` | Make a session current for the caller. |
| `ForkSession { session, title }` | Copy a session's history. |
| `RenameSession { session, title }` | Rename a session. |
| `DeleteSession { session }` | Delete a session. |
| `SendMessage { session, text, attachments, budget, reasoning, replay }` | Start a turn; creates a session when `session` is `None`. |
| `Steer { session, text, id }` | Queue a message for the next turn boundary. |
| `CancelTurn { session }` | Ask the running turn to stop. |
| `Resume { session }` | Continue a parked or interrupted session. |
| `AnswerQuestion { session, question, answer }` | Answer a parked question. |
| `SetModel { session, provider, model }` | Switch the model at a turn boundary. |
| `SetReasoning { session, scope, reasoning }` | Set reasoning for the session or one message. |
| `SetBudget { session, budget }` | Set the session budget. |
| `Compact { session }` | Compact now. |
| `StopJobs { session, id }` | Stop a session's background jobs (one, or all). |
| `DetachTurn { session }` | Stop waiting for the running call; it keeps running as a job. |
| `ListJobs { session }` | List a session's background jobs. |
| `ListModels` | List catalogue models. |
| `RefreshCatalog` | Re-read the catalogue files (nothing is fetched). |
| `ListSkills` | List installed skills. |
| `RefreshSkills` | Re-scan the skill roots. |
| `CostReport { session }` | Ledger totals, optionally for one session. |

`SetReasoning` validates against the session's current model; `session`
scope applies to every later turn, `message` to the next only. `Steer` and
`SetReasoning` emit no event of their own — an empty response is the
acknowledgement. `SetModel` answers `ModelChanged` immediately when idle,
or `ModelQueued` when busy (applied at the next turn boundary).

## Events

A turn normally emits `TurnStarted`, zero or more `Delta`s, `ToolStarted` /
`ToolFinished` around each call, `RequestReconstructed` per provider
request, `Committed`, then `TurnFinished`.

| Event | Purpose |
| --- | --- |
| `SessionCreated`, `SessionList`, `SessionDeleted`, `SessionRenamed`, `SessionForked`, `SessionSwitched` | Session lifecycle. |
| `TurnStarted`, `TurnFinished { phase, committed }` | Turn bounds. |
| `Delta { turn, delta }` | `text`, `reasoning` or `refusal` chunk. Append in arrival order. |
| `ToolStarted`, `ToolFinished`, `ToolOutput` | Tool activity, live output chunks, result, error, duration. |
| `RequestReconstructed` | Usage, reported cost, timing, cache, truncation, tools. |
| `Committed { version }` | A commit landed. |
| `MessageReplayed`, `SessionTodos`, `SessionStats` | Resume prefix: past messages, todo list, accumulated usage. |
| `CompactionStarted`, `CompactionFinished` | Compaction progress, summary and new session id. |
| `BudgetWarning`, `BudgetExceeded`, `BudgetChanged` | Budget crossings and changes. |
| `CostUpdated`, `CostNotReported` | Ledger totals and unpriced requests. |
| `ModelChanged`, `ModelQueued` | Model switches (immediate or queued). |
| `SteerApplied { ids, messages }` | Steering messages that reached the in-flight request. |
| `TurnStopped { reason }` | The host ended the turn on purpose — a decision, not a failure. |
| `JobList`, `JobsStopped` | Background job listing and stop counts. |
| `CatalogUpdated`, `ModelList` | Catalogue refresh and model list. |
| `SkillList`, `SkillsUpdated` | Skill discovery and refresh. |
| `QuestionAsked`, `QuestionAnswered` | A parked question and its answer. |
| `RetryScheduled` | A transient failure will be retried. |
| `Error` | A terminal failure. |

## Stability

New optional fields use `#[serde(default, skip_serializing_if =
"Option::is_none")]` so older clients keep working. A breaking shape change
bumps `PROTOCOL_VERSION`.
