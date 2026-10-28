# Configuration

This is a compact reference. The full field-by-field version, including every
validation range, is the repository's
[`CONFIG.md`](https://github.com/JeanVydes/NorteCode/blob/dev/CONFIG.md).

## Files and precedence

1. User file: `$XDG_CONFIG_HOME/norte-code/config.json`,
   `~/.config/norte-code/config.json` or `%APPDATA%\norte-code\config.json`.
2. Project overlay: `.norte-code/config.json` in the workspace or nearest parent.

The project overlay wins key by key. `--config PATH` replaces the user path.
`version` must be `1`. Unknown keys are reported as warnings, not applied.

## Top level

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `version` | integer | `1` | Schema revision. |
| `default_provider` | string or null | null | Provider for new sessions. |
| `default_model` | string or null | null | Model id for new sessions; may contain slashes. |
| `catalog` | — | — | Removed. Providers and models are `providers.json` and `models.json` beside this file. |
| `compaction` | object | see below | Automatic compaction. |
| `titling` | object | see below | Automatic session titles. |
| `retry` | object | see below | Transient retry policy. |
| `reasoning` | object | `{"default":"auto"}` | Default reasoning setting. |
| `system_prompt` | string or null | null | Base prompt replacement. |
| `approval_mode` | `ask`, `auto-edit`, `yolo` | `ask` | When tools ask. |
| `approvals` | object | empty lists | Global allow/deny rules. |
| `max_iterations` | integer or null | null | Model iterations per turn. |
| `extensions` | object | `{}` | Vendor request fields. |
| `ui` | object | see below | What the client shows. |
| `cost` | object | `{"show":true}` | Cost display. |
| `budgets` | object | all null | Message, session, daily limits. |
| `tools` | object | see below | Web fetch and sandbox. |
| `skills` | object | see below | Skill discovery. |

## `providers` and `catalog`

Both are gone. A provider is an entry of `providers.json` and a model is an
entry of `models.json`, beside this file — see
[Configure a provider](../guides/configure-a-provider.md). The configuration
names only which pair a new session opens on:

| Key | Default | Meaning |
| --- | --- | --- |
| `default_provider` | null | The provider a new session opens on. |
| `default_model` | null | The model a new session opens on; may contain slashes. |

A key comes from the environment, named by the provider's `environment_keys` and
tried in order. There is no plaintext key field, no file to point at and no
override to write.

## `compaction`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Allow automatic compaction. |
| `threshold` | `0.90` | Context fraction in `(0, 1]`. |
| `trigger` | `idle` | `idle` or `eager`. |
| `keep_recent` | `6` | Recent messages kept verbatim. |
| `model` | null | Summary model; null uses the session model. |
| `reasoning` | `lowest` | Summary reasoning effort. |

## `titling`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Name a new session from its first message. |
| `model` | null | Model that writes the title; null uses the session model. |
| `max_output` | `24` | Tokens the title request may produce. |

A session with no title is listed by its key, and a key is not a name. When
titling is on, the first message a person sends is also sent to the model as a
request of its own — a scratch session holding one instruction, no tools, no
reasoning, the output capped by `max_output` — and the answer becomes the
session's title. Nothing of that request reaches the session being named, which
is the point: the naming is not something the model can see, answer to, or bill
as conversation.

The answer is a label that reaches a footer and a startup row, so it is
sanitised rather than trusted: control characters and bidi overrides are dropped,
only the first line survives, whitespace folds, and a reply that is not a name is
refused so the session keeps its key rather than taking a name nobody can use.

`titling.model` names a cheaper model to ask, resolved the way a model switch
resolves one, so a provider the catalogue does not list works. `enabled = false`
turns the feature off: it is one extra request per session, and a host that does
not want it must be able to say so. A session opened before titling existed, or
one whose request failed, gets its name from `/rename` instead.

## `retry`

| Key | Default | Meaning |
| --- | --- | --- |
| `max_attempts` | `4` | Retries after the first failure; `0` disables. |
| `max_delay_ms` | `60000` | Upper bound for one wait. |
| `backoff_ms` | `5000` | First fallback wait; doubles. |

Only rate limits, overload, server errors and transport errors are retried.

## `reasoning`

`default` is `auto`, `off`, an effort name (`minimal`, `low`, `medium`, `high`)
or `{"budget": N}`. It is checked against the selected model.

## `approval_mode` and `approvals`

`ask` asks for `write`, `edit`, `shell` and `skill_script`. `auto-edit` lets
edits run and still asks for commands and scripts. `yolo` runs everything.
`approvals.allow` and `approvals.deny` are tool-wide; denial wins. `/allow` and
`/deny` edit the same rules; `global` writes the user config.

## `ui`

What the client shows, as opposed to what the engine does. A host with no
interface ignores the block; the engine never reads it.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `verbosity` | string | `medium` | How much of the conversation to show: `low` (the answer alone), `medium` (also that tools run and that the model is thinking) or `high` (the reasoning, what each tool was asked and what it returned). |
| `diff_style` | `unified`, `split` | `unified` | How an `edit` call's diff is drawn. |

`--verbose` overrides `ui.verbosity` for one run. A config that says nothing
keeps `medium`, which is what the client shows with no file at all.

`diff_style` names two shapes of the same box, both carrying the same edit
number and line ranges:

- `unified` writes a patch: one column, `-` for a line that left and `+` for a
  line that arrived, with one row of context per unchanged line. It is the
  default because it fits a terminal that is not wide — the side-by-side view
  needs two panels of at least ten columns each and draws nothing below that.
- `split` shows the file before and after, side by side. Wider, and the only
  shape that shows a change inside a line rather than the two whole lines.

```json
{
  "ui": {
    "verbosity": "high",
    "diff_style": "split"
  }
}
```

## `budgets`

`message`, `session` and `global_daily` each accept null or:

| Key | Meaning |
| --- | --- |
| `unit` | `usd`, `tokens` or `requests`. |
| `limit` | A positive number. |
| `action` | `warn`, `ask` or `stop`. |

`ask` is currently a resumable stop with a notice. See
[Cost and budgets](../concepts/cost-and-budgets.md).

## `tools`

| Key | Default | Meaning |
| --- | --- | --- |
| `web_fetch` | `false` | Register `web_fetch`. |
| `command_max_model_bytes` | `131072` | Bytes of each `shell` stream that reach the model; the rest is elided. Range `1024`–`1048576`. |
| `sandbox.enabled` | `false` | Wrap commands in the OS sandbox. |
| `sandbox.network` | `false` | Allow network in the sandbox. |
| `sandbox.writable` | `[]` | Extra writable roots. |

## `skills`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Discover and register skills. |
| `paths` | `[]` | Extra roots after the user root. |
| `interop` | `true` | Read `.agents/skills` roots. |
| `allow_scripts` | `false` | Register `skill_script`; outside the command sandbox. |
| `honor_allowed_tools` | `false` | Let a skill pre-approve calls. |
| `max_skills` | `128` | Maximum valid skills. |

## `providers.<id>.models.<model-id>`

Removed. A model's limits, modalities and reasoning are facts about the model,
and they live in the entry that describes it — `models.json` — where they can be
edited without a second file having a say.

## `extensions`

Keys are `<wire-family>:<dotted.path>`, values are JSON. A key whose wire family
does not match the active model is ignored.
