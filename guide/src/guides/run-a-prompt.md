# Start a session

```sh
norte-code
```

With no arguments it asks which session to open — a new one, the last one,
or any saved one — then reads messages from the terminal. Skip the menu:

```sh
norte-code new
```

`new` creates a session and stays in it. It takes no prompt: the first line
you type is the first message.

## Choose a model

The default comes from `default_provider` / `default_model` in the config;
`--provider` / `--model` override it:

```sh
norte-code new --model openrouter/anthropic/claude-sonnet-4
```

When neither is set and no flag names them, startup asks which provider and
model to use and writes the answer to the config, so it asks once.
`norte-code models` shows what the catalogue knows.

## Change the prompt or endpoint

```sh
norte-code new --system-prompt "Answer as a terse reviewer."
norte-code new --endpoint https://openrouter.ai/api/v1
```

`--system-prompt` replaces the base prompt for this session; `AGENTS.md` in
the workspace is appended after it. `--endpoint` overrides the provider
endpoint without editing the config.

## Reasoning and limits

```sh
norte-code new --reasoning high
norte-code new --max-iterations 12
norte-code new --extension 'openai.chat:provider.order=["deepinfra"]'
```

Reasoning is validated against the selected model before the turn starts
(and is also `/reasoning` once open). `--max-iterations` caps model
iterations; `--extension` is repeatable and malformed JSON is rejected
before the turn.

## How much to show

```sh
norte-code new --verbose high
```

- `high` — each tool call's arguments and result, plus reasoning.
- `medium` (default) — that tools run and the model is thinking.
- `low` — the answer alone.

`/debug` at the prompt prints startup paths, model capabilities, effective
reasoning and prompt provenance. It never prints keys or endpoint URLs.

## Sending one message and exiting

`new` is for a session someone reads. To send one message and read the
events programmatically, speak the protocol — see
[Script against the protocol](script-with-jsonl.md).

## Exit status

`0` normal end, `1` failed turn, `2` command-line or configuration error.
A turn parked on a question is not an error: the session stays resumable.
