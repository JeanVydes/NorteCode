# Changelog

All notable changes to Norte Code are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `shell_bg` runs a shell script in the background and answers at once with a
  job id; `manage_jobs` lists a session's jobs, reads what one has written, or
  stops one. A job's output is kept in a bounded buffer while it runs and its
  result arrives as a message when the process ends, so a long build no longer
  holds the turn. Only `shell_bg` registers a job: a waited `shell` is its own
  result, and the job ends when its streams close rather than when its call
  finishes.
- `/jobs [list|check|stop] [job-id]` shows the session's background jobs, reads
  one, or ends one. A job belongs to the session that started it, which is also
  what `Ctrl-C` reaches.
- `/restart` starts a fresh session and leaves the previous one saved and
  resumable.
- `Ctrl-C` stops the session's background jobs while a turn runs, `Escape` asks
  the running turn to stop, and `Ctrl-Shift-B` detaches the running call: the
  turn stops waiting, the process keeps running, and the call answers with what
  arrived so far. `Ctrl-Shift-B` rather than `Ctrl-B`, which readline uses to
  move the cursor back.
- The terminal is put in raw mode with `ISIG` cleared, so `Ctrl-C` reaches the
  editor as a key instead of being turned into `SIGINT`. Before, the signal
  handler and the key reader raced for the same keystroke and the handler did
  not always win, so `Ctrl-C` looked like it sometimes did nothing.
- `--auth disabled`, and `providers.<id>.auth: "disabled"` in the config, sends
  no credential and needs no API key. A local OpenAI-compatible server can be
  used without exporting a dummy key.
- `command`, `write` and `read` show a short result when they finish, so the
  transcript carries what the tool did without `--verbose`: a command's output,
  the path a write landed in with a glimpse of it, and what a read returned.
  The block is bounded and escaped, and a failure now shows its reason at any
  verbosity.
- `/reload` re-reads the configuration without restarting, so a session keeps
  its transcript. Everything the config derives is rebuilt — the default model,
  the keys registered for redaction, the skills and the global approval rules —
  and an unknown key is reported rather than ignored. The open session keeps the
  model it resolved and the rules granted during the run, which are session
  state; a new session uses the new default. A file that does not parse, or
  names a model that cannot be built, changes nothing.
- An MCP server's tools are filtered by `allow` and `deny`, written as the
  server names them. Every tool rides in each request, so the sixteen tools of
  the reference server cost about 25k characters of schema on every turn; the
  lists are what turn "the whole server" into "the tools this host asked for".
  A name the server does not publish is reported at startup instead of being
  ignored.
- `/mcp` lists the connected servers with their tools and whether they are on;
  `/mcp enable|disable <server>` switches one. The process stays up, so turning
  one back on is instant; what changes is whether its tools reach the model.
- MCP servers, named in an `mcpServers` block and each started once at
  startup. Every tool a server publishes is registered as
  `mcp__<server>__<tool>`, so the server keeps its own namespace and a per-tool
  approval rule still names one tool. The block keeps the name and the shape the
  rest of the ecosystem uses, so a configuration written for another client works
  here unchanged. The child gets only its configured `env` and the `PATH`, and
  its values are registered for redaction. A server that cannot start is
  reported and skipped rather than stopping the run, and `--debug` prints the
  servers that connected with the ones that did not.
- `--verbose <LEVEL>` replaces `--quiet` and `--verbose`, which were the two
  ends of one axis and contradicted each other. `low` shows the answer and each
  tool's result; `medium` (the default) also says that a tool is running and
  that the model is thinking; `high` adds the reasoning text, the arguments each
  tool was given and its full output. An `edit` shows its diff at every level.
- `command`, `write` and `read` show a short result when they finish, so the
  transcript carries what the tool did without asking for more verbosity. The
  block is bounded and escaped, a long stdout no longer hides the stderr that
  explains a failure, and a failed tool shows its reason at any level.
- The footer names the session from `medium` up, so returning to one no longer
  means scrolling back to the startup block to copy its key.
- `/reload` re-reads the configuration without restarting, so a session keeps
  its transcript. Everything the config derives is rebuilt and an unknown key is
  reported rather than ignored; the open session keeps the model it resolved and
  the rules granted during the run, which are session state.
- The cache share shows two decimals wherever it is printed: `99.99%`, and
  `100.00%` rather than `100%`. A cache that stops hitting moves by hundredths.
- `--api-key-env VAR` reads the API key from `VAR` instead of the configured
  `api_key_env` or the catalogue's `env` names, without touching the config
  file.
