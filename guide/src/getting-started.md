# Getting started

## Prerequisites

- Rust 1.85 or newer.
- Linux or macOS for the supported command sandbox. The rest of the host builds
  on other platforms, but the sandbox fails closed when its platform binary is
  missing.
- An API key for at least one provider, or an OpenAI-compatible endpoint you can
  reach.

## Build

```sh
cargo build
```

The binary is `target/debug/norte-code`. During development the same command is
available through Cargo:

```sh
cargo run -p norte-code-cli -- --help
```

`norte-code` is the headless client. It builds the service in the same process;
there is no daemon to start first.

## Name a model and a key

Norte Code reads a user config from the platform config directory, merged with
an optional `.norte-code/config.json` project overlay. The smallest useful user
file is:

```json
{
  "version": 1,
  "default_model": "openrouter/anthropic/claude-sonnet-4",
  "providers": {
    "openrouter": {
      "api_key_env": "OPENROUTER_API_KEY"
    }
  }
}
```

Then export the key:

```sh
export OPENROUTER_API_KEY=sk-or-...
```

Replace the model with one the catalogue actually knows:

```sh
norte-code models --provider openrouter --tools
```

The full schema, including file-based keys, endpoint overrides, budgets,
compaction and the sandbox, is in the [configuration reference](reference/configuration.md).
The repository also keeps the longer [`CONFIG.md`](https://github.com/JeanVydes/NorteCode/blob/dev/CONFIG.md).

## Run one prompt

```sh
norte-code run "Explain the repository layout"
```

Text goes to stdout. Reasoning, tool activity and usage go to stderr when they
are enabled. The command prints the session key, so the next command can
continue the same conversation:

```sh
norte-code run --session <key> "Now list the tests"
```

`--model` overrides the default for this run:

```sh
norte-code run --model openrouter/anthropic/claude-sonnet-4 "review the diff"
```

## Stay in the session

```sh
norte-code run --interactive "start here"
```

The prompt keeps the same session open. It is a line editor: the arrow keys,
Home/End, Delete and paste work. Your message is echoed above the pinned footer;
while a turn streams, the lines you type stay in its queued preview and are sent
as the next message. The footer also shows the todo widget (completed count,
current item and pending items), and the running session totals, updated after
each request. Type `/help` to see the chat commands, `/exit` or Ctrl-D to
leave. A first prompt is optional with `--interactive`:

```sh
norte-code run -i
```

## Inspect what happened

```sh
norte-code sessions
norte-code cost
norte-code telemetry --bucket day
norte-code skills list
```

`cost` reports provider-reported money; `telemetry` reports local request
counts, tokens, cache, tools and latency. Neither leaves the machine.

## Offline and tests

The catalogue can be read from its cache or bundled snapshot:

```sh
norte-code --offline models
```

The workspace checks are:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Hermetic tests use `norte-testing` scripted models and in-memory stores. Live
provider tests are ignored and key-gated. See
[Test without a network](guides/test-without-a-network.md).

## Where data lives

Sessions, approvals, pending questions, effect intents, costs and request
telemetry live in SQLite under the platform data directory, normally
`$XDG_DATA_HOME/norte-code/sessions.db` or
`~/.local/share/norte-code/sessions.db`. Deleting that file deletes the local
history; it is not a cache.
