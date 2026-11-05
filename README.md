# Norte Code

A terminal-based coding agent. Open a session in a directory, ask for work
in plain language, and the model reads, writes and runs commands there —
with your approvals, your budgets and your model.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/JeanVydes/NorteCode/dev/scripts/install.sh | sh
```

Or build it from source (Rust 1.88+):

```sh
cargo build --release
```

## Quick start

```sh
norte-code
```

Pick or create a session and start typing. A few things worth knowing:

- **Approvals** — tools run only under your rules. `/allow` and `/deny`
  decide per tool, per session or globally.
- **Plans** — the model proposes work as a Markdown file and asks your
  approval before it becomes active. `/plans` lists them.
- **Budgets** — cap spend in USD, tokens or requests: `--budget` on the
  command line or `budgets` in the config.
- **Sandbox** — opt-in OS containment for `shell` (`--sandbox`).

## Providers and models

Models come from two files you own beside the config — `providers.json`
and `models.json`. Nothing is fetched or bundled. Keys come from the
environment or a private `secrets.json` (mode 0600); they are never in
`config.json`.

Prefer not to maintain the files? The
[catalog-plugin](https://github.com/JeanVydes/catalog-plugin) offers a
ready-made catalogue as a plugin:

```sh
norte-code plugins add https://github.com/JeanVydes/catalog-plugin
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
conversation history. The server binary persists the RAEP frame journal beside
`NORTE_CODE_DATABASE` with the `.raep.jsonl` suffix, so sequence replay also
survives a server restart. Artifact uploads go directly into Norte's
content-addressed BlobStore; no second artifact directory is created. The
`norte_code.command` extension is available when a
client needs the complete NorteCode command surface, including model, budget,
steering, jobs and session operations.

Set `NORTE_CODE_AUTH_TOKEN` to require `Authorization: Bearer <token>` on
WebSocket, SSE, artifact upload and session-command routes. Health and
discovery endpoints remain public for orchestration probes. Upload binary
content with `POST /raep/v1/artifacts` and use the returned `id` as
`content[].source.artifact_id` in a multimodal `USER_INPUT`. An uploaded blob
can be read back with authenticated `GET /raep/v1/artifacts/{artifact_id}`.

## Documentation

- **[Guide](https://jeanvydes.github.io/NorteCode/)** — getting started,
  sessions, plans, approvals, budgets, sandbox, plugins and the full CLI
  and config reference. Run `mdbook serve guide` to read it locally.
- **[CONFIG.md](CONFIG.md)** — the full configuration schema.

## License

MIT. See [LICENSE](LICENSE).
