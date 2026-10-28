# Start a session

## The basic shape

```sh
norte-code
```

With no arguments the command asks which session to open — a new one, the last
one, or any of the saved ones — and then reads messages from the terminal. The
first line typed is the first message of the session; `/exit` leaves.

To skip the menu:

```sh
norte-code new
```

`new` creates a session and stays in it. It takes no prompt: a session is read,
not handed a message.

## Choose a model

The default comes from `default_provider` and `default_model` in the config,
and `--provider`/`--model` override them:

```sh
norte-code new --model openrouter/anthropic/claude-sonnet-4
```

When `default_provider`/`default_model` are not configured and no flag names
them, the startup flow
asks: it lists the providers named in the configuration first and the catalogue's
after them, then the models of the one chosen, and writes the answer to the config
file as `default_provider`/`default_model`, so the question is asked once.
`norte-code models` shows
what the catalogue knows.

## Change the prompt or endpoint

```sh
norte-code new --system-prompt "Answer as a terse reviewer."
norte-code new --endpoint https://openrouter.ai/api/v1
```

`--system-prompt` replaces the base prompt for this session. `AGENTS.md` in the
workspace is appended after it. `--endpoint` overrides the provider endpoint
without editing the config.

## Reasoning and approvals

```sh
norte-code new --reasoning high
norte-code new --approval-mode auto-edit
norte-code new --approval-mode yolo
```

The reasoning setting is validated against the selected model before the turn
starts. `auto-edit` lets edits run but still asks for commands; `yolo` runs
everything. Deny rules still win in `yolo`. Both are also at `/reasoning` and
`/allow`, `/deny` once the session is open.

## Limits and extensions

```sh
norte-code new --max-iterations 12
norte-code new --extension 'openai.chat:provider.order=["deepinfra"]'
```

`--max-iterations` caps the engine's iterations. `--extension` is repeatable;
malformed JSON is rejected before the turn.

## Verbs and levels

```sh
norte-code new --verbose high
norte-code --verbose low
```

- `--verbose high` prints each tool call's effective arguments and result,
  shortened when large.
- `--verbose medium` (the default) also says that a tool is running and that the
  model is thinking.
- `--verbose low` shows the answer alone.
- `/debug` at the prompt prints startup paths, model capabilities, effective
  reasoning and prompt provenance. It never prints keys or endpoint URLs.

## Sending one message

`new` is for a session someone reads. A client that wants to send one message and
read the events speaks the protocol instead; see
[the protocol reference](../reference/protocol.md) and
[scripting against it](../guides/script-with-jsonl.md).

## Exit status

`0` means the session ended normally. A failed turn exits with `1`. A
command-line or configuration error exits with `2`. A turn that parks for
approval or a question is not an error; the session is left in a resumable phase.
