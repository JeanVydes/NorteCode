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

Norte Code reads a user config from the platform config directory, merged with
an optional `.norte-code/config.json` project overlay. The smallest useful user
file is:

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

Check the catalogue knows the model:

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

It loads before the catalogue is read, so its providers and models appear in
`norte-code models` and `/model` for that run, and your files stay untouched.
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
message. Speech-to-text style one-liners do not exist here — to send one
message and exit, use the protocol (see
[Script against the protocol](guides/script-with-jsonl.md)).

```sh
norte-code new --model openrouter/anthropic/claude-sonnet-4
```

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
