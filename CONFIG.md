# Configuration

Norte Code reads two kinds of file: its own settings, and the catalogue of
providers and models.

**Settings** are `config.json`, merged from two optional files:

1. `$XDG_CONFIG_HOME/norte-code/config.json`, else
   `~/.config/norte-code/config.json`, else `%APPDATA%\norte-code\config.json`.
   `--config PATH` replaces this path.
2. `.norte-code/config.json` in the workspace or the nearest parent directory
   that has one. The project overlay wins key by key over the user file.

**The catalogue** is two files beside the user configuration, not inside it:
`providers.json` and `models.json`. They are written empty when missing, never
fetched, and never overwritten once they exist.

Both settings files are merged deeply: JSON objects are merged key by key, so a
project overlay can override one nested field without dropping its siblings.
Arrays and scalars replace the user value. Missing keys keep their defaults.
Unknown keys are kept out of the typed config and reported as
``warning: unknown config key `path` ``. Every field is optional unless stated
otherwise.

`version` must be `1`. A value that fails validation stops startup with an error
that names the field; it is never silently replaced. The catalogue files are
validated the same way, and a missing or malformed one names the file to fill in
or fix.

## Complete example

```json
{
  "version": 1,
  "default_provider": "openrouter",
  "default_model": "anthropic/claude-sonnet-4",
  "compaction": {
    "enabled": true,
    "threshold": 0.90,
    "trigger": "idle",
    "keep_recent": 6,
    "provider": null,
    "model": null,
    "reasoning": "lowest"
  },
  "titling": {
    "enabled": true,
    "provider": null,
    "model": null,
    "max_output": 24
  },
  "retry": {
    "max_attempts": 4,
    "max_delay_ms": 60000,
    "backoff_ms": 5000
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
  "ui": {
    "verbosity": "medium",
    "diff_style": "unified"
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
    "command_max_model_bytes": 131072,
    "disabled": [],
    "confinement": {
      "workspace_only": true,
      "sandbox": {
        "enabled": false,
        "network": false,
        "writable": []
      }
    },
    "screenshots": {
      "enabled": true,
      "max_dimension": 1568,
      "max_bytes": 4000000
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
  "mcpServers": {}
}
```

## Top level

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `version` | integer | `1` | Schema revision; must be `1`. |
| `default_provider` | string or `null` | `null` | Provider for a new session without `--provider`. |
| `default_model` | string or `null` | `null` | Model id for a new session without `--model`; an opaque string that may contain slashes. The provider is its own key and never joined with it. Set with `default_provider` or not at all. |
| `compaction` | object | see below | Automatic context compaction. |
| `titling` | object | see below | Automatic session titles. |
| `retry` | object | see below | Bounded retries for transient provider failures. |
| `reasoning` | object | see below | Default reasoning setting. |
| `system_prompt` | string or `null` | `null` | Replaces the built-in base prompt. `AGENTS.md` is appended after it. |
| `approval_mode` | `"ask"`, `"auto-edit"` or `"yolo"` | `"ask"` | The default approval mode. |
| `approvals` | object | `{"allow":[],"deny":[]}` | Tool-wide rules that outlive a session. |
| `max_iterations` | integer or `null` | `null` | Model iterations per turn; `null` is unlimited. |
| `extensions` | object | `{}` | Vendor request fields keyed `<wire-family>:<dotted.path>`. |
| `cost` | object | `{"show":true}` | Whether to display reported cost. |
| `ui` | object | see below | Verbosity and diff style. |
| `budgets` | object | all `null` | Optional message, session and daily limits. |
| `tools` | object | see below | Tool switches, confinement and screenshots. |
| `skills` | object | see below | Agent Skills discovery and execution. |
| `mcpServers` | object | `{}` | MCP servers whose tools the agent may call. |

`providers` and `catalog` are not configuration keys: providers and models live
in `providers.json` and `models.json`, and the configuration only names the
default provider/model pair. The configuration never overrides a model's
capabilities — the catalogue is the only authority on what a model accepts.

## `providers.json`

The providers a session may use. Each entry is a provider id and what is needed
to reach it:

