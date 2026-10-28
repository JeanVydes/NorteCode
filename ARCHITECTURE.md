# Architecture

Norte Code is a host around the Norte agent engine. Norte owns the durable
session, the turn state machine, streaming, tool dispatch, policies and the
commit point. Norte Code owns the agent loop that decides when to call the
engine again, the model catalogue, approvals, compaction, budgets, telemetry,
the sandbox and the user interfaces.

The dependency rule is one-way: Norte Code depends on Norte and never vendors,
forks or patches it. If a change belongs in Norte, it is reported upstream
instead of worked around here.

## Crate map

| Crate | Responsibility |
| --- | --- |
| `norte-code-protocol` | Versioned serde types for `ClientRequest` and `ServerEvent`. No Norte type appears on this wire. |
| `norte-code-agent` | `Service`: sessions, engines, agent loop, compaction, retries, approvals, sandbox, telemetry, skills. |
| `norte-code-config` | Typed config, merge, validation, approval-rule updates and secret resolution. |
| `norte-code-catalog` | models.dev client, ETag/TTL disk cache and mapping to Norte capabilities. |
| `norte-code-cost` | Cost ledger and budget evaluation. Provider-reported amounts only. |
| `norte-code-tools` | Host tools: `grep`, `todo`, `ask_user`, `web_fetch`. |
| `norte-code-store-sqlite` | Norte `Store` + `BlobStore`, logical sessions, approvals, questions, effects, costs and request telemetry. |
| `norte-code-cli` | The `norte-code` command and its interactive prompt. |


## Runtime path

```text
CLI
   │  ClientRequest { request_id, kind }
   ▼
norte-code-protocol
   ▼
Service ────────────────► Catalogue (models.dev + cache)
   │                         │
   │                         ▼
   │                    ModelSpec / capabilities
   ▼
Norte Engine (one model, one Norte session)
   ├─ policies: workspace root, approval mode, budgets
   ├─ tools: read/write/edit/command + host tools + skills
   ├─ layers: budget, compaction, telemetry
   └─ observers: telemetry counters
   │
   ▼
SQLite store
   ├─ Norte sessions and nodes (Norte's own schema)
   ├─ logical_sessions (client key → Norte session, stats, rules)
   ├─ approvals / questions / effects
   ├─ costs (one row per billed request)
   └─ requests (one row per provider attempt)
```

`Service::submit` returns an `EventStream`, a stream of `ServerEvent`s. Every
event carries the `request_id` that caused it and, when it concerns a session,
the stable logical `SessionKey`. A turn may emit text, reasoning, tool and
usage events before its terminal `TurnFinished` or `Error`.

## Session and turn lifecycle

A logical session is what the client sees. It owns a current Norte `SessionId`,
the model, a phase, host counters and the session approval rules. Norte owns the
actual history and the commit point. A new message creates a Norte session if
the logical session has none; a resume continues the current one.

The host loop is:

1. Start or resume a turn on the engine.
2. Drain engine events. Text and reasoning deltas are forwarded; tool calls are
   announced and their results are read from `engine.history()` after the tool
   finishes.
3. If the turn parks for an approval, persist the parked state and let the
   client approve or deny. The decision resumes the same turn.
4. If the turn ends with tool calls and the model needs another iteration, call
   the engine again with the same session. Iterations are unlimited unless
   `max_iterations` is set.
5. Settle usage, reported cost, telemetry and the session phase.

Norte guarantees commit-at-most-once, not effect-at-most-once. Effectful calls
are therefore journaled before execution and marked done afterwards. A process
that dies between those points leaves an intent that is refused on the next
start instead of being replayed.

## Protocol and clients

The protocol is plain serde JSON with `deny_unknown_fields` on events and a
versioned shape. Clients never receive Norte IDs or Norte event types: the
service maps them once. New optional fields use `#[serde(default,
skip_serializing_if = "Option::is_none")]` so older clients keep working.

