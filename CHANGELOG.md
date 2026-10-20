# Changelog

All notable changes to Norte Code are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- The built-in system prompt now states the Norte Code identity, requires tool
  use and verification, and tells the model not to guess its harness or claim
  to be another product.
- Tool calls render in blue, and a `command` call shows the command it ran even
  under `--quiet`; a long command is clipped to one line. Cancelling a turn
  before the provider reports usage now explains that the cancelled request is
  still recorded in telemetry.

### Added

- Runtime awareness stored per session: a new session gets a system block with
  the current model, provider and wire, catalogue dates and capabilities,
  modalities, compaction threshold and retained messages, sandbox state,
  approval mode, effective config and a short Norte Code help section. It
  contains no keys or endpoint URLs. A model switch appends a runtime update,
  and compaction seeds the fresh session with a new block.
- The mdBook guide under `guide/`: concepts, task guides and reference pages for
  the service, protocol, sessions, tools, approvals, compaction, budgets,
  storage, skills and CLI. A Pages workflow builds it on pull requests and
  publishes it from `dev`.
- Initial Cargo workspace: the protocol, agent service, config, catalog, cost,
  tools, SQLite store, CLI and TUI crates, with CI, formatting and lint
  configuration.
- The versioned client protocol: requests, server events, deltas, usage and
  budgets, with no Norte types on the wire.
- The in-process service: one Norte engine, logical sessions, streamed turns,
  cancellation, usage settling and reported cost.
- Configuration loading and merging (user file plus project overlay), with
  unknown-key warnings, validation and env/file secrets.
- The provider factory: models.dev `npm` selects the wire, `api` is the
  endpoint and `env` names the key, so any OpenAI-compatible provider takes the
  same path as OpenRouter. Native OpenAI, Anthropic and Google use Norte's
  presets when no URL is configured.
- The headless `norte-code run` command, with streamed text and `--jsonl`
  output, plus `--budget`, `--reasoning` and `--approval-mode`.
- `--interactive` on `run` and `resume`: the same session stays open and a line
  editor is pinned below the streamed output. Arrow keys, Home/End, Delete,
  Backspace, paste and the usual control keys work; a reverse-video cursor marks
  the insertion point and typed text never leaks into the answer. Lines
  completed while a turn is still running are queued and sent together as the
  next message, even after a failed or cancelled turn. `/exit`, `quit`, Ctrl-D
  or Ctrl-C at the prompt leave; a first prompt is optional on `run`. The prompt
  shares stdin with approvals and questions so typed input stays in order.
- The models.dev catalog client: tolerant schema, capability and reasoning
  mapping, ETag/TTL cache with stale and bundled fallbacks, and a live refresh
  request.
- Every provider the catalog describes, not just the native four: `npm` picks
  the wire, `api` supplies the endpoint (including `${VAR}` templates), and
  `env` names the API key; native OpenAI, Anthropic and Google still use Norte's
  presets when no URL is configured.
- `norte-code models`, with provider, tool and reasoning filters plus `--json`,
  and `model_list` over the protocol for the TUI picker.
- Mid-session model switching at a turn boundary, reusing the session and its
  store while rebuilding the engine for the new model.
- Reasoning resolution per model, with effort clamping to Norte's four efforts
  and config overrides that win over catalog data. `--reasoning` accepts
  `auto`, `off`, `minimal`, `low`, `medium` or `high` and is checked against
  the selected model before the turn starts; the implicit default asks for
  `medium` when the model declares it.
- Agent Skills: discovery from user, configured and project roots; YAML
  frontmatter validation against the Agent Skills specification; progressive
  disclosure through the `skill`, `skill_resource` and opt-in `skill_script`
  tools; `norte-code skills list|check`; and `ListSkills`/`RefreshSkills` over the
  protocol.
- A configured tool registry per engine: Norte's `read`, `write`, `edit` and
  `command` (with `PATH` and every resolved API key redacted), plus `glob`,
  `list_dir`, `grep`, `todo`, `ask_user` and opt-in `web_fetch`.
- A workspace-root policy that re-roots tool paths, canonicalizes the existing
  prefix so a symlink cannot escape the root, refuses dangling symlinks, and
  rejects FIFOs, sockets and devices before a tool can block on them.
