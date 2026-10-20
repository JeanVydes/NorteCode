# Run a prompt

## The basic shape

```sh
norte-code run "Explain the repository layout"
```

`run` creates a session unless `--session` names an existing one, streams the
answer, and exits. Text is written to stdout; reasoning, tool activity and usage
go to stderr.

## Choose a model

The default comes from `default_model` in the config, or from `--model`:

```sh
norte-code run --model openrouter/anthropic/claude-sonnet-4 "review the diff"
```

If neither names a model, the command explains both options instead of guessing.
`norte-code models` shows what the catalogue knows.

## Change the prompt or endpoint

```sh
norte-code run --system-prompt "Answer as a terse reviewer." "review"
norte-code run --endpoint https://openrouter.ai/api/v1 "review"
```

`--system-prompt` replaces the base prompt for this run. `AGENTS.md` in the
workspace is appended after it. `--endpoint` overrides the provider endpoint
without editing the config.

## Reasoning and approvals

```sh
norte-code run --reasoning high "plan the migration"
norte-code run --approval-mode auto-edit "fix the typo"
norte-code run --approval-mode yolo "run the formatter"
```

The reasoning setting is validated against the selected model before the turn
starts. `auto-edit` lets edits run but still asks for commands; `yolo` runs
everything. Deny rules still win in `yolo`.

## Limits and extensions

```sh
norte-code run --max-iterations 12 "keep going until the tests pass"
norte-code run \
  --extension 'openai.chat:provider.order=["deepinfra"]' \
  "review the patch"
```

`--max-iterations` caps the engine's iterations for this run. `--extension` is
repeatable; malformed JSON is rejected before the turn.

## Output modes

```sh
norte-code run --quiet "summarise the module"
norte-code run --verbose "fix the failing test"
norte-code run --debug "why did that fail?"
norte-code run --jsonl "list the files"
```

- `--quiet` redacts reasoning to one `thinking…` marker per turn and hides tool
  activity and usage.
- `--verbose` prints each tool call's effective arguments and result, shortened
  when large.
- `--debug` prints startup paths, model capabilities, effective reasoning and
  prompt provenance. It never prints keys or endpoint URLs.
- `--jsonl` writes one protocol event per line to stdout and nothing else.

## Exit status

`0` means the turn ended normally. A failed turn exits with `1`. A command-line
or configuration error exits with `2`. A turn that parks for approval or a
question is not an error; the session is left in a resumable phase.
