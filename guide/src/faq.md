# FAQ

## Is there a server to start?

Yes. The CLI is a stateless client. Start the local service with
`norte-code service start`, or set `service_type` to `remote` and
`service_url` in `cli.json`.

## Can I use it from a script?

Yes. Pipe newline-delimited messages to `norte-code new --jsonl`; it emits
service events and client diagnostics as JSONL. See
[Script with JSONL](guides/script-with-jsonl.md).

## Is there a TUI?

Yes. The terminal client is the interactive UI; `--jsonl` is the headless
alternative.

## Does it work with OpenRouter?

Yes. OpenRouter is an OpenAI-compatible chat endpoint. Upstream routing goes
through `--extension 'openai.chat:provider.order=["deepinfra"]'`.

## Does it compute costs?

No. It only reports what the provider reported. A request the provider did
not price stays unpriced — it is not treated as free, and a USD budget skips
it instead of guessing.

## Why did a session stop?

It may be waiting on a question, stopped by a budget, or interrupted by
Ctrl-C. `norte-code sessions` shows the phase; `norte-code resume <key>`
continues a parked or stopped session.

## Is my code sent anywhere else?

The only network destination is the model endpoint you configured. The
catalogue is two local files, telemetry is local SQLite, and there is no
remote crash reporting, update check or analytics.

## Are commands sandboxed by default?

No. `tools.confinement.sandbox.enabled` defaults to `false`: the default is
an argument gate, not OS isolation, so `shell` may reach outside the
workspace. Enable it with `"tools": {"confinement": {"sandbox": {"enabled": true}}}`
or `--sandbox` — see [Use the sandbox](guides/use-the-sandbox.md).
`skill_script` runs outside the command sandbox and stays opt-in.
