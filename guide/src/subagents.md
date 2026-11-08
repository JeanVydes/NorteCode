# Delegation

`spawn_agent` creates a child session for a configured agent. The parent
profile's `delegates` list is an allow-list; `subagent` selects its default
child profile, and `agent_id` can choose another allowed profile. The child
uses that profile's model, system prompt and disabled-tool policy. Its
conversation starts with the task only, unless `context_mode: "summary"` asks
for a bounded, redacted excerpt of the parent history.

Children have their own session id, history, tools and lifecycle. They can
spawn further children up to the host's maximum delegation depth. Child runs
use the parent's budget ledger; a separate reserved `budget_share` is not
implemented.

## Detached runs

With `detach: true`, `spawn_agent` returns a `job-N` while the child runs in a
background task. `manage_jobs` can inspect or stop the job. When it finishes,
the child result is recorded in the job and delivered to the parent as a
message. Detached jobs are runtime tasks, not crash-recoverable work.

## Messaging and lifecycle

`message_agent` can steer the current turn, queue a follow-up turn, or
interrupt the current turn before starting the follow-up. An idle child starts
a new turn when messaged. `wait_agent` waits for idle or terminal state;
`stop_agent` allows a grace period or cancels immediately. `list_agents` and
`inspect_agent` expose the child state and available session details.

`lifetime` accepts `turn` or `session`, but automatic turn-bound cleanup,
idle expiry and parent-close descendant cleanup are not implemented yet.
Inspection of child events, approval routing, client-tool access, and
delegation idempotency are also not implemented.

## Handoff and takeover

`handoff_agent` is present but does not switch profiles: requests for another
agent are refused, and a request naming the current model is only accepted as
a no-op. RAEP's `HANDOFF` frame is not supported.

Client `takeover` on attach and delegation takeover are separate operations.
The host accepts `DELEGATOR_CONTROL` suspend/release and holds messages while
suspended. Controller takeover on attach does not yet displace the existing
controller connection; its current tracking is session-wide rather than
connection-specific.
