# FAQ

## Is there a server to start?

No. There is no daemon, port or socket. `norte-code` builds everything in its
own process and talks to the service in-process.

## Can I use it from a script?

Yes, two ways: drive the CLI from a shell, or speak the request/event
protocol directly. See [Script against the protocol](guides/script-with-jsonl.md).

## Is there a TUI?

No. The terminal prompt is the only client. The protocol is shaped so a
future client could reuse it, but none ships.

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
