# FAQ

## Is the service a server?

No. `Service` is a Rust library. It exposes request/event streams and runs in the
same process as the CLI. The protocol is transport-agnostic, but only the
in-process tokio transport exists today.

## Can I use Norte Code as a library?

Yes. Depend on `norte-code-agent` and `norte-code-protocol`, build a `Service`
with a store, blob store, model factory and catalogue, then submit
`ClientRequest` values. The CLI is a normal client of that API.

## Where is the TUI?

`norte-code-tui` is a placeholder. The CLI is the supported client, and the
protocol is designed so the TUI can reuse it without a rewrite.

## Does it work with OpenRouter?

Yes. OpenRouter is an OpenAI-compatible chat endpoint. The provider entry uses
the `openai.chat` wire, and optional vendor fields such as upstream routing go
through `--extension 'openai.chat:provider.order=["deepinfra"]'`.

## Does it compute costs?

No. The ledger stores the amount the provider reported and stores `NULL` when it
reported none. Budgets in USD are skipped for unpriced requests rather than
treating them as free.

## Why did a session stop?

It may be parked for an approval (`AwaitingApproval`), waiting on a question,
stopped by a budget, or interrupted by Ctrl-C. `norte-code sessions` shows the
phase; `norte-code resume <key>` continues a parked or stopped session.

## Is my code sent anywhere else?

The only network destinations are the configured model endpoint and models.dev
for the catalogue. Telemetry is local SQLite. There is no remote crash
reporting, update check or analytics.

## Are commands sandboxed by default?

No. `tools.sandbox.enabled` defaults to `false`. When enabled, Linux uses
bubblewrap and macOS uses `sandbox-exec`; a missing binary fails closed. The
sandbox contains writes, process namespaces and the network, but it is not a
virtual machine. `skill_script` is outside the command sandbox and remains opt-in.
