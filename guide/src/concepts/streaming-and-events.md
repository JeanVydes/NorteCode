# Streaming and events

## The event stream

`Service::submit` returns an `EventStream`. A turn normally emits:

1. `TurnStarted`
2. zero or more `Delta` events (`text`, `reasoning` or `refusal`)
3. `ToolStarted` / `ToolFinished` around each call
4. `RequestReconstructed` after each provider request, with usage, reported
   cost, timing and the tools that request called
5. `Committed` when history advances
6. `TurnFinished` with the phase and whether an assistant message was committed

Errors are terminal unless the host schedules a retry. `RetryScheduled` is
followed by another attempt on the same session; no `TurnFinished` is emitted
between them.

## What each event is for

| Event | Use |
| --- | --- |
| `Delta` | Render tokens as they arrive. Text, reasoning and refusal are separate. |
| `ToolStarted` | Show the tool name and, when recoverable, the effective arguments. |
| `ToolFinished` | Show success, the text the model saw, truncation and duration. |
| `ApprovalRequested` | Print the tool, its arguments and the sandbox badge, then wait. |
| `QuestionAsked` | Ask the human, including any closed options. |
| `RequestReconstructed` | Show tokens, cache, latency, token rate, cost and tools. |
| `BudgetWarning` / `BudgetExceeded` | Tell the user a limit was crossed and what the host did. |
| `CostUpdated` / `CostNotReported` | Update a running ledger view without guessing. |
| `RetryScheduled` | Explain a transient failure and the wait before the next attempt. |
| `Error` | A terminal failure for this request. |

## Ordering and identity

Every event echoes the request id. A turn's events also carry the session key.
The client should be prepared for several turns in the same request stream when a
turn parks and resumes, and for approval or question events to arrive between
the tool events.

A `Committed` event means Norte accepted the commit at a new version. A
`TurnFinished` with `committed: false` means the turn ended without an assistant
message; this is normal when a turn ends in a tool call or is cancelled.

## JSONL

The CLI's `--jsonl` mode writes one serialized `ServerEvent` per line to stdout
and nothing else. Diagnostics stay on stderr. See
[Script with JSONL](../guides/script-with-jsonl.md).

## Cancellation and streams

Dropping an `EventStream` does not by itself promise that the engine task has
finished; use `CancelTurn` when the client wants a turn stopped. Ctrl-C in the
CLI does exactly that and then waits for the turn to settle before exiting.
