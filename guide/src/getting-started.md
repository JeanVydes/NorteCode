# Getting started

## Prerequisites

- Rust 1.88 or newer.
- An API key for at least one provider, or an OpenAI-compatible endpoint you
  can reach.
- Linux or macOS if you want the command sandbox (it needs `bwrap` on Linux or
  `sandbox-exec` on macOS; without them the sandbox fails closed).

## Install

```sh
cargo install --git https://github.com/JeanVydes/NorteCode norte-code-cli --force
```

`--force` (re)installs the binary, so running the same command is also how you
update to the latest `dev` build.

During development:

```sh
cargo run -p norte-code-cli -- --help
```

## Name a model and a key

The server reads its configuration from the platform config directory, merged
with an optional `.norte-code/service.json` project overlay. The CLI reads only
its own `cli.json`; it does not load providers, models, keys or server
settings. Start the local service first:

```sh
norte service up
```

Follow the service log with:

```sh
norte service logs --live
```

The smallest useful server file is:

```json
{
  "version": 1
}
```

Install the recommended OpenRouter catalogue:

```sh
norte plugins add https://github.com/JeanVydes/catalog-plugin --package catalog-plugin-openrouter
```

Then export the key and check the server catalogue:

```sh
export OPENROUTER_API_KEY=sk-or-...
norte models --provider openrouter --tools
```

The catalogue is two files you own beside the config — `providers.json` (who
serves models) and `models.json` (the models). Nothing is fetched or bundled:
the bundled files are empty and a missing file is also created empty.

**Recommended for OpenRouter:** install its catalogue from the trusted
[catalog-plugin](https://github.com/JeanVydes/catalog-plugin) instead — no
catalogue files to maintain:

```sh
norte plugins add https://github.com/JeanVydes/catalog-plugin --package openrouter
```

The server loads it before the catalogue is read, so its providers and models
appear in `norte models` and `/model`, and your files stay untouched.
(Installing a plugin clones and builds untrusted code — including its
`build.rs` — so only do it for sources you trust.)

See [Configure a provider](guides/configure-a-provider.md) for the full shapes
and where keys come from, [Plugins](guides/plugins.md) for how plugins work,
and the [configuration reference](reference/configuration.md) for every config key.

## Run your first prompt

```sh
norte new
```

`new` creates a session and stays in it: the first line you type is the first
message. For a headless one-shot, pipe newline-delimited messages:

```sh
printf '%s\n' 'Explain this project' | norte new --jsonl
```

`--jsonl` emits every service event plus client diagnostics (connection,
input, request lifecycle, errors and completion), one JSON object per line.

It prints the session key, so the next command can continue it:

```sh
norte resume <key>
```

## Stay in the session

While a turn streams, text you type is steered into that turn and reaches the
request it is about to make. The footer shows the todo list, the running
session totals and a suggestion for the command you are typing. Type `/help`
for the chat commands, `/exit` or Ctrl-D to leave.

## Inspect what happened

```sh
norte sessions
norte cost
norte telemetry --bucket day
norte skills list
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