- `--disable-prompt-cache-key` (and `providers.<id>.prompt_cache_key: false`)
  omits the session-derived `prompt_cache_key` field that OpenAI wires send.
  A gateway that rejects unknown fields can now be used.
- Resuming a session shows where the conversation was. The agent replays the
  most recent messages, oldest first, capped at a hundred, so a client that
  starts blank can print the transcript before the turn continues.
- The footer has a context dial: a ring whose filled share is how much of the
  model's window the last request used, blue below the compaction threshold,
  amber near it and red with `compact` once it is reached.

### Removed

- The runtime-awareness system block is no longer injected at session start, on
  a model switch or after compaction. The model sees the base prompt and
  `AGENTS.md` only. The block was a second system message, which local chat
  templates often reject, and it was not worth its prompt cost.

### Changed

- The workspace gate and the command sandbox live together under
  `tools.confinement`. They answer the same question at two levels: the gate
  re-roots a tool's path under the workspace and refuses an escape, while the
  sandbox wraps `command`'s child process. The sandbox stays off by default and
  paths stay confined by default.
- `tools.confinement.workspace_only: false` lets `read`, `write`, `edit`,
  `glob`, `list_dir`, `grep` and `command`'s `cwd` name any path, for a host
  working across repositories. A special file such as a FIFO is still refused,
  because opening one with no writer would block a tool forever.
- The `--debug` blocks are framed panels whose columns are sized from their
rows, so a long path no longer breaks the frame and the fields of a block line
up whatever the block holds. The section title is bold cyan, a field label dim
and its value in the default colour, and the startup panel carries the session
key, the effective settings and the capabilities in force together. A pipe or a
log file gets the plain rows instead of the box drawing.

### Fixed

- OpenAI Responses now honors the host's `no_prompt_cache_key` quirk and the
  `--disable-prompt-cache-key` flag. It sent `prompt_cache_key`
  unconditionally, so a gateway that rejects the field could not be used.
- An Anthropic overload or API failure that arrives as a stream event is now
  retried like the same HTTP status instead of ending the turn as an unknown
  error, and a response stopped by the context window shows as truncated.
- Anthropic thinking follows the model instead of using one shape for every
  generation. Claude 4.6 and later take adaptive thinking with
  `output_config.effort`; 4.5 keeps the token budget. An explicit request asks
  for a visible summary, thinking tokens are counted in usage, and cache
  breakpoints never land on a thinking block, which cannot carry one.
- `--reasoning off` is offered on Anthropic 4.5 through 4.8 and Opus 5,
  which accept `thinking: {type: "disabled"}` even though models.dev has no
  toggle for them. Sonnet 5.5 and Opus 5.5 keep refusing it, as the API does.
- Every request now sends the model's catalogue output limit as its max
  output. The adapters fell back to 4096, which capped long answers and made
  an Anthropic reasoning budget of 4096 fail because `max_tokens` must be
  larger than the budget.
- A shell `command` no longer has to name its interpreter (`shell` defaults
  to `sh`), and `write` no longer has to send `overwrite` (it defaults to
  false). The schema required both, so a natural call failed and the model
  spent turns guessing.
- A native Anthropic request that asks for a reasoning token budget now sends
  it as `thinking` instead of dropping it. Models whose catalogue entry only
  offers `budget_tokens`, such as Claude Haiku 4.5, now think when asked.
- Approving a parked tool with `y` no longer closes the interactive session.
  The approval submitted the resumed stream and the turn loop returned before
  driving it, so the session looked parked even though the tool had run.
- A provider that reports cache reads separately, like Anthropic and the
  OpenRouter Messages endpoint, no longer shows a cache share above 100%.
  Input tokens now include the cached and cache-creation tokens, so the
  percentage is a share of the whole prompt.
- A rate-limited free tier is no longer given up on in three seconds. The
  default retry schedule waits 5, 10, 20 and 40 seconds (four retries) under a
  60-second cap, so a per-minute window can reopen before the turn fails.
- A rate limit that says how long to wait is obeyed. Gemini sends the delay
  in the error body instead of a `Retry-After` header, so the CLI used to burn
  its retries and fail while the quota window was still closed.
- Anthropic and Google never report cost, so their live lines no longer say
  it is missing. A USD budget on either warns at startup that it cannot be
  measured and points at tokens or requests instead.
- A queued model switch that could not be honoured no longer breaks the
  session. The model may have left the catalogue, or the session's own
  settings may refuse the switch; either way the turn goes on with the current
  model instead of failing, and every turn after it failed too.
- A model switch and a compaction at the same turn boundary are both reported
  now. The compaction replaced the events of the boundary and the switch was
  lost.
