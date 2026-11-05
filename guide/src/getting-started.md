# Getting started

## Prerequisites

- Rust 1.88 or newer.
- An API key for at least one provider, or an OpenAI-compatible endpoint you
  can reach.
- Linux or macOS if you want the command sandbox (it needs `bwrap` on Linux or
  `sandbox-exec` on macOS; without them the sandbox fails closed).

## Build

```sh
cargo build
```

The binary is `target/debug/norte-code`. During development:

```sh
cargo run -p norte-code-cli -- --help
```

## Name a model and a key

The server reads its configuration from the platform config directory, merged
with an optional `.norte-code/service.json` project overlay. The CLI reads only
its own `cli.json`; it does not load providers, models, keys or server
settings. Start the local service first:

```sh
norte-code service start
```

Use `-f` to keep following the service log after startup; press Ctrl-C to
stop following without stopping the service:

```sh
norte-code service start -f
```

The smallest useful server file is:

```json
{
  "version": 1,
  "default_provider": "openrouter",
  "default_model": "anthropic/claude-sonnet-4"
}
```

Then export the key:

```sh
export OPENROUTER_API_KEY=sk-or-...
```

Check the server catalogue knows the model:

```sh
norte-code models --provider openrouter --tools
```

The catalogue is two files you own beside the config — `providers.json` (who
serves models) and `models.json` (the models). Nothing is fetched or bundled:
if a file is missing it is written empty and the error names it.

**Not maintaining those files?** Install the
[catalog-plugin](https://github.com/JeanVydes/catalog-plugin) instead — it
offers a ready-made catalogue as a plugin, no files to write:

```sh
norte-code plugins add https://github.com/JeanVydes/catalog-plugin
```

The server loads it before the catalogue is read, so its providers and models
appear in `norte-code models` and `/model`, and your files stay untouched.
(Installing a plugin clones and builds untrusted code — including its
`build.rs` — so only do it for sources you trust.)

See [Configure a provider](guides/configure-a-provider.md) for the full shapes
and where keys come from, [Plugins](guides/plugins.md) for how plugins work,
and the [configuration reference](reference/configuration.md) for every config key.

## Run your first prompt

```sh
norte-code new
```

`new` creates a session and stays in it: the first line you type is the first
message. For a headless one-shot, pipe newline-delimited messages:

```sh
printf '%s\n' 'Explain this project' | norte-code new --jsonl
```

`--jsonl` emits every service event plus client diagnostics (connection,
input, request lifecycle, errors and completion), one JSON object per line.

It prints the session key, so the next command can continue it:

```sh
norte-code resume <key>
```

## Stay in the session

While a turn streams, text you type is steered into that turn and reaches the
request it is about to make. The footer shows the todo list, the running
session totals and a suggestion for the command you are typing. Type `/help`
for the chat commands, `/exit` or Ctrl-D to leave.

## Inspect what happened

```sh
norte-code sessions
norte-code cost
norte-code telemetry --bucket day
norte-code skills list
```

`cost` is provider-reported money; `telemetry` is local request counts,
tokens, cache, tools and latency. Neither leaves the machine.

## Where data lives

Sessions, pending questions, plans, costs and telemetry live in SQLite under
the platform data directory (normally
`$XDG_DATA_HOME/norte-code/sessions.db` or
`~/.local/share/norte-code/sessions.db`). Plans also live as Markdown under
`~/.local/norte-code/<session>/plans/`. Deleting the database deletes the
local history; it is not a cache.