The CLI is a normal client of the same protocol. Approvals and questions are
durable protocol objects, not terminal-only state. A non-interactive client
leaves the session parked instead of guessing a decision.

## System prompt

A new session stores one leading system message: the base prompt plus the
optional `AGENTS.md` project instructions. Nothing else is injected into the
history. Model facts, limits, sandbox state and config paths are available to
the human through `--debug` and the protocol, not as prompt text.

Compaction seeds the fresh session with the same base prompt, and a model switch
leaves the existing history unchanged. `PromptSize` counts the base prompt and
the tool declarations.

## Model switching

A model switch happens at a turn boundary. The service keeps the logical
session and its Norte session, resolves the new `provider/model` through the
catalogue and config overrides, checks that the new model supports the existing
budget unit, then builds a new engine over the same stores. The old engine is
dropped only after the new one is built, so a failed switch does not lose the
session. A switch that would make a session budget unmeasurable is refused
before it happens.

Reasoning is resolved per model: explicit `--reasoning`, then the config
default, then the catalogue's efforts. An unknown effort is rejected before the
turn starts; the implicit default picks `medium` when the model declares it and
falls back to the closest lower effort otherwise.

## Compaction

Compaction keeps one logical session but creates a new Norte session:

1. Read the old history and the current todo list.
2. Ask a model for a structured summary. The summary request is a normal
   provider request with the configured budget and telemetry; it has no tools.
3. Create a fresh Norte session and seed it with the summary, the todo list and
   the last `keep_recent` messages.
4. Point the logical session at the fresh Norte session. The old Norte session is
   kept for audit and is linked through `compacted_from`.

Safety invariants:

- A suffix never starts with a tool result whose tool call was dropped.
- Eager compaction runs only at a safe model-request boundary, after a complete
  tool result, never while an approval is pending, and at most once per turn.
- A failed summary leaves the old history, the active session id and the phase
  unchanged.
- The new session id is shown to the client. The summary's reported cost enters
  the ledger and counts toward budgets.
- A model without a catalogue context window does not compact automatically.

`/compact` uses the same path on demand. `compaction.trigger` selects `idle`
(default, at a turn boundary) or `eager` (first safe boundary inside a turn).

## Approvals and rules

The approval policy is consulted before any effectful tool runs. Tool names can
be allowed or denied at two scopes:

- Global rules live in the user config (`approvals.allow` / `approvals.deny`).
- Session rules live in the logical session and are persisted with it.

Denial wins over allow, and a global deny wins over a session allow. The
interactive prompt also has one-shot and tool-wide choices. Session choices
become Norte grants; session rules are the durable host-level form. The
interactive commands `/allow <tool> [session|global]` and `/deny <tool>
[session|global]` edit the same rules; an allow rule replaces an existing deny
for that tool, which is how a deny is revoked. `/refresh` re-scans skill roots
and rebuilds the tool registry for the next turn.

An approval panel always shows the tool and the arguments the model requested.
The OS sandbox is applied later, at execution time, so the human approves the
original command, not a rewritten launcher.

## Command sandbox

The sandbox is an executor wrapper around Norte's `CommandTool`, not a policy.
The tool keeps its name, schema and arguments; only the process invocation is
rewritten. This keeps approvals, history, the effect journal and telemetry
honest.

Linux uses bubblewrap:

```text
bwrap --die-with-parent --new-session --unshare-all --cap-drop ALL \
  --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp \
  --bind <workspace> <workspace> [--bind <extra> <extra>] \
  [--share-net] --chdir <cwd> -- <original command>
```

macOS uses `sandbox-exec -p <profile> -- <original command>`. The profile keeps
reads, denies writes outside the workspace, the configured extras and the
temporary roots, and denies the network unless enabled.