- `ask_user` parks a running turn on a question and resumes it through the
  `QuestionAsked`/`AnswerQuestion` protocol, with a headless prompt on a terminal.
  The model may pass `options` for a closed selection and `multiple: true` for a
  multi-select; the terminal always appends an “answer something else” entry
  that falls back to the existing free-form answer channel.
- Global approval rules in the user config: `approvals.allow` and
  `approvals.deny` are tool-wide, denial wins over allow, and “always” choices
  written by the CLI preserve the rest of the config file.
- Approval modes (`ask`, `auto-edit`, `yolo`) as a Norte policy, with
  `Approve`/`Deny`/`Revoke` resuming a parked session and `allowed-tools`
  pre-approval behind `skills.honor_allowed_tools`.
- Failed tool calls carry the error text the model saw, shown by
  `norte-code run --verbose`.
- A SQLite `Store` + `BlobStore` that passes Norte's `store_conformance` suite,
  with one transaction per commit, optimistic concurrency and the host's own
  tables for logical sessions, pending questions and settings.
- Durable logical sessions: `run` reopens the database, restores the session
  list, and `--session <key>` continues one; `norte-code sessions` lists them.
- Restart-resume of parked approvals and pending questions. A resumed session
  re-emits the approvals still waiting and a question is replayed by its tool
  call id, so an answer given after a restart reaches the parked tool.
- Approval records and sessions are stored with their grants lifted into a
  side table shape, because `GrantScope`'s internally tagged newtype variants
  are not representable in JSON.
- An effect journal for `write`, `edit`, `command` and `skill_script`. The
  intent is written from the engine's tool-metadata callback immediately before
  the executor runs and the finish is written from the tool event; a call that
  was still at intent when the process started is refused on replay instead of
  being run a second time. Norte guarantees commit-at-most-once, not
  effect-at-most-once.
- Interactive approvals on stdin: a parked call prints the tool and a semantic
  preview of its arguments, then accepts `y` (allow once), `s` (allow this tool
  for the session), `a` (allow this tool always), `n` (deny once), `ns` (deny
  this tool for the session) and `ng` (deny this tool always). A message after
  any denial (`n <message>`, `ns <message>`, `ng <message>`) reaches the model
  as human feedback. Session denials are durable with the session and global
  rules live in the user config. The resumed turn is folded back into the same
  render loop, so a turn can park and resume several times. Piped stdin still
  leaves the session parked rather than guessing.
- Interactive chat commands: `/help`, `/compact`, `/refresh`, `/allow <tool>
  [session|global]` and `/deny <tool> [session|global]`. Rules default to the
  session and are stored with it; `global` writes the user config and replaces
  the opposite rule. An allow rule revokes a previous deny without another
  pending call. `/refresh` re-scans the skill roots so the next turn registers
  the new tools.
- A styled text renderer: model and tool names, outcomes, usage and errors use
  colour when the destination is a terminal, reasoning streams in its own dim
  gutter instead of mixing with the answer, and the run ends with a compact
  request/token/cost summary that includes the cache-read share and the
  input:output ratio. `NO_COLOR`, `TERM=dumb`, `CLICOLOR=0`, pipes and `--jsonl`
  all keep the output plain. With `--quiet`, reasoning is redacted to a single
  `thinking…` marker rather than disappearing entirely.
- Interrupted sessions are repaired on startup: Norte's `streaming` phase
  describes a live process, so a session left there by a hard kill is committed
  back to `idle` instead of being listed as streaming forever.
- A provider-reported cost ledger in SQLite, one row per model request, with
  per-message, per-session and per-UTC-day rollups. A request whose usage
  payload has no cost is counted as "not reported", never priced; catalogue
  prices are ignored. `norte-code cost` prints the same numbers as the run
  summary, with `--json` and `--session` filters.
- Budgets with `message`, `session` and `global_daily` scopes, `usd`, `tokens`
  and `requests` units, and `warn`, `ask` and `stop` actions. A Norte layer
  checks the ledger before each request, a post-request check reports warnings
  and exhaustion, and a stopped turn stays resumable. USD budgets are refused
  over unreported cost, and an incompatible model switch is blocked.
- `norte-code resume <key>` continues a parked or stopped session, optionally
  replacing its budget first (`--budget`, `--budget-unit`, `--budget-action`).
- `Ctrl-C` cancels the running turn through the protocol, so a hanging tool is
  stopped (including its process group) instead of leaving the terminal stuck;
  a second `Ctrl-C` quits. At an approval or question prompt it leaves the
  decision pending and exits with a resume hint instead of guessing.