- Text typed while the budget stopped the turn is not thrown away: if it is
  not a new limit, it is steered into the session and the prompt asks again.
- A turn the host ended on purpose is reported as such instead of reading as
  a failure that committed nothing.
- The budget no longer lets a request through that the limit should have
  stopped. The gate measures the request it just let through, in the engine's
  own path; before, the cost was recorded by the service, which runs while the
  engine already asks for the next request, so the budget sometimes answered
  late.
- The numbers a request reports — its tools, its timing — no longer depend on
  whether the service settled the request before or after the answer was
  committed. The telemetry that arrives first waits for the message it belongs
  to.
- The interactive prompt reads again after a turn. When no steered message was
  left to apply, the turn loop restarted with nothing to wait for and never
  returned, so the next line was read by the terminal and never reached the
  service.
- A message typed while a turn runs is echoed into the transcript when it is
  steered. Before, the line was lost from the transcript, so the model's next
  answer read as if it came from nowhere.
- A retry says when the wait is over. The service reports the attempt as it
  starts and the prompt prints `retrying now (attempt N)`, so the last
  `retrying in ...` line no longer stands while the model is already thinking.
- The working indicator no longer paints over the reasoning. It stopped only on
  the first answer token, never on the first reasoning one, so the animated line
  overwrote the dim `┌ thinking` gutter and left the thought cut up. It is now a
  bar that owns its line, is painted through `Style` and is stopped before any
  other writer emits.
- The loading bar no longer carves through the `debug: session` line. It stops
  before the diagnostic is written, like every other writer on stderr.
- `--debug` no longer claims `(200k context, 32k output)` for a model the
  catalogue does not list. It says the capabilities come from the config or the
  fallback and prints the effective limits below, so a `model_overrides` context
  is not contradicted on the line above it.
- A keepalive or the stream's trailing empty part no longer splits a thought
  into several `┌ thinking` blocks. An empty text or reasoning delta changed the
  active block, so the gutter closed and the answer interleaved with its
  reasoning.
- The renderer's own escape sequences are no longer escaped themselves, which
  printed `\u{1b}[1mdebug: model` instead of a styled heading.
- A resumed session announces its turn before replaying the history. The turn's
  task emitted `TurnStarted` only after the replayed messages had been read, so
  the transcript appeared while the client still showed the session as idle.
- A selection of headers are rendered, tables are drawn with borders and
  aligned columns, and emphasis inside a table cell keeps its style. The
  renderer had lost all three: a table's header was dropped, its cells were
  joined at whatever width each happened to be, and a style written inside a
  cell landed outside the row. Inline and block HTML is now shown as the text it
  is and never interpreted, a link shows its destination, an image its source,
  and a `mermaid` block is labelled as a diagram rather than shown as runnable
  code.
- The footer no longer walks down the screen on every resize. The frame that
  moves the scroll region ended with a newline on its bottom row, and each
  newline scrolled the region and left a blank line above the footer; a drag
  fires the event many times, so the gap grew. The frame also released the
  region before setting the new one, which let streamed text land on a footer
  row and be erased by the repaint.
- Markdown answers are escaped before they are rendered. A control character or
  a bidi override inside a model's markdown could reach the terminal raw, and
  one that spelled an escape sequence was turned into a real one by the
  renderer.
- A resumed session shows the conversation, not just the live one. A compaction
  replaces the session with a summary and the last few messages, and the replay
  read only that, so resuming after a compaction printed a handful of lines
  where the user expected the transcript. The chain of sessions a compaction
  came from is walked to fill the window, still without crossing a boundary.
- Resuming reports the usage the session already spent. The counters live in
  the session record and survived, but only the replayed messages crossed the
  wire, so the footer showed the totals of the messages it happened to see: a
  session that had spent a hundred thousand input tokens showed nothing until
  the next request arrived.
- `Shift+Enter` breaks the line instead of submitting. The editor branched on
  the shift modifier, which the terminal never reported: without the keyboard
  protocol it sends the same carriage return for both, so the newline branch
  was unreachable.
- A multi-line message is drawn on its own rows. The input line always drew
  one row and ignored the break, so a second line was invisible and the cursor
  column counted it, which made the draft look like it was erasing itself.

### Added

- A provider can override its wire and credential scheme in the configuration.
  `providers.<id>.wire = "messages"` sends that provider's models through the
  Anthropic adapter and `auth = "bearer"` authenticates a gateway that expects
  `Authorization`; together they let OpenRouter's `/api/v1/messages` endpoint
  exercise the Messages wire without an Anthropic key. `--wire` and `--auth`
  do the same for one run.
