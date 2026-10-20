# Changelog

All notable changes to Norte Code are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
- `--interactive` on `run` and `resume`: the same session stays open after each
  turn and prints a `›` prompt for the next message. `/exit`, `quit`, Ctrl-D or
  Ctrl-C at the prompt leave; a first prompt is optional on `run`. The prompt
  shares stdin with approvals and questions so queued input stays in order.
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

[Unreleased]: https://github.com/JeanVydes/NorteCode/commits/dev
