# Configuration

## Two files, one object

Norte Code merges two optional JSON files:

1. The user file: `$XDG_CONFIG_HOME/norte-code/config.json`,
   `~/.config/norte-code/config.json`, or `%APPDATA%\norte-code\config.json`.
2. The project overlay: `.norte-code/config.json` in the workspace or the
   nearest parent directory that has one.

The merge is shallow and key-by-key: a key present in the project overlay wins,
and missing keys keep the user value or the default. `--config PATH` replaces
the user file path.

Unknown keys are kept out of the typed config and reported:

```text
warning: unknown config key `compaction.thresholdd`
```

Validation errors stop startup and name the field. `version` must be `1`.

## Secrets

A provider names a key source, never a plaintext key:

```json
{
  "providers": {
    "openrouter": {
      "api_key_env": "OPENROUTER_API_KEY"
    }
  }
}
```

Resolution order is `api_key_env`, then `api_key_file`, then the environment
names the catalogue declares for that provider. Keys are held in Norte's redacted
secret type and are registered for output redaction with the command tool.

## The main sections

| Section | What it controls |
| --- | --- |
| `default_model` | The `provider/model` for a new session. |
| `providers` | Key source and endpoint override per provider. |
| `catalog` | models.dev URL, cache TTL and offline mode. |
| `compaction` | Threshold, trigger, recent messages and summary model. |
| `retry` | Bounded retries for transient provider failures. |
| `reasoning` | The default reasoning setting. |
| `system_prompt` | A replacement base prompt; `AGENTS.md` is appended after it. |
| `approval_mode` | `ask`, `auto-edit` or `yolo`. |
| `approvals` | Global allow/deny tool rules. |
| `max_iterations` | Model iterations per turn; `null` is unlimited. |
| `extensions` | Vendor-specific request fields. |
| `cost` | Whether to display reported cost. |
| `budgets` | Message, session and daily limits. |
| `tools` | `web_fetch`, the command output cap and the command sandbox. |
| `skills` | Discovery roots, scripts and allowed-tools behaviour. |
| `model_overrides` | Per-model context, output and reasoning overrides. |

The complete tables, defaults and validation ranges are in the
[configuration reference](../reference/configuration.md). The repository's
[`CONFIG.md`](https://github.com/JeanVydes/NorteCode/blob/dev/CONFIG.md) is the
long-form version.

## Project instructions

If the workspace root contains a regular `AGENTS.md`, the service appends it to
the base prompt. It is untrusted input: the read is capped at 64 KiB, only a
regular file is accepted, and the instructions do not override the user's
request, approvals or budgets.

`--system-prompt` overrides the base prompt for one run. `system_prompt` in the
config overrides it by default. `--debug` prints where the effective prompt came
from and how many characters the prompt and tool declarations contain.

### Runtime awareness

A new session stores a second system block next to the base prompt. It tells the
model the current `provider/model`, wire, catalogue dates and capabilities,
input/output modalities, context and output limits, compaction threshold and
`keep_recent`, workspace, sandbox and approval state, budgets, retries, skills
and the current config paths. It also includes a short command/config map so the
model can help with the harness itself.

The block is part of the session history, so the provider prompt cache sees a
stable prefix. A model switch appends a `Runtime update` block that supersedes
the old facts, and compaction writes a fresh block into the new session. It
contains no API keys or endpoint URLs.

## Extensions

Vendor-specific fields use Norte's extension mechanism:

```json
{
  "extensions": {
    "openai.chat:provider": {"order": ["deepinfra"]}
  }
}
```

The key is `<wire-family>:<dotted.path>`, and the value is JSON. A key whose wire
family does not match the active model is ignored. The same values can be passed
with `--extension 'openai.chat:provider.order=["deepinfra"]'`.

## Defaults that matter

- `approval_mode`: `ask`.
- `compaction.enabled`: `true`, threshold `0.90`, trigger `idle`, keep `6`.
- `retry.max_attempts`: `2`, backoff `1000 ms`, max delay `30000 ms`.
- `skills.enabled`: `true`, scripts and allowed-tools off.
- `tools.web_fetch`: `false`.
- `tools.sandbox.enabled`: `false`.
- `cost.show`: `true`.
- No budgets.
