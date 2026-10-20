# Configuration

Norte Code reads one JSON object from two optional files:

1. `$XDG_CONFIG_HOME/norte-code/config.json`, or
   `~/.config/norte-code/config.json` on Linux/macOS and `%APPDATA%` on
   Windows. `--config PATH` replaces this path.
2. `.norte-code/config.json` in the workspace or the nearest parent directory
   that has one. The project overlay wins key by key over the user file.

Both files are merged shallowly: a key present in the project overwrites the
same key in the user file, while missing keys keep their defaults. Unknown keys
are kept out of the typed config and reported as `warning: unknown config key
\`path\``. Every field is optional unless stated otherwise.

`version` must be `1`. A value that fails validation stops startup with an error
that names the field; it is never silently replaced.

## Complete example

```json
{
  "version": 1,
  "default_model": "openrouter/anthropic/claude-sonnet-4",
  "providers": {
    "openrouter": {
      "api_key_env": "OPENROUTER_API_KEY",
      "api_key_file": null,
      "endpoint": "https://openrouter.ai/api/v1"
    }
  },
  "catalog": {
    "url": "https://models.dev/catalog.json",
    "ttl_hours": 24,
    "offline": false
  },
  "compaction": {
    "enabled": true,
    "threshold": 0.90,
    "trigger": "idle",
    "keep_recent": 6,
    "model": null,
    "reasoning": "lowest"
  },
  "retry": {
    "max_attempts": 2,
    "max_delay_ms": 30000,
    "backoff_ms": 1000
  },
  "reasoning": {
    "default": "auto"
  },
  "system_prompt": null,
  "approval_mode": "ask",
  "approvals": {
    "allow": [],
    "deny": []
  },
  "max_iterations": null,
  "extensions": {
    "openai.chat:provider": {
      "order": ["deepinfra"]
    }
  },
  "cost": {
    "show": true
  },
  "budgets": {
    "message": null,
    "session": {
      "unit": "usd",
      "limit": 2.0,
      "action": "ask"
    },
    "global_daily": null
  },
  "tools": {
    "web_fetch": false,
    "sandbox": {
      "enabled": false,
      "network": false,
      "writable": []
    }
  },
  "skills": {
    "enabled": true,
    "paths": [],
    "interop": true,
    "allow_scripts": false,
    "honor_allowed_tools": false,
    "max_skills": 128
  },
  "model_overrides": {
    "openrouter/anthropic/claude-sonnet-4": {
      "max_context": 200000,
      "max_output": 64000,
      "reasoning": {
        "supported": true,
        "can_disable": true,
        "efforts": ["low", "medium", "high"],
        "supports_budget": false,
        "max_budget": null
      }
    }
  }
}
```

## Top level

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `version` | integer | `1` | Schema revision; must be `1`. |
| `default_model` | string or `null` | `null` | `provider/model` for a new session without `--model`. |
| `providers` | object | `{}` | Per-provider keys and endpoint overrides. |
| `catalog` | object | see below | models.dev download and cache. |
| `compaction` | object | see below | Automatic context compaction. |
| `retry` | object | see below | Bounded retries for transient provider failures. |
| `reasoning` | object | `{"default":"auto"}` | Default reasoning setting. |
| `system_prompt` | string or `null` | `null` | Replaces the built-in base prompt. `AGENTS.md` is appended after it. |
| `approval_mode` | `"ask"`, `"auto-edit"` or `"yolo"` | `"ask"` | When tools need approval. |
| `approvals` | object | `{"allow":[],"deny":[]}` | Tool-wide rules that outlive a session. |
| `max_iterations` | integer or `null` | `null` | Model iterations per turn; `null` is unlimited. |
| `extensions` | object | `{}` | Vendor request fields keyed `<wire-family>:<dotted.path>`. |
| `cost` | object | `{"show":true}` | Whether to display reported cost. |
| `budgets` | object | all `null` | Optional message, session and daily limits. |
| `tools` | object | see below | Tool switches and the command sandbox. |
| `skills` | object | see below | Agent Skills discovery and execution. |
| `model_overrides` | object | `{}` | Per-model overrides keyed `provider/model`. |

## `providers.<id>`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `api_key_env` | string or `null` | `null` | Environment variable with the API key. Wins over the catalogue's `env` names. |
| `api_key_file` | path or `null` | `null` | File containing the API key. Used after `api_key_env`. |
| `endpoint` | string or `null` | `null` | Base URL override for a proxy or compatible vendor. |

Key resolution order for a provider: `api_key_env`, `api_key_file`, then the
`env` names the catalogue declares for that provider. Keys are held in Norte's
redacted secret type and never enter a model event, a log or a child process.
Prefer the environment or a file outside the repository; the config schema has
no plaintext `api_key` field.

## `catalog`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `url` | string | `https://models.dev/catalog.json` | Catalogue source. |
| `ttl_hours` | integer | `24` | How long the disk cache is fresh. |
| `offline` | boolean | `false` | Never touch the network; use the cache or the bundled snapshot. |

`--offline` and `--refresh` override this section for one command.

