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
norte-code run --session <key> "continue with the tests"
norte-code resume <key> --budget 20000 --budget-unit tokens
norte-code sessions
norte-code cost
norte-code skills list
```

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
terminal; `NO_COLOR`, `TERM=dumb` and pipes keep it plain.

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
catalogue URL (default `https://models.dev/catalog.json`). Norte Code has no
telemetry.

## License

MIT. See [LICENSE](LICENSE).
