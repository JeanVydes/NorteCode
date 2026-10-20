# Compaction

## Why it exists

Long sessions eventually exceed the model's context window. Compaction replaces
the transcript with a summary plus the most recent messages, while keeping the
same logical session and its local history of decisions.

## When it runs

`compaction.threshold` is a fraction of the model's context window, default
`0.90`. The trigger is based on the last provider-reported prompt plus answer
tokens.

`compaction.trigger` selects the moment:

| Trigger | When |
| --- | --- |
| `idle` (default) | At an idle turn boundary, and before the next turn when a session is resumed or switched to a smaller model. |
| `eager` | At the first safe model-request boundary inside a turn, after a complete tool result. |

Eager compaction never runs while an approval is pending, never splits a tool
call from its results, and runs at most once per turn.

`/compact` in the interactive prompt and `ClientRequest::Compact` use the same
path on demand.

## What happens

1. Read the old Norte history and the current todo list.
2. Ask a model for a structured summary. The summary request is a normal
   provider request with the configured budget and telemetry; it has no tools.
3. Create a fresh Norte session and seed it with the summary, the todo list and
   the last `keep_recent` messages (default `6`).
4. Point the logical session at the fresh session. The old Norte session stays
   in the store and is linked through `compacted_from`.
5. Emit `CompactionFinished` with the new Norte session id.

The summary model defaults to the session model. `compaction.model` and
`compaction.reasoning` can override it; the reasoning default is `lowest`.

## Safety invariants

- A suffix never starts with a tool result whose tool call was dropped.
- A failed summary leaves the old history, the active session id and the phase
  unchanged.
- The summary's reported cost enters the ledger and counts toward budgets.
- The logical session key, title and approval rules survive.
- A model without a catalogue context window does not compact automatically.
- A concurrent turn does not repeat an in-flight compaction.

`CompactionFinished { compacted: false, error: … }` is the visible failure shape;
the session remains usable.

## Cost

The summary is billed like any other request. If a budget stops the summary
request, the compaction does not land and the old history stays active. This is
intentional: a budget must not be able to leave a half-written transcript.