- `--verbose` prints each tool's effective arguments and returned output as
  fenced blocks, with a visible truncation marker instead of an unbounded dump.
  The run summary also lists every tool that executed and how many times.
- `--debug` prints startup details: session id, model and wire, approval mode,
  iteration limit, workspace, config and database paths, and extension keys. It
  also prints the selected model's modalities, context and output limits,
  tool kinds, reasoning controls, streaming, cache, features, structured output
  and catalogue metadata, the effective reasoning setting for the session, and
  the character length of the system prompt and tool declarations; tokens are
  left to the provider instead of being estimated.
- A system prompt can be set in the config (`system_prompt`) or per run or
  resume (`--system-prompt`); the flag wins over the config, and the config wins
  over the built-in prompt.
- Model iterations are unlimited by default, with `--max-iterations N` to cap a
  run or resume. The compaction threshold can be overridden for one run or
  resume with `--compaction-threshold FRACTION`; it otherwise comes from
  `compaction.threshold` in the config.
- Vendor request extensions via `--extension KEY=JSON` or the config
  `extensions` map, keyed `<wire-family>:<field>`. OpenRouter upstream routing
  works through `openai.chat:provider.order`.
- Local request telemetry: a `requests` table in SQLite records every provider
  attempt with session and model, prompt/output/cache tokens, tools called,
  latency to first output and total, reasoning, reported cost, status and
  metadata. `norte-code telemetry` rolls the rows up by hour, day, month, year
  or total, with top providers, models and sessions. Nothing leaves the machine.
- Compaction at a safe boundary: when the provider-reported context crosses
  `compaction.threshold`, Norte Code summarises the old conversation and seeds a
  fresh Norte session with the summary, the todo list and the last
  `keep_recent` messages. `compaction.trigger` selects `idle` (the default,
  compact at a turn boundary) or `eager` (compact at the first safe
  model-request boundary inside a turn, after a complete tool result). Tool
  calls are never split from their results, eager never runs while an approval
  is pending and runs at most once per turn, the old session is kept for audit,
  the new session id is shown, and the summary request enters the cost ledger
  and budgets. `/compact` in the interactive prompt triggers the same path on
  demand; a failed summary leaves history unchanged.
- Project instructions: an `AGENTS.md` in the workspace root is appended to the
  system prompt of every new session. Only a regular file is read, the read is
  capped at 64 KiB, and the text is treated as repository guidance that does
  not override the user's request, the approvals or the budgets. `--debug` shows
  the file and its size.
- Bounded retries for transient provider failures (rate limits, overload,
  server and transport errors). The wait honours the provider's `retry_after`,
  capped by `retry.max_delay_ms`, and falls back to `retry.backoff_ms` with
  exponential growth when the provider gives no hint. A retry resumes the same
  session instead of resending the user's message, permanent failures (quota,
  authentication, malformed requests) are not retried, and
  `retry.max_attempts: 0` disables retries. Clients see a `RetryScheduled`
  event with the attempt, the delay and the reason.
- Optional command sandbox (`tools.sandbox`): `command` runs through bubblewrap
  on Linux or `sandbox-exec` on macOS with the workspace writable, the rest of
  the filesystem read-only, a private `/tmp` and no network unless enabled.
  `--sandbox`, `--no-sandbox` and `--sandbox-network on|off` override the config
  for one run or resume; the network option needs the sandbox and conflicts with
  `--no-sandbox`. Approvals and the effect journal keep the original command,
  the panel adds a `[bwrap]`/`[seatbelt]` badge, and a missing platform binary
  fails closed instead of running unsandboxed. Request telemetry rows record
  whether the sandbox was on and whether it allowed network, and `--debug` shows
  the effective sandbox state.
- A lost store race is reported as a conflict with a resume hint instead of a
  provider error. The session version is an optimistic-concurrency token, not a
  message count; a turn that loses the race did not commit.
- The per-request footer now includes answer tokens per second, total duration,
  first-token latency and the tools that request called, alongside the existing
  input/output, cache-read share, input:output ratio and reported cost.
  `--quiet` still hides all usage lines.
- The per-request token rate is answer tokens from the first delta to the end
  of the request. The wait for the first token is reported separately as
  latency, and a single final burst falls back to the whole duration instead of
  dividing by a sliver.

[Unreleased]: https://github.com/JeanVydes/NorteCode/commits/dev