## `compaction`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | boolean | `true` | Whether automatic compaction may run. |
| `threshold` | number in `(0,1]` | `0.90` | Fraction of the model context that triggers a summary. |
| `trigger` | `"idle"` or `"eager"` | `"idle"` | `idle` compacts at a turn boundary; `eager` compacts at the first safe request boundary inside a turn. |
| `keep_recent` | integer | `6` | Recent messages copied verbatim into the new session. |
| `model` | string or `null` | `null` | Model used for the summary; `null` uses the session model. |
| `reasoning` | string | `"lowest"` | Reasoning effort for the summary request. |

Eager compaction never splits a tool call from its result, never runs while an
approval is pending, and runs at most once per turn. `/compact` triggers the
same path on demand. A failed summary leaves the old history and session id
unchanged.

## `retry`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `max_attempts` | integer `0..=10` | `2` | Retries after the first failure. `0` disables retries. |
| `max_delay_ms` | integer `0..=600000` | `30000` | Upper bound for one wait. |
| `backoff_ms` | integer `0..=600000` | `1000` | First fallback wait; doubles on each attempt. |

Only rate limits, provider overload, server errors and transport errors are
retried. Quota, authentication and malformed requests fail immediately. A retry
resumes the same session with no new input, so the user message is never
appended twice.

## `reasoning`

`default` is `"auto"`, `"off"`, an effort name such as `"low"`, or
`{"budget": 2048}`. `--reasoning` overrides it for one run or resume. The value
is checked against the selected model before the turn starts.

## `approval_mode` and `approvals`

| Value | Behaviour |
| --- | --- |
| `ask` | `write`, `edit`, `command` and `skill_script` ask first. |
| `auto-edit` | Edits run; `command` and `skill_script` still ask. |
| `yolo` | Every tool runs without asking. |

`approvals.allow` and `approvals.deny` contain tool names. A denied tool is
refused before the mode is consulted; an allowed tool runs without asking.
Denial wins when both lists name the same tool. These rules apply to every
session. In the interactive prompt, `/allow <tool> [session|global]` and
`/deny <tool> [session|global]` edit the same rules; `session` is the default
and `global` writes this file.

## `budgets`

`message`, `session` and `global_daily` each take `null` or an object:

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `unit` | `"usd"`, `"tokens"` or `"requests"` | `"usd"` | What the limit measures. |
| `limit` | number `> 0` | `0` (invalid) | The limit. |
| `action` | `"warn"`, `"ask"` or `"stop"` | `"warn"` | What happens when the limit is crossed. |

`warn` emits a warning and continues. `stop` ends the turn uncommitted and
leaves it resumable. `ask` currently behaves like a resumable stop with a
notice rather than opening a prompt: raise the budget and resume to continue.
Cost is only the amount the provider reports; Norte Code never derives a price
from the catalogue.

## `tools`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `web_fetch` | boolean | `false` | Register the `web_fetch` tool. |
| `sandbox.enabled` | boolean | `false` | Run `command` inside bubblewrap (Linux) or `sandbox-exec` (macOS). |
| `sandbox.network` | boolean | `false` | Allow network access inside the sandbox. |
| `sandbox.writable` | array of paths | `[]` | Extra writable directories besides the workspace. Relative paths resolve against the workspace; missing paths are ignored. |

On Linux the workspace and extras are writable, the rest of the filesystem is
read-only, `/tmp` is a private tmpfs and the process tree gets new namespaces.
On macOS the seatbelt profile keeps reads, denies writes outside the workspace,
the extras and the temporary roots, and denies the network unless enabled. A
missing platform binary fails closed; there is no unsandboxed fallback.
`--sandbox`, `--no-sandbox` and `--sandbox-network on|off` override this
section for one run or resume.

## `skills`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | boolean | `true` | Discover skills and register their tools. |
| `paths` | array of paths | `[]` | Extra discovery roots, after the user root. |
| `interop` | boolean | `true` | Also read `.agents/skills` interoperability roots. |
| `allow_scripts` | boolean | `false` | Register `skill_script`. Scripts are not covered by the command sandbox. |
| `honor_allowed_tools` | boolean | `false` | Let a skill's `allowed-tools` pre-approve calls. |
| `max_skills` | integer `>= 1` | `128` | Maximum number of valid skills kept. |

`/refresh` in the interactive prompt re-scans these roots; the next turn
registers the refreshed tools.

## `model_overrides.<provider/model>`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `max_context` | integer or `null` | `null` | Context window override. |
| `max_output` | integer or `null` | `null` | Output limit override. |
| `reasoning.supported` | boolean or `null` | `null` | Whether the model reasons. |
| `reasoning.can_disable` | boolean or `null` | `null` | Whether reasoning may be turned off. |
| `reasoning.efforts` | array of strings or `null` | `null` | Effort names to expose. |
| `reasoning.supports_budget` | boolean or `null` | `null` | Whether a token budget is accepted. |
| `reasoning.max_budget` | integer or `null` | `null` | Largest accepted budget. |

`null` means “keep the catalogue value”. An override only fills or replaces
what the catalogue declares; it does not invent capabilities the model lacks.

## `extensions`

Keys are `<wire-family>:<dotted.path>`, for example `openai.chat:provider` or
`anthropic.messages:metadata.user_id`. The value is JSON and is passed through
to the matching wire only. `--extension KEY=JSON` adds one for a single run.
`--debug` lists the configured keys, never their secret values.
