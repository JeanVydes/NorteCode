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
| `Steer { session, text }` | Queue a message for the next turn boundary. |
| `CancelTurn { session }` | Ask the running turn to stop. |
| `Resume { session }` | Continue a parked or interrupted session. |
| `Approve { session, approval, arguments, scope, remember }` | Approve a parked call. |
| `Deny { session, approval, reason, scope }` | Deny a parked call. |
| `Revoke { session, approval, grant }` | Revoke a live grant. |
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
`message` applies to the next turn only. `Steer` persists a message and applies it
when the next turn starts, before that turn's own input; it does not interrupt a
turn already in flight. Neither request emits an event, so an empty response is
the acknowledgement.

## Events

| Event | Purpose |
| --- | --- |
| `SessionCreated`, `SessionList`, `SessionDeleted`, `SessionRenamed`, `SessionForked`, `SessionSwitched` | Session lifecycle. |
| `TurnStarted` | A turn started. |
| `Delta { turn, delta }` | Text, reasoning or refusal delta. |
| `ToolStarted`, `ToolFinished` | Tool activity, arguments, output, error and duration. |
| `ApprovalRequested`, `ApprovalResolved` | A parked decision and its outcome. |
| `RequestReconstructed` | Usage, reported cost, timing, cache, truncation and tools for one request. |
| `Committed` | A commit landed at `version`. |
| `CompactionStarted`, `CompactionFinished` | Compaction progress and the new Norte session id. |
| `BudgetWarning`, `BudgetExceeded`, `BudgetChanged` | Budget crossing and changes. |
| `CostUpdated`, `CostNotReported` | Ledger totals and unpriced requests. |
| `ModelChanged`, `ReasoningDowngraded` | Model switches. The second is wire vocabulary for a host that clamps; this service refuses an incompatible setting instead. |
| `CatalogUpdated`, `ModelList` | Catalogue refresh and model list. |
| `SkillList`, `SkillsUpdated` | Skill discovery and refresh. |
| `GrantRevoked` | A grant was revoked. |
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
