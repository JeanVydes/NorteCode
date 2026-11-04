# Norte Code

**A terminal-based agentic coding tool.** You open a session in a directory,
ask for work in plain language, and the model reads, writes and runs commands
in that directory — with your approval rules, your budgets and your model.

## Start here

1. [Getting started](getting-started.md) — build it, name a model, run your
   first prompt.
2. [Configure a provider](guides/configure-a-provider.md) — the two catalogue
   files and where your API key goes.
3. [Start a session](guides/run-a-prompt.md) — the flags that shape a run.

## Everyday use

- [Continue a session](guides/continue-a-session.md) — resume, the chat
  commands, Ctrl-C, compaction.
- [Plans](plans.md) — how the model proposes work for your approval, and how
  to read and edit those proposals.
- [Approve a tool](guides/approve-a-tool.md) — `/allow`, `/deny`, and what
  happens when a call is refused.
- [Set a budget](guides/set-a-budget.md) — stop or warn when a run costs too
  much.
- [Use the sandbox](guides/use-the-sandbox.md) — contain what `shell` can reach.
- [Plugins](guides/plugins.md) — install, update and trust third-party plugins, including the catalogue plugin.

## Reference

- [CLI](reference/cli.md) — every subcommand and flag.
- [Configuration](reference/configuration.md) — every config key.
- [Protocol](reference/protocol.md) — the request/event stream, for scripts.
- [Script against the protocol](guides/script-with-jsonl.md) — drive it from
  another program.
- [FAQ](faq.md) — server, TUI, costs, sandbox defaults.

## What it is not

- Not a daemon: there is no server to start. The CLI builds the service in
  the same process.
- Not a model catalogue of its own: models come from two files you own,
  `providers.json` and `models.json`. Nothing is fetched or baked in.
- Not a fork of its engine: if a change belongs in the engine, it goes there.
