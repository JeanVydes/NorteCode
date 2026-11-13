# TUI implementation plan

## Architecture

- Use `norte-code-client` for host requests and `norte-code-wire` for shared
  protocol types.
- Keep views and dialogs component-based, with explicit component lifecycle and
  state.
- Keep presentation state in `TuiPreferences`; use enums for visibility and
  toggles instead of unrelated booleans.
- Use background surfaces and sparse separators instead of full box borders.
  Keep modal clearing and native terminal text selection.
- Keep all UI colors in the shared palette, with pastel pink accents in dark
  mode and magenta accents in light mode.
- Session commands that need an attachment go through the client's
  `AgentClient`, so the client keeps the attachment, takes it again when the
  host says it lost it, and never leaves a client with a stale belief.

## Implemented

- Chat sessions: create, select, rename, delete with confirmation, fork, queue
  modes, model selection, streaming history/deltas, tool output and read-only
  child sessions.
- Transcript: markdown, responsive tables, right-aligned user messages on their
  own background, assistant messages on their own background, collapsible
  reasoning on a moving palette flare, timestamps, tool output preferences, and
  unified or split edit diffs.
- Composer: focused by default, the queue mode with its meaning under the
  input, the reasoning effort and the session model in the status line, and a
  task list panel above it with the model's own list.
- Chips: a piece of content too long to show inline becomes a chip, a file
  the reader attaches is a chip, and a chip is deleted as a whole. The chips
  travel with the message and are drawn under it in the transcript.
- Turn control: the running turn can be stopped (`C` in the transcript,
  `Ctrl+C` from the composer), the reasoning effort cycles (`r`), and a turn
  can be rewound (`R`) or pinned (`P`) from a picker of the turns the reader
  wrote.
- Background jobs: the jobs the host knows are listed when the session opens,
  the running ones get a panel of their own with their last output, the
  transcript says when one starts and how it ended, and a running job can be
  stopped (`S` and the job's number) through the host's `CANCEL_JOB`.
- Notices: the model the session runs on, a model change that waits for the
  turn to end, and a footer under each answer with the tokens, the duration
  and the cost the provider reported.
- Turn interactions: plan approval, denial and feedback, plus single-choice,
  multiple-choice and free-form questions.
- Session actions: budget, export, and compact; compact refreshes the transcript
  from the client history API.
- Shared configuration: `~/.config/norte-code/client.json` stores service URL,
  service type, optional token, agent ID, and shared preferences. Terminal-only
  diff and chart settings live under `cli`. The former `cli.json` is migrated
  on first load, unknown fields are preserved, and writes are atomic with
  owner-only permissions on Unix. The TUI no longer reads its service URL or
  token from environment variables.
- Budget behavior: the TUI sets a USD warning budget. Crossing it reports a
  warning and does not stop the turn; the transcript explains this at entry
  and when the host reports the configured budget. The action is `warn`, `ask`
  or `stop`, `ask` by default, and it is persisted in `client.json`. With `ask`
  the turn pauses for one approved continuation, which the composer answers.
- Compaction: streamed completion is rendered as “Session compacted.” without
  displaying the generated summary; the full history is then reloaded.
- Export: the TUI requests a session bundle and reports its download URL.
- Host: connection diagnostics, health, quota, capabilities, skills, MCP,
  tool approvals, plugins, telemetry, and provider-reported costs.
- Catalogue: search and model selection, provider/model create and remove,
  with plugin-provided entries read-only.
- Settings: light/dark theme, reasoning visibility, timestamps, tool output,
  session refresh, and developer diagnostics.
- Session composer and transcript occupy separate layout regions, including
  short terminal heights, so the composer does not cover transcript content.
- Split app client-data, event, and session-state handling into focused modules;
  split provider catalogue form behavior from the catalogue view.
- Strict Clippy warnings addressed in the TUI and related client/agent crates.
- Shared client: the host's refusal is read from its error envelope, so a
  refusal reads as a sentence instead of raw JSON, and a session that was
  detached while the client held it is attached again and the request sent once
  more.
- The app reads a session's cost report without attaching and detaching it, so
  the costs page cannot take the attachment away from the open conversation.
- The host lets an observer attachment run the commands that only read state.
- The server and the interface stop on `Ctrl+C` and on `SIGTERM`: the server
  closes its listener and says it is shutting down, and the interface restores
  the terminal before it leaves.
- Attachments: a file the reader attaches is read, checked against what the
  host and the agent accept, uploaded to the session, and kept as a chip
  until the message goes out. A host that does not accept uploads says so
  instead of failing at the upload.

## Remaining limitations

1. **Live action smoke test:** The configured host answers its readiness
   endpoint, and the client/agent/RAEP test suites pass. No live budget,
   export, or compaction operation was run: compaction may call a model and
   incur provider cost, and export creates a host-side artifact. Run those
   against a disposable session when live verification is explicitly desired.
2. **Catalogue editing:** The current catalogue detail API does not provide
   sufficient source data to safely edit existing provider/model definitions.
   Create and remove are available; existing plugin entries remain read-only.
3. **Additional session workflows:** Rewind, pinning and background jobs are in
   place. Uploads, session import, artifact download and rewind of a session
   the reader did not write are not exposed yet; add them only through
   existing client APIs and with clear terminal interaction flows.
4. **Developer mode:** Its value is migrated and persisted, but it does not yet
   gate or change diagnostics; define its intended behavior before treating
   that preference as functional.
5. **One event stream per session, with reconnection:** The stream is the
   attachment: the host binds the session to it and releases it when the
   stream closes. If the connection drops, the interface takes the stream
   again with backoff and reloads the history, so a dropped turn is filled in
   instead of lost silently. The chips of a message are visual only; the host
   does not keep them in the history, so a message that arrives from the
   history is drawn without chips.
6. **The app still reads the event stream itself:** `norte-code-app/src/sse.rs`
   holds the app's own platform transport for the event stream. It belongs in
   `norte-code-client`, which already owns the decoder and the endpoint
   catalog, with the app supplying only the platform transport the way the
   client expects. The native transport the TUI uses lives in the client
   today; the wasm one does not. Move it, then delete the app's reader.
7. **Attachments are guessed:** The media type of a file is guessed from its
   name, because a terminal has no platform to ask. A file with a name the
   client does not know is refused rather than sent as something it is not.

## Verification

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace`

Latest results: formatting and strict Clippy passed; 101 TUI, 39 client, 199
agent, and 71 RAEP tests passed. One agent test and two agent doctests are
ignored. The RAEP test suite includes export endpoint coverage; the agent suite
includes budget and manual-compaction coverage.