- `scripts/install.sh` installs the CLI from the latest release: it picks the
  binary for the system, checks its checksum and puts it in `$PREFIX/bin`.
  The release workflow publishes a binary per platform and the checksums that
  go with them.
- Every request can carry extra headers. `openrouter` gets the attribution
  headers by default (`HTTP-Referer`, `X-OpenRouter-Title`,
  `X-OpenRouter-Categories`), so the app shows up in OpenRouter's rankings,
  and `providers.<id>.headers` in the configuration changes them or adds
  others.
- A session that moved less than one percent of the total is left out of the
  session table, and the report says how many were left out.
- The telemetry report is a table now: a bar shows how many tokens each group
  moved against the busiest one, coloured by the cache — green for the prompt
  the provider served from its cache, red for the part it had to read — and the
  columns carry the requests, the tokens in and out, the reasoning tokens, the
  cache hit rate weighted by tokens, the time to the first token, the latency
  and the cost. The groups are ordered by volume and the time group is named
  after `--bucket`.
- `--bucket week` groups the telemetry by ISO week.
- `Service::with_interceptor` registers an interceptor the engine consults at
  the safe points of a turn. It may add messages to the history or stop the
  turn with a reason, which the client hears as `TurnStopped`: a decision, not
  a failure.
- `xhigh` and `max` are first-class reasoning efforts. The catalogue values map
  to Norte's new variants, `--reasoning` and the config accept them without
  collapsing to `high`, and validation still rejects them for a model that does
  not declare them. OpenAI serializes both, Anthropic sends
  `output_config.effort`, and Gemini uses its `HIGH` ceiling.
- `SetReasoning` stores a session-wide override or a one-message override. Both
  are validated against the selected model before they are saved, survive a
  restart, and are applied by the next turn; a model switch that cannot honour
  the stored setting is refused instead of silently lowering it.
- `Steer` hands a message to the running turn: the engine appends it to the
  history before the next request, so the model sees it there. The message
  stays durable until the engine reports it applied, and whatever the turn
  never reached waits for the next turn boundary.
- `SetModel` is accepted while the session is busy. The switch is validated,
  persisted and applied at the next idle turn boundary, and the request answers
  with `ModelQueued`; `ModelChanged` is emitted when the switch is actually
  applied. An approval or question continuation is not a turn boundary.
- `ListGrants` returns the approval grants a session holds, with their id,
  tool, scope and state, and `GrantList` carries them back.
- The interactive chat accepts `/reasoning`, `/model` and `/revoke` next to the
  commands it already had. Every command with arguments parses into a closed
  set: an unknown setting, scope or argument is rejected instead of being sent
  to the model as a message.
- The input editor suggests the command names and their arguments in dimmed
  text, and Tab accepts the suggestion.
- The interactive footer shows the session's todo list above the session
  totals: the completed count, the current item first, pending items that fit,
  and `+N` for those that do not. It updates when a `todo` call succeeds and is
  cleared by an empty list; model-provided content is folded onto one line and
  escaped before it reaches the terminal. The built-in prompt and the tool
  description now ask the model to call `todo` before multi-step work and after
  each step.

### Changed

- Depend on the Norte revision that explains a literal `\n`, `\r` or `\t` in a
  failed `edit` search and reports an ambiguous exact match as occurrences, so
  the model sees the escaping mistake or the real count instead of only a
  similarity score.
- The built-in system prompt now states the Norte Code identity, requires tool
  use and verification, and tells the model not to guess its harness or claim
  to be another product.
- Terminal styling now uses 24-bit RGB when `COLORTERM` advertises truecolour,
  with the 256-colour palette as the fallback. Themes that remap the classic
  16-colour slots (such as a monochrome kitty theme) previously rendered every
  label in the same foreground colour.
- Tool calls render in blue, and a `command` call shows the command it ran even
  under `--quiet`; a long command is clipped to one line. Cancelling a turn
  before the provider reports usage now explains that the cancelled request is
  still recorded in telemetry.
- The interactive footer shows that a turn is working as soon as it is sent, and
  shows the retry delay and attempt while a transient provider failure waits.
- The local message queue is gone. Text typed while a turn runs is steered into
  that turn: the engine appends it to the history before the next request the
  turn makes, so the model sees it there instead of after the turn ends. The
  message stays durable until the engine reports it applied, and whatever the
  turn never reached waits for the next turn boundary. The commands that make
  sense during a turn are handled straight away. The footer row that showed the
  queue now shows the command suggestion.
