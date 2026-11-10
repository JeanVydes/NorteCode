# Norte Code

Norte Code is the RAEP server for the Norte agent, plus a Dioxus client that
talks to it over the same protocol any other client does. The server owns
sessions, execution, catalogues and plugins; a client attaches to a session
and sends the next user input.

## Build

Rust 1.88+:

```sh
cargo build --release
```

The server binary is `target/release/norte-code-server`. It listens on
`127.0.0.1:8787` and keeps its state beside `NORTE_CODE_DATABASE` (default
`norte-code.db`).

## Run

```sh
NORTE_CODE_DATABASE=./norte-code.db cargo run -p norte-code-raep
```

The client is a Dioxus app:

```sh
cd norte-code-app && dx serve
```

A few things worth knowing:

- **Approvals**: tools run only under your rules; a client answers each
  approval over the protocol, per tool, per session or globally.
- **Plans**: the model proposes work as a Markdown file and asks your
  approval before it becomes active.
- **Budgets**: cap spend in USD, tokens or requests per session or turn.
- **Sandbox**: opt-in OS containment for `shell`.
- **Compaction, forks and rewind**: history is a node graph, so a session can
  branch, rewind to a turn, or replace its history with a summary.

## Providers and models

The server reads two files beside its configuration — `providers.json` and
`models.json`. They are empty by default: nothing is fetched or bundled. Keys
come from the environment or a private `secrets.json` (mode 0600); the
stateless CLI never reads them.

Install the OpenRouter catalogue through the trusted
[catalog-plugin](https://github.com/JeanVydes/catalog-plugin) instead of
maintaining those files:

```sh
norte-code plugins add https://github.com/JeanVydes/catalog-plugin --package catalog-plugin-openrouter
```

## RAEP server

The RAEP server keeps the NorteCode harness on the server. A client does not
send the provider request or rebuild the model history; it attaches to a
session and sends the next user input. The server loads that session's
durable history, appends the input, and runs the internal `generate` operation.

An attach frame creates or opens a session:

```json
{
  "version": "1.0",
  "id": "attach-001",
  "session_id": "",
  "timestamp": 1730000000000,
  "type": "session_attach",
  "payload": {
    "agent_id": "norte-code",
    "create_if_missing": true,
    "mode": "controller"
  }
}
```

The client then sends the user message on the returned session:

```json
{
  "version": "1.0",
  "id": "input-001",
  "session_id": "session-123",
  "timestamp": 1730000001000,
  "type": "user_input",
  "payload": {
    "prompt": "Explícame cómo funciona RAEP"
  }
}
```

The server translates that frame to a NorteCode `SendMessage` request. The
result is streamed as RAEP events, including text deltas, tool calls,
approvals, questions, jobs, usage and completion. To recover events after a
disconnect, the client attaches with `resume_from_seq`; it does not resend the
conversation history. Each session's frames are kept in
`<database stem>.replay/<session>.jsonl`, capped by bytes, so sequence replay
survives a server restart; an attach whose sequence is older than what the file
still holds comes back truncated and the client reloads history. Artifact uploads go directly into Norte's
content-addressed BlobStore; no second artifact directory is created. The
common event families use canonical RAEP payloads (`turn`, tools, jobs,
questions, budget, usage, retries and errors). The original NorteCode event is
kept under envelope `ext.norte_code` for lossless compatibility while the
session, history, compaction, agent/catalogue and cost events are normalized
as well.
`norte_code.command` extension is available when a
client needs the complete NorteCode command surface, including model, budget,
steering, jobs and session operations.
Common session operations use the standard `SESSION_CONFIGURE` frame with an
`operation` enum (`list_jobs`, `stop_jobs`, `set_model`, `set_reasoning`,
`set_budget`, `answer_question`, `resume`, or `compact`), so clients do not
need the NorteCode extension for those controls.

Set `NORTE_CODE_AUTH_TOKEN` to require a bearer `Authorization` header on
WebSocket, SSE, artifact upload and session-command routes. Health and
discovery endpoints remain public for orchestration probes. Upload binary
content with `POST /raep/v1/artifacts` and use the returned `id` as
`content[].source.artifact_id` in a multimodal `USER_INPUT`. An uploaded blob
can be read back with authenticated `GET /raep/v1/artifacts/{artifact_id}`.

## Documentation

- **[CONFIG.md](CONFIG.md)**: the full configuration schema.
- **[AGENTS.md](AGENTS.md)**: the crate layout and the rules that keep it
  that way.
- **`guide/`**: written for the terminal client that no longer ships; it is
  being rewritten for the app and the protocol.

## License

MIT. See [LICENSE](LICENSE).