The sandbox contains writes, PIDs, IPC/UTS/cgroup namespaces and (by default)
the network. It is not a virtual machine: host files remain readable, Unix
sockets visible in the filesystem can still be reached, CPU/memory/disk are
shared, and the kernel is shared. A missing platform binary fails closed; there
is no unsandboxed fallback. `skill_script` is a separate opt-in
(`skills.allow_scripts`) and is not covered by this wrapper.

## Retries

Retries are host policy, not provider retries. Only `RateLimited`,
`ModelOverloaded`, `ServerError` and `Transport` errors are retryable. Quota,
authentication and malformed requests fail immediately. The wait is the
provider's `retry_after` capped by `retry.max_delay_ms`, otherwise
`retry.backoff_ms` doubling up to the cap. A retry resumes the same session with
no new input, so the user message is not appended twice. The client sees
`RetryScheduled { turn, attempt, delay_ms, reason }`; the engine's duplicate
error event is suppressed and the final failure is emitted once.

## Telemetry and metrics

Every provider attempt writes one `requests` row: session and Norte session,
provider/model, start time, duration, first and last output delta, provider
token counters, cache flags, tool calls, reasoning, reported cost, status, error,
whether the sandbox was enabled and whether it allowed network. Tokens are
provider-reported; the host does not estimate them, except for a bytes/4
context fallback when no usage exists.

The per-request footer's token rate is:

```text
output_tokens / (request_end - first_delta)
```

The first-token wait is reported separately as `latency`. A single final chunk
has no meaningful generation window, so the rate falls back to the whole request
duration. This avoids the hundreds-of-tokens-per-second figure produced by
dividing by the tiny gap between a buffered first and last chunk.

Engine-event counters (requests, stream deltas, tool calls, commits, errors) are
recorded beside the provider numbers. Telemetry is local SQLite only; Norte Code
sends no telemetry anywhere.

## Cost and budgets

Cost is only what the provider reports. The host never computes a price from a
catalogue list price, and `cost.*` catalogue fields are ignored. The ledger has
one row per billed request with tokens, reported cost or `NULL`, and a model
switch that would make a session budget unmeasurable is refused. `norte-code
cost` prints the ledger; `norte-code telemetry` prints request latency and
token/cache totals.

Budget scopes are message, session and global daily, measured in USD, tokens or
requests. `warn` continues, `stop` ends the turn uncommitted and leaves it
resumable, and `ask` is currently a resumable stop with a notice rather than an
immediate prompt. The host never treats unreported USD cost as zero; such a
budget is skipped instead of pretending it was met.

### Per-provider cost probe results

These are the results observed in the development environment, not a promise
about every account:

| Provider | Result |
| --- | --- |
| OpenRouter | Live requests report `usage` with prompt/output/cache counters and, for paid models, a USD cost. The free `minimax/minimax-m3.1-flash:free` route reports zero cost; some failed attempts have no cost. |
| OpenAI | Not probed here: `OPENAI_API_KEY` was unset. |
| Anthropic | Not probed here: `ANTHROPIC_API_KEY` was unset. |
| Google/Gemini | Not probed here: `GEMINI_API_KEY`/`GOOGLE_API_KEY` were unset. |

The host treats a missing cost as `NULL`, not as zero, so an unpriced request is
visible in the ledger and in the budget's "unpriced" count. A provider that
reports cost in a vendor field is read through Norte's usage payload; no local
price table is involved.

## Network and privacy

The only network destinations are the configured model endpoints and the
models.dev catalogue. `--offline` and `catalog.offline` prevent the catalogue
fetch; the cache and bundled snapshot are used instead. API keys live in Norte's
redacted secret type and are registered with the command tool for output
redaction. `--debug` never prints keys or endpoint URLs. There is no remote
telemetry, crash reporting or update check.

## Verification

The workspace is checked with:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Hermetic tests use `norte-testing` (`ScriptedModel`, `ProbeModel`,
`RecordingObserver`) and an in-memory or temporary SQLite store. Live-provider
tests are `#[ignore]`d and key-gated. The current milestone report, including
what is not verified, is in [`REPORT.md`](REPORT.md).
