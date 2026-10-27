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
| `default_model` | string or null | null | `provider/model` for new sessions. |
| `providers` | object | `{}` | Key source and endpoint per provider. |
| `catalog` | object | see below | models.dev source and cache. |
| `compaction` | object | see below | Automatic compaction. |
| `titling` | object | see below | Automatic session titles. |
| `retry` | object | see below | Transient retry policy. |
| `reasoning` | object | `{"default":"auto"}` | Default reasoning setting. |
| `system_prompt` | string or null | null | Base prompt replacement. |
| `approval_mode` | `ask`, `auto-edit`, `yolo` | `ask` | When tools ask. |
| `approvals` | object | empty lists | Global allow/deny rules. |
| `max_iterations` | integer or null | null | Model iterations per turn. |
| `extensions` | object | `{}` | Vendor request fields. |
| `cost` | object | `{"show":true}` | Cost display. |
| `budgets` | object | all null | Message, session, daily limits. |
| `tools` | object | see below | Web fetch and sandbox. |
| `skills` | object | see below | Skill discovery. |
| `model_overrides` | object | `{}` | Per-model overrides. |

## `providers.<id>`

| Key | Meaning |
| --- | --- |
| `api_key_env` | Environment variable with the key. |
| `api_key_file` | File containing the key. |
| `endpoint` | Base URL override. |
| `wire` | Wire to speak: `responses`, `chat`, `messages` or `generate-content`; overrides the catalogue's choice. |
| `auth` | Credential scheme: `bearer`, `x-api-key`, `x-goog-api-key` or `disabled`; overrides the wire's default. `disabled` sends no credential and needs no key. |
| `headers` | Extra headers sent with every request to this provider. |

Resolution order: `api_key_env`, `api_key_file`, then the catalogue's `env`.
There is no plaintext key field.

The catalogue's `npm` selects the wire and the key is sent the way that wire
expects. `wire` and `auth` are escape hatches for a compatible gateway: changing
the wire without `auth` keeps that wire's credential scheme.

The headers are added to the ones the provider already carries, and a header
named here replaces the one of the same name. `openrouter` gets the
attribution headers by default, so the app can appear in OpenRouter's
rankings: `HTTP-Referer` (the site's URL, which identifies the app),
`X-OpenRouter-Title` (the name it shows) and `X-OpenRouter-Categories` (the
marketplace category, `cli-agent` for a terminal coding assistant).

```json
{
  "version": 1,
  "providers": {
    "openrouter": {
      "headers": {
        "X-OpenRouter-Categories": "cli-agent,programming-app"
      }
    }
  }
}
```

## `catalog`

| Key | Default | Meaning |
| --- | --- | --- |
| `url` | `https://models.dev/catalog.json` | Catalogue source. |
| `ttl_hours` | `24` | Cache freshness. |
| `offline` | `false` | Never use the network. |

`--offline` and `--refresh` override this section for one command.

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

`ask` asks for `write`, `edit`, `command` and `skill_script`. `auto-edit` lets
edits run and still asks for commands and scripts. `yolo` runs everything.
`approvals.allow` and `approvals.deny` are tool-wide; denial wins. `/allow` and
`/deny` edit the same rules; `global` writes the user config.

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
| `command_max_model_bytes` | `131072` | Bytes of each `command` stream that reach the model; the rest is elided. Range `1024`–`1048576`. |
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

## `model_overrides.<provider/model>`

| Key | Meaning |
| --- | --- |
| `max_context` | Context window override. |
| `max_output` | Output limit override. |
| `reasoning.supported` | Whether the model reasons. |
| `reasoning.can_disable` | Whether reasoning may be disabled. |
| `reasoning.efforts` | Effort names to expose. |
| `reasoning.max_budget` | Maximum reasoning budget. |

## `extensions`

Keys are `<wire-family>:<dotted.path>`, values are JSON. A key whose wire family
does not match the active model is ignored.
