# Sessions and turns

## The logical session

A client sees a `SessionKey`: a stable string chosen by Norte Code. Behind it,
the service keeps a `SessionRecord` with:

- the current Norte `SessionId`;
- the model specification;
- the phase, title and creation time;
- the session budget and the session's allow/deny tool rules;
- the session reasoning override and any one-message override or steers queued
  for the next turn;
- provider-reported totals and request counts;
- the Norte session this one was compacted from, when it was.

The key is stable for the life of the conversation. A model switch keeps it. A
compaction keeps it while replacing the Norte session underneath and recording
the old one as `compacted_from`.

## Phases

The protocol exposes four phases:

| Phase | Meaning |
| --- | --- |
| `Idle` | Nothing is in flight. A new message may start a turn. |
| `Streaming` | A turn is running. |
| `AwaitingTool` | A tool call is authorised and about to run. |
| `AwaitingApproval` | A tool call is parked until a human decides. |

Questions are delivered as `QuestionAsked` while the turn is parked on the
question broker; they do not add a fifth public phase.

A process that dies while a session is `Streaming` leaves a stale row. On the
next load the service sees that no process can be streaming a session it just
read and repairs the Norte store to `Idle`. Parked approvals and questions are
not repaired away: they are durable and resumable.

## A turn

A turn is one round of "send input to the model, execute any tool calls, commit
what happened". The host loop is:

1. Build an engine for the session's current model.
2. Start or resume the Norte turn.
3. Drain engine events and map them to protocol events.
4. If a tool needs approval, persist the parked state and wait for the client.
5. If the model asks for another iteration, call the engine again with the same
   session and the tool results in history.
6. Settle usage, cost, telemetry, budget notices and the session phase.

Norte owns the commit point. A turn ends in at most one commit; the host's
effect journal covers the gaps Norte deliberately does not cover: a write or
command may have run even if the process died before the commit.

## Iterations

The engine has an iteration limit. The config's `max_iterations` and the CLI's
`--max-iterations` map to it; `null` means unlimited. Reaching the limit ends the
turn with an error event rather than silently dropping the remaining tool calls.

## Cancellation

`CancelTurn` asks the running turn to stop. The command tool kills its child
process group, so a hung command does not keep the terminal hostage. The turn
finishes uncommitted and the session remains resumable.

## Model switches

A model switch happens at a turn boundary. The service resolves the new
`provider/model`, checks that the existing session budget can still be measured
in the same unit, builds a new engine over the same store and only then drops
the old one. A failed switch leaves the session on the old model.

## Compaction

Compaction is a new Norte session behind the same logical key. It is described
in [Compaction](compaction.md); the point here is that the client's session key
and the host's approval rules survive even though the engine session id changes.