- A steering message carries a name. The client may name it, the engine reports
  the ones it applied with `SteerApplied`, and the service drops the durable
  copy of exactly those. The client no longer counts messages to know whether
  one of them landed.
- `budget.action = "ask"` asks for a new limit in the interactive CLI. The turn
  stops uncommitted, the CLI prompts for a limit, and a new limit raises the
  budget and resumes the session. An empty answer leaves the session stopped
  and resumable.
- A second `Ctrl-C` force-quits immediately instead of waiting five seconds, and
  the first one now reports that the running tool is being stopped.

### Security

- Model answers, reasoning, tool output, skill bodies and resources, approval
  previews and catalogue or report text are escaped before they reach the
  terminal. ANSI escapes and bidi overrides can no longer move the cursor,
  rewrite a line, spoof an approval prompt or change the terminal title;
  newlines and tabs are kept.
- Skill discovery refuses a `SKILL.md` that resolves outside its skill
  directory, so a project cannot point it at `/etc/passwd` or another host file.
- `web_fetch` accepts only public HTTP(S) targets. Loopback, private,
  link-local, CGNAT, benchmark, documentation and reserved IPv4/IPv6 addresses
  (including cloud metadata endpoints) are refused, every resolved address is
  checked and pinned for the request, proxies are bypassed, and each redirect is
  validated before the next hop.
- `skill_script` keeps at most the configured output bytes per stream while
  draining the pipes, so a script that floods stdout or stderr cannot exhaust
  host memory before the timeout stops it.
- `skill_resource` reads at most the configured bytes plus one instead of
  loading the whole file, and `SKILL.md` and shebang parsing have the same kind
  of bound, so a large or hostile skill file cannot exhaust memory first.
- Config loading reads at most 1 MiB, API key files at most 64 KiB, and both
  must be regular files. A project overlay can no longer point `api_key_file` at
  a FIFO or an endless device and hang or exhaust the host at startup.
- The catalogue cache and download are read in bounded chunks, so a corrupt
  cache file or an oversized response is refused instead of being loaded whole.
- `grep` reads at most its file cap even if the file grows after the size check,
  so a changing file cannot make the tool allocate without bound.

### Fixed

- An `edit` call always renders each change in its own boxed, side-by-side
  view with original and result line numbers plus two lines of surrounding
  context, including under `--quiet` and when `--verbose` is off. The host
  snapshots the file immediately before the edit runs, so the preview still
  shows the original lines when the tool finishes quickly. A failed edit shows
  its diagnostic in the same modes.
- A forked session receives a fresh engine session id, so it no longer collides
  with its source, and it is recorded as a fork rather than a compaction.
- `glob` and `grep` honor `.gitignore` in a workspace without a `.git` directory
  and skip hidden files and directories.
- Resizing the terminal no longer duplicates or breaks streamed text. Resize
  events are debounced, the footer rows for the old and new sizes are cleared,
  and the scroll region is reset in one atomic write so the renderer never
  continues a line the terminal has just reflowed.
- The catalogue cache writes a unique temporary file with `create_new` and
  renames it, so a concurrent or interrupted write cannot replace a good cache
  with a partial one.
- A global approval update writes the user config atomically, keeps a symlinked
  config a symlink, preserves an existing mode, and creates a new config with
  owner-only permissions.

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
- `--interactive` on `run` and `resume`: the same session stays open. A pinned
  footer holds the running session totals, the queued-message preview and a line
  editor. The totals update after every request (requests, input/output tokens,
  average cache share, total input:output ratio and reported cost). Messages you
  send are echoed above the footer; lines completed while a turn is still
  running stay in the queued preview and are sent together as the next message,
  even after a failed or cancelled turn. Arrow keys, Home/End, Delete,
  Backspace, paste and the usual control keys work; a reverse-video cursor marks
  the insertion point and typed text never leaks into the answer. `/exit`,
  `quit`, Ctrl-D or Ctrl-C at the prompt leave; a first prompt is optional on
  `run`. The prompt shares stdin with approvals and questions so typed input
  stays in order.
- `tools.command_max_model_bytes` (default `131072`, range `1024`–`1048576`)
  bounds the bytes of each `command` stream that reach the model; the rest is
  elided around the marker. Previously this was fixed at Norte's 32 KiB default
  and could not be raised from the config.
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

### Fixed

- `grep` and `web_fetch` now escape Unicode bidi overrides and line separators
  the way `read` does, so a crafted line cannot visually reorder or hide the
  rest of a result. `grep` also clips its 500-character line width by
  characters instead of bytes, matching `read` for accented text.

[Unreleased]: https://github.com/JeanVydes/NorteCode/commits/dev