```json
{
  "providers": {
    "openrouter": {
      "name": "OpenRouter",
      "doc": "https://openrouter.ai",
      "environment_keys": ["OPENROUTER_API_KEY"],
      "wires": [
        { "api": "https://openrouter.ai/api/v1", "schema": "openai" }
      ]
    }
  }
}
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | `""` | The display name. |
| `doc` | string or `null` | `null` | A documentation URL. |
| `environment_keys` | array of strings | `[]` | Environment variables that may hold the key, tried in order. |
| `wires` | array of wires | `[]` | The API surfaces this provider serves. |

A wire:

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `api` | string | — | The full base URL requests go to. |
| `schema` | `"openai"`, `"anthropic"` or `"google"` | — | The request/answer shape the surface speaks. |
| `auth` | `"bearer"`, `"x_api_key"`, `"x_goog_api_key"` or `"disabled"` | schema's own | The credential header, when it is not the one the schema implies. |

`openai` is the chat completions surface every compatible vendor speaks;
`anthropic` is Messages; `google` is `generateContent`. The credential header
follows from the schema unless a wire names one — `bearer` is
`Authorization: Bearer`, `x_api_key` is `x-api-key`, `x_goog_api_key` is
`x-goog-api-key`, and `disabled` sends none. There is no key in the file: a key
comes from the environment, named by `environment_keys` and tried in order, is
held in Norte's redacted secret type, and never enters a model event, a log or a
child process.

When a provider serves several wires, `--wire` picks one; otherwise the order is
`openai`, then `anthropic`, then `google`.

## `models.json`

Every model, with the provider that serves it, as a flat list. A model is
identified by the pair `(id, provider_id)`, so two providers may serve the same
id without either shadowing the other, and an id may carry slashes of its own.

```json
{
  "models": [
    {
      "id": "anthropic/claude-sonnet-4",
      "provider_id": "openrouter",
      "canonical_slug": "openrouter/anthropic/claude-sonnet-4",
      "name": "Claude Sonnet 4",
      "modalities": { "input": ["text", "image"], "output": ["text"] },
      "limit": { "context": 200000, "output": 64000 },
      "supported_params": ["tools", "temperature", "top_p"],
      "reasoning": { "default_enabled": true, "supported_efforts": ["low", "medium", "high"] }
    }
  ]
}
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | — | The id the provider expects in a request. |
| `provider_id` | string | — | The `providers.json` entry that serves it. |
| `canonical_slug` | string | `""` | A stable label such as `openrouter/anthropic/claude-sonnet-4`; what the UI shows. |
| `name` | string | `""` | The display name. |
| `description` | string or `null` | `null` | A one-line description. |
| `family` | string or `null` | `null` | The model family, such as `claude`. |
| `knowledge` | string or `null` | `null` | The knowledge cutoff. |
| `release_date` | string or `null` | `null` | The release date. |
| `last_updated` | string or `null` | `null` | The last update date. |
| `open_weights` | boolean | `false` | Whether the weights are open. |
| `modalities.input` | array of strings | `[]` | Accepted inputs: `text`, `image`, `audio`, `video`, `file` (`pdf` is a synonym). |
| `modalities.output` | array of strings | `[]` | Produced outputs, same names. |
| `limit.context` | integer or `null` | `null` | The context window, in tokens. |
| `limit.output` | integer or `null` | `null` | The largest output, in tokens. |
| `status` | `"deprecated"`, `"beta"` or absent | absent | Where the model is in its life. |
| `supported_params` | array of strings | `[]` | The request fields the model accepts. |
| `pricing.input` | number or `null` | `null` | USD per input token, for reference only. |
| `pricing.output` | number or `null` | `null` | USD per output token, for reference only. |
| `pricing.cached` | number or `null` | `null` | USD per cached-input token, for reference only. |
| `reasoning.mandatory` | boolean | `false` | The model always reasons; it cannot be asked not to. |
| `reasoning.default_enabled` | boolean | `false` | Reasoning is on unless a request turns it off. |
| `reasoning.default_effort` | string or `null` | `null` | The effort used when a request names none. |
| `reasoning.supported_efforts` | array of strings | `[]` | The efforts accepted: `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`. |
| `reasoning.budget_supported` | boolean | `false` | Whether a token budget is accepted. `supports_budget` is an alias. |

`supported_params` is a closed set, and a name it does not know is a read error
rather than a silent "not supported": `frequency_penalty`, `include_reasoning`,
`logit_bias`, `logprobs`, `max_tokens`, `min_p`, `presence_penalty`, `reasoning`,
`reasoning_effort`, `repetition_penalty`, `response_format`, `seed`, `stop`,
`structured_output`, `temperature`, `tool_choice`, `tools`, `top_a`, `top_k`,
`top_logprobs`, `top_p`, `verbosity`.

Pricing is kept for reference and is never a source of cost: Norte Code reports
what the provider said a request cost and derives nothing from a list price.

