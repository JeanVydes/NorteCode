# Norte Code

A terminal-based agentic coding tool, in Rust.

Norte Code is a host that sits on top of [Norte](https://github.com/JeanVydes/Norte),
the provider-agnostic agent engine. Norte owns the session, the turn, streaming,
tool dispatch, policies and durability; Norte Code owns the agent loop, the
model catalogue, approvals, compaction, cost tracking and the user interface.
Norte Code never forks Norte.

> Status: under construction. The workspace skeleton is in place; the service,
> CLI and TUI are built milestone by milestone. Nothing here is released yet.

## Layout

| Crate | What it is |
| --- | --- |
| `norte-code-protocol` | The versioned serde types on the client wire. No Norte types. |
| `norte-code-agent` | The service library: engines, agent loop, policies, sessions, compaction. |
| `norte-code-config` | `config.json` loading, validation, merging and secrets. |
| `norte-code-catalog` | The models.dev client, its disk cache and the mapping to Norte capabilities. |
| `norte-code-cost` | The cost ledger and budget evaluation. Provider-reported cost only. |
| `norte-code-tools` | The tools Norte does not ship: `glob`, `list_dir`, `grep`, `todo`, `ask_user`, `web_fetch`. |
| `norte-code-store-sqlite` | A Norte `Store` + `BlobStore` over SQLite, plus Norte Code's own tables. |
| `norte-code-cli` | The headless `norte-code` command. |
| `norte-code-tui` | The interactive terminal UI. |

## Building

```sh
cargo build
cargo test
```

The engine is a git dependency (`norte`, branch `dev`). `Cargo.lock` pins the
exact revision and is committed.

## Running

```sh
norte-code run "explain this repository"
norte-code run --interactive "start here"
norte-code run --session <key> "continue with the tests"
norte-code resume <key> --budget 20000 --budget-unit tokens
norte-code sessions
norte-code cost
norte-code telemetry --bucket day
norte-code skills list

# Startup details, tool I/O and a vendor extension
norte-code run --debug --verbose \
  --extension 'openai.chat:provider.order=["deepinfra"]' \
  "review the diff"
```

`--debug` prints the session id, model, wire, approval mode, iteration limit,
compaction threshold and trigger, workspace, config and database paths, and the
configured extension keys. It also prints the selected model's modalities, context and
output limits, tool kinds, reasoning controls, streaming, cache, features,
structured output and catalogue metadata, the effective reasoning setting for
the session, and where the system prompt came from (flag, config or built-in),
plus how many characters the system prompt and tool declarations contain.
Tokens are not estimated: only the provider counts them, and the per-request
usage line reports the provider's number. Unless `--quiet` is set, each request
also shows answer tokens per second, total time, time to the first token,
the cache-read share, the input:output ratio and the tools called by that
request. `--verbose` prints each tool call's
effective arguments and its result as fenced blocks; large values are shortened,
and long tool output is capped before it crosses the wire. The final summary
lists the tools that ran and their counts. Iterations are unlimited by default;
`--max-iterations N` caps one run or resume. The compaction threshold lives in
`compaction.threshold` (default `0.90`) and can be overridden for one run or
resume with `--compaction-threshold FRACTION`. The system prompt comes from
`--system-prompt` when given, otherwise from `system_prompt` in the config, and
otherwise from the built-in prompt. An `AGENTS.md` in the workspace root is
appended to whichever prompt is used, as project instructions. It is untrusted
input: only a regular file is read, the read is capped at 64 KiB, and the
instructions never override the user's request, the approvals or the budgets.

`--interactive` keeps the same session open after each turn and prints a `›`
prompt for the next message; `/compact` summarises the conversation on demand
and `/exit`, `quit`, Ctrl-D or Ctrl-C at that prompt leave. `run --interactive` may omit its first prompt and start at the prompt;
`resume --interactive` reopens a parked session and then continues the
conversation. It needs a terminal and cannot be combined with `--jsonl`. The
interactive prompt shares stdin with approvals and questions, so input typed
ahead is consumed in order by the next prompt that needs it.

Vendor-specific request fields use Norte's extension mechanism: a key is
`<wire-family>:<field>`, and the value is JSON. The config has the same shape
under `extensions`. For OpenRouter, the wire is `openai.chat`, so upstream
routing is for example:

```sh
--extension 'openai.chat:provider.order=["deepinfra"]'
# or pin one endpoint variant
--extension 'openai.chat:provider.order=["deepinfra/turbo"]'
```

A key whose wire family does not match the selected model is ignored.

Compaction runs automatically when the last provider-reported prompt plus answer
crosses `compaction.threshold` (default `0.90` of the model's context window).
`compaction.trigger` selects when: `idle` (the default) waits for an idle turn
boundary and also checks before the next turn when a session is resumed or
switched to a smaller model; `eager` checks at the first safe model-request
boundary inside a turn, after a complete tool result and before the next
request. Eager still never splits a tool call from its results, never runs
while an approval is pending, and runs at most once per turn. It also runs on
`/compact`. The summary is built from the old transcript and the new session is
seeded with the summary, the current todo list and the last `keep_recent`
messages (default `6`). A suffix never starts with a tool result whose call was
dropped. The old Norte session is left untouched for audit; the logical session
keeps its key but shows the new session id. The summary request is a normal
provider request: its reported cost enters the ledger and counts toward budgets.
If the summary fails, history and the active session id stay unchanged, and an
eager turn stops at the safe point so it can be resumed. Models without a
catalogue context limit do not compact automatically.

A transient provider failure (a rate limit, an overload, a server error or a
transport error) is retried up to `retry.max_attempts` times (default `2`). The
wait is the provider's `retry_after` when it sent one, capped by
`retry.max_delay_ms` (default 30 s); without a hint the first wait is
`retry.backoff_ms` (default 1 s) and doubles on each attempt. A retry resumes
the same session with no new input, so the user's message is never appended
twice. Permanent failures (quota, authentication, a malformed request) fail
immediately, and `retry.max_attempts: 0` disables retries.

Sessions live in a SQLite database under the platform data directory
(`$XDG_DATA_HOME/norte-code/sessions.db`, or
`~/.local/share/norte-code/sessions.db`). A run creates a new session unless
`--session` names an existing key. Parked approvals and pending questions are
in that database, so a session can be resumed after a restart. Effectful tool
calls are journaled: if a process dies after a write or command may have run,
the next process refuses to run that same call again rather than repeat it.

When a tool needs approval, `norte-code run` prints the call and its arguments
and offers six choices on stdin: allow once (`y`), allow this tool for the
session (`s`), allow this tool always (`a`), deny once (`n`), deny this tool for
the session (`ns`) or deny this tool always (`ng`). `n <message>` (and the
session/global forms) sends human feedback to the model instead of a bare
refusal. Session allows become Norte grants; session denials are stored with the
session; “always” rules are written to `approvals.allow` / `approvals.deny` in
the user config, so they survive a restart. A global deny wins over a global
allow. Approving resumes the same turn; input is read from the terminal, never
from the model. With no interactive terminal the session stays parked, which is
what `--jsonl` scripts and other clients expect. Reasoning streams in dim italic
and tool, usage and turn summaries are colourised only when the output is a
terminal; `NO_COLOR`, `TERM=dumb` and pipes keep it plain. With `--quiet`,
reasoning is redacted to one `thinking…` marker per turn instead of disappearing
entirely, and tool activity and usage stay hidden.

`ask_user` accepts an optional `options` list and `multiple` flag. When the model
offers choices, the terminal shows them as a numbered list plus an automatic
“Something else (type it)” entry; choosing that entry asks for free-form text
through the same answer channel. A multi-select answer joins the selected option
strings with `, `.

While a turn is running, `Ctrl-C` cancels it: the running tool is asked to stop
(Norte's `command` tool kills the whole child process group, so a hung command
does not keep the terminal hostage) and the turn finishes uncommitted. A second
`Ctrl-C` within five seconds only repeats “still cancelling”; after five seconds
it force-exits. The session stays resumable with `norte-code resume`.

`Ctrl-C` at an approval panel or a question prompt does not submit a decision or
an answer. The approval or question stays pending, the command exits cleanly and
prints how to resume, so a mistaken keypress never turns into a deny or an
invented answer.

## Command sandbox

`tools.sandbox.enabled` (default `false`) runs every `command` inside the host
sandbox: bubblewrap on Linux, `sandbox-exec` on macOS. The workspace is
writable, the rest of the filesystem is read-only, `/tmp` is a private tmpfs,
and the network is off unless `tools.sandbox.network` is `true`. Extra writable
directories go in `tools.sandbox.writable`. The approval panel still shows the
command the model asked for, with a `[bwrap]` or `[seatbelt]` badge; the sandbox
flags are internal and never replace the approved arguments. The wrapper sits
inside the command tool's executor, after policies, approvals and the effect
journal.

On `run` and `resume`, `--sandbox` and `--no-sandbox` override
`tools.sandbox.enabled` for that invocation, and `--sandbox-network on|off`
overrides `tools.sandbox.network`. `--sandbox-network` needs the sandbox to be
on, so pass `--sandbox` or enable it in the config; it cannot be combined with
`--no-sandbox`, and `--sandbox` and `--no-sandbox` are mutually exclusive.

```bash
norte-code run --sandbox --sandbox-network on \
  "fetch the changelog and summarise it"
```

The sandbox needs the platform binary (`apt install bubblewrap`, `dnf install
bubblewrap`, `pacman -S bubblewrap`, or the `sandbox-exec` that ships with
macOS). If it is missing, or the platform cannot create the namespaces, the
command fails instead of running unsandboxed. It contains writes, processes and
(by default) the network, but it is not a VM: it can still read files the user
can read, reach Unix sockets visible in the filesystem, consume CPU, memory or
disk, and share the kernel. Treat it as one layer with the approval rules, not
as a replacement for them. `skill_script` has its own switch
(`skills.allow_scripts`) and is not covered by the command sandbox.

## Local request telemetry

Every provider attempt is recorded locally in the `requests` table: session,
provider and model, prompt/output/reasoning/cache tokens, tools called, latency
to first output and to completion, reasoning setting, reported cost, status and
response metadata. `norte-code telemetry` prints totals and averages; `--bucket
hour|day|month|year|total` changes the grouping, `--session`, `--provider` and
`--model` filter, and `--json` emits one object. It also lists the top
providers, models and sessions by requests and tokens. This is a local audit
log only: Norte Code sends no telemetry anywhere.

## Cost and budgets

Norte Code never computes a price. The only dollar figure is the one the
provider reports in each response's usage payload, and a request without one is
counted as "not reported" rather than priced from a list. The catalogue's
`cost.*` fields are ignored. Token counters always come from the provider.

The SQLite ledger stores one row per model request (session, turn, request
index, model, token counters, reported cost or `NULL`). `norte-code cost` prints
the total, per-session and per-UTC-day rollups; `--session <key>` adds the
session's turns and `--json` emits the same data as one object. The run summary
and JSONL events use the same ledger numbers. `cost.show: false` hides them;
budget warnings still appear.

Budgets are opt-in, in config and per message (`run --budget`, in USD) or per
session (`resume --budget` with `--budget-unit usd|tokens|requests`). Scopes are
`message`, `session` and `global_daily`; actions are `warn` (keep going), `ask`
and `stop`. A Norte layer checks the measured spend before every request and the
service checks again after each one, so the last request may overshoot by one.
A stopped turn is not committed and stays resumable: raise the budget with
`resume` and continue. A USD budget is refused when the provider has already
returned requests without a reported cost, and a model switch that would make an
existing USD budget unmeasurable is blocked.

## Network

The only network destinations are the configured model endpoints and the model
catalogue URL (default `https://models.dev/catalog.json`). Norte Code sends no
telemetry; the request history and cost ledger stay in the local SQLite
database.

## License

MIT. See [LICENSE](LICENSE).