## `compaction`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | boolean | `true` | Whether automatic compaction may run. |
| `threshold` | number in `(0,1]` | `0.90` | Fraction of the model context that triggers a summary. |
| `trigger` | `"idle"` or `"eager"` | `"idle"` | `idle` compacts at a turn boundary; `eager` compacts at the first safe request boundary inside a turn. |
| `keep_recent` | integer | `6` | Recent messages copied verbatim into the new session. |
| `provider` | string or `null` | `null` | Provider of the summary model; set with `model` or not at all. |
| `model` | string or `null` | `null` | Model id used for the summary; `null` uses the session model. |
| `reasoning` | string | `"lowest"` | Reasoning effort for the summary request: `off`, `lowest`, or an effort such as `low`, `high`, `xhigh` or `max`. |

Eager compaction never splits a tool call from its result, and runs at most once
per turn. `/compact` triggers the same path on demand. A failed summary leaves
the old history and session id unchanged.

`provider` and `model` are two keys and never one `provider/model` string: a
model id may carry slashes of its own, so the two are set apart. Both or neither:
one without the other cannot be resolved.

## `titling`

Settings for the model-written title. They do **not** control the automatic
title: a new session's title is the first 32 characters of its first message,
truncated once and never asked again. These fields are read when a session is
renamed without a title (a bare `/rename`), which asks the model to name it from
the first message the session holds.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | boolean | `true` | Allow the model-written title; `false` makes a bare `/rename` fail. |
| `provider` | string or `null` | `null` | Provider for the title request; the session's own when `null`. Set with `model` or not at all. |
| `model` | string or `null` | `null` | Model id for the title request; the session's model when `null`. |
| `max_output` | integer | `24` | Tokens the title request may produce. |

## `retry`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `max_attempts` | integer `0..=10` | `4` | Retries after the first failure. `0` disables retries. |
| `max_delay_ms` | integer `0..=600000` | `60000` | Upper bound for one wait. |
| `backoff_ms` | integer `0..=600000` | `5000` | First fallback wait; doubles on each attempt. |

Only rate limits, provider overload, server errors and transport errors are
retried. Quota, authentication and malformed requests fail immediately. A retry
resumes the same session with no new input, so the user message is never
appended twice.

## `reasoning`

`default` is `"auto"`, `"off"`, an effort name such as `"low"`, `"xhigh"` or
`"max"`, or `{"budget": 2048}`. `--reasoning` overrides it for one run or
resume. The value is checked against the selected model before the turn starts,
so `xhigh` and `max` are accepted only where the model declares them.

## `approval_mode`

How a tool call is decided when no `/allow`, `/deny` or session rule names it.
`ask` (the default) asks before every write and command; `auto-edit` allows
edits without asking and still asks for commands; `yolo` allows everything.
`--approval-mode` overrides it for one run.

## `approvals`

`approvals.allow` and `approvals.deny` contain tool names. A denied tool is
refused before it runs; an allowed tool runs without asking. Denial wins when
both lists name the same tool. These rules apply to every session. In the
interactive prompt, `/allow <tool> [session|global]` and `/deny <tool>
[session|global]` edit the same rules; `session` is the default and `global`
writes this file.

A rule outlives the turn it was written in. A session rule is kept with the
session and survives a restart; a global rule lives in this file. The engine
consults the rule immediately before a tool call runs: nothing pauses for a
person, and a denied call reaches the model as a result it reads like a failure
and can adapt to.

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

## `ui`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `verbosity` | `"low"`, `"medium"` or `"high"` | `"medium"` | `low` shows the answer alone, `medium` also shows that tools run and that the model is thinking, `high` adds reasoning, tool arguments and results. `--verbose` overrides it for one run. |
| `diff_style` | `"unified"` or `"split"` | `"unified"` | How an `edit` call is drawn: a patch (`-`/`+`) or two columns before/after. |

## `tools`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `web_fetch` | boolean | `false` | Register the `web_fetch` tool. |
| `disabled` | array of strings | `[]` | Tools a new session does not get, by the name the model calls them. A tool named here is not registered at all: no schema, no request, no entry in the registry. Read when a session is created and kept for its life, because the declarations are part of the prompt prefix; editing the file changes what the next session gets. |
| `command_max_model_bytes` | integer `1024..=1048576` | `131072` | Bytes of each `command` stream and summary that reach the model. The tool still captures up to its own larger ceiling; this bounds the text the model sees, eliding the middle with a marker. |
| `confinement.workspace_only` | boolean | `true` | Confine tool paths to the workspace. Off, `read`, `write`, `edit`, `glob`, `list_dir`, `grep` and `command`'s `cwd` may name any path. A special file such as a FIFO is refused either way. |
| `confinement.sandbox.enabled` | boolean | `false` | Run `command` inside bubblewrap (Linux) or `sandbox-exec` (macOS). |
| `confinement.sandbox.network` | boolean | `false` | Allow network access inside the sandbox. |
| `confinement.sandbox.writable` | array of paths | `[]` | Extra writable directories besides the workspace. Relative paths resolve against the workspace; missing paths are ignored. |
| `screenshots.enabled` | boolean | `true` | Attach a `browser_use` screenshot to the conversation so the model can look at it. |
| `screenshots.max_dimension` | integer | `1568` | Longest side, in pixels, a screenshot keeps. A larger one is downscaled. |
| `screenshots.max_bytes` | integer | `4000000` | The largest a screenshot may be after re-encoding, in bytes. |

On Linux the workspace and extras are writable, the rest of the filesystem is
read-only, `/tmp` is a private tmpfs and the process tree gets new namespaces.
On macOS the seatbelt profile keeps reads, denies writes outside the workspace,
the extras and the temporary roots, and denies the network unless enabled. A
missing platform binary fails closed; there is no unsandboxed fallback.
`--sandbox`, `--no-sandbox` and `--sandbox-network on|off` override the sandbox
for one run or resume. The two halves of `confinement` are separate: the
workspace gate is a check on a tool's arguments and the sandbox is an operating
system isolation of one child process.

`screenshots` promotes a `browser_use` capture out of its text result into an
image the model can see on every wire. A model that does not take image input is
not sent it: the capture is dropped with a warning. Setting `enabled` to `false`
leaves the tool's answer as it was, with the size and no image.

## `mcpServers`

Servers that speak the Model Context Protocol. An entry is either a local
program over its standard input and output, or a remote Streamable HTTP endpoint.
The block keeps the shape the rest of the ecosystem uses, so a configuration
written for another client works here unchanged.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `<name>.command` | string | `""` | The program to run. Required for a local server. |
| `<name>.url` | string or `null` | `null` | The endpoint, for a remote server. Exactly one of `command` and `url` is set. |
| `<name>.args` | array of strings | `[]` | The program's arguments, in order. |
| `<name>.env` | object | `{}` | Environment variables for the child. A value named here is registered for redaction. |
| `<name>.headers` | object | `{}` | Headers every request to a remote server carries. |
| `<name>.enabled` | boolean | `true` | Whether the server is started. A disabled entry may be a placeholder. |
| `<name>.allow` | array of strings | `[]` | The tools of this server that may reach the model. Empty means every one it publishes. |
| `<name>.deny` | array of strings | `[]` | The tools of this server that never reach the model. `deny` wins over `allow`. |

Both lists are written as the **server** names its tools, without the
`mcp__<name>__` prefix the model sees. This is the control that matters in
practice: every tool rides in each request, so a server with sixteen tools costs
about 25k characters of schema on every turn, whether the model uses them or
not.

```json
{
  "mcpServers": {
    "everything": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-everything"],
      "deny": ["gzip-file-as-resource", "get-tiny-image"],
      "allow": ["echo", "get-sum", "get-env"]
    },
    "remote": {
      "url": "https://mcp.example.com/mcp",
      "headers": { "Authorization": "Bearer ..." }
    }
  }
}
```

A name in `allow` that the server does not publish is reported at startup rather
than ignored: the tool the host asked for would otherwise simply be absent. A
server name may hold letters, digits, `-` and `_`, and not the `__` the namespace
uses.

Inside the interactive prompt, `/mcp` lists the servers with their tools and
whether they are on, and `/mcp enable <server>` / `/mcp disable <server>`
switches one. Switching a server off keeps its process running and takes its
tools out of the next turn.

Every tool a server publishes is registered as `mcp__<name>__<tool>`, so the
server keeps its own namespace and a per-tool approval rule still names one tool.

A local child gets only the variables listed in its `env` and the `PATH`: the
ambient environment is cleared. A server that cannot start is reported and
skipped; `--debug` prints the servers that connected and the ones that did not,
with the reason.

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

## `extensions`

Keys are `<wire-family>:<dotted.path>`, for example `openai.chat:provider` or
`anthropic.messages:metadata.user_id`. The value is JSON and is passed through
to the matching wire only; a dotted path is applied inside the body, and a path
that collides with a field the adapter set is a typed error. `--extension
KEY=JSON` adds one for a single run. `--debug` lists the configured keys, never
their secret values.

Sampling controls the neutral request carries (`temperature`, `top_p`, `top_k`,
`min_p`, `top_a`, `frequency_penalty`, `presence_penalty`, `repetition_penalty`,
`logit_bias`, `seed`, `stop`, `max_tokens`, `logprobs`) are set by the host, not
by this file; use an extension for any other vendor field.
