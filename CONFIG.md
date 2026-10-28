# Configuration

Norte Code reads one JSON object from two optional files:

1. `$XDG_CONFIG_HOME/norte-code/config.json`, or
   `~/.config/norte-code/config.json` on Linux/macOS and `%APPDATA%` on
   Windows. `--config PATH` replaces this path.
2. `.norte-code/config.json` in the workspace or the nearest parent directory
   that has one. The project overlay wins key by key over the user file.

Both files are merged deeply: JSON objects are merged key by key, so a project
overlay can override one nested field without dropping its siblings. Arrays and
scalars replace the user value. Missing keys keep their defaults. Unknown keys
are kept out of the typed config and reported as `warning: unknown config key
\`path\``. Every field is optional unless stated otherwise.

`version` must be `1`. A value that fails validation stops startup with an error
that names the field; it is never silently replaced.

## Complete example

```json
{
  "version": 1,
  "default_provider": "openrouter",
  "default_model": "anthropic/claude-sonnet-4",
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
  "providers": {
    "openrouter": {
      "models": {
        "anthropic/claude-sonnet-4": {
          "max_context": 200000,
          "max_output": 64000,
          "modalities": {
            "input": ["text", "image"],
            "output": ["text"]
          },
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
  }
}
```

## Top level

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `version` | integer | `1` | Schema revision; must be `1`. |
| `default_provider` | string or `null` | `null` | Provider for a new session without `--provider`. |
| `default_model` | string or `null` | `null` | Model id for a new session without `--model`; an opaque string that may contain slashes. |
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


## `providers.<id>`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `api_key_env` | string or `null` | `null` | Environment variable with the API key. Wins over the catalogue's `env` names. |
| `api_key_file` | path or `null` | `null` | File containing the API key. Used after `api_key_env`. |
| `endpoint` | string or `null` | `null` | Base URL override for a proxy or compatible vendor. |
| `headers` | object | `{}` | Extra headers sent with every request to this provider. |
| `models` | object | `{}` | Per-model overrides, keyed by the model id alone. |

Key resolution order for a provider: `api_key_env`, `api_key_file`, then the
`env` names the catalogue declares for that provider. Keys are held in Norte's
redacted secret type and never enter a model event, a log or a child process.
Prefer the environment or a file outside the repository; the config schema has
no plaintext `api_key` field.

The headers are added to the ones the provider already carries, and a header
named in the configuration replaces the one of the same name. `openrouter`
gets the attribution headers by default, so the app can appear in OpenRouter's
rankings: `HTTP-Referer` (the site's URL, which identifies the app),
`X-OpenRouter-Title` (the name it shows) and `X-OpenRouter-Categories` (the
marketplace category; `cli-agent` is the one for a terminal coding
assistant).

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
| `reasoning` | string | `"lowest"` | Reasoning effort for the summary request: `lowest`, `off`, or an effort such as `low`, `xhigh` or `max`. |

Eager compaction never splits a tool call from its result, never runs while an
approval is pending, and runs at most once per turn. `/compact` triggers the
same path on demand. A failed summary leaves the old history and session id
unchanged.

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
| `command_max_model_bytes` | integer | `131072` | Bytes of each `command` stream and summary that reach the model. The tool still captures up to its own larger ceiling; this only bounds the text the model sees, eliding the middle with a marker. Valid range: `1024`–`1048576`. |
| `confinement.workspace_only` | boolean | `true` | Confine tool paths to the workspace. Off, `read`, `write`, `edit`, `glob`, `list_dir`, `grep` and `command`'s `cwd` may name any path, for a host working across repositories. A special file such as a FIFO is refused either way. |
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

`browser_use` answers a `screenshot` with the PNG as base64 inside its result,
and a tool result carries text only on the `chat` wire, so the image never
reached the model: it was told the size and nothing else. With
`screenshots.enabled` the host takes the capture out of the result, bounds it,
stores it, and attaches it to the conversation as an image the model can look
at — which works on every wire, because a *user* message carries an image on all
of them. The bound is applied before anything is sent, so a full-page capture is
downscaled rather than paid for.

A model that does not take image input is not sent the image at all: the
capture is dropped, with a warning, instead of being attached to a turn the
model would refuse. `screenshots.enabled` set to `false` restores the previous
behaviour, where the answer reports how large the capture was and the model
never sees it.

## `mcpServers`

Servers that speak the Model Context Protocol over their standard input and
output. The block keeps the name and the shape the rest of the ecosystem uses,
so a configuration written for another client works here unchanged.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `<name>.command` | string | — | The program to run. Required unless the entry is disabled. |
| `<name>.args` | array of strings | `[]` | Its arguments, in order. |
| `<name>.env` | object | `{}` | Environment variables for the child. A value named here is registered for redaction, so it never reaches a log or a model event. |
| `<name>.enabled` | boolean | `true` | Whether the server is started. An entry may be kept disabled while its command is filled in. |
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
        }
    }
}
```

A name in `allow` that the server does not publish is reported at startup rather
than ignored: the tool the host asked for would otherwise simply be absent.

Inside the interactive prompt, `/mcp` lists the servers with their tools and
whether they are on, and `/mcp enable <server>` / `/mcp disable <server>`
switches one. Switching a server off keeps its process running and takes its
tools out of the next turn, so `/mcp enable` brings them back at once.

Every tool a server publishes is registered as `mcp__<name>__<tool>`, so the
server keeps its own namespace and a per-tool approval rule still names one
tool. The name may hold letters, digits, `-` and `_`, and not the `__` the
namespace uses.

The child gets only the variables listed in its `env` and the `PATH`: the
ambient environment is cleared, because a server is a program from elsewhere and
that is where a key would leak from.

A server that cannot start is reported and skipped. One broken entry does not
stop the agent, which is what a person expects from a configuration they have
not tried yet; `--debug` prints the servers that connected and the ones that did
not, with the reason.

```json
{
    "mcpServers": {
        "filesystem": {
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/me/notes"],
            "env": { "TOKEN": "..." }
        },
        "everything": {
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-everything"]
        }
    }
}
```

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

## `providers.<id>.models.<model-id>`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `max_context` | integer or `null` | `null` | Context window override. |
| `max_output` | integer or `null` | `null` | Output limit override. |
| `reasoning.supported` | boolean or `null` | `null` | Whether the model reasons. |
| `reasoning.can_disable` | boolean or `null` | `null` | Whether reasoning may be turned off. |
| `reasoning.efforts` | array of strings or `null` | `null` | Effort names to expose, including `xhigh` and `max`. |
| `reasoning.supports_budget` | boolean or `null` | `null` | Whether a token budget is accepted. |
| `reasoning.max_budget` | integer or `null` | `null` | Largest accepted budget. |
| `modalities.input` | array of strings or `null` | `null` | Modalities the model takes in. |
| `modalities.output` | array of strings or `null` | `null` | Modalities the model produces. |

`null` means “keep the catalogue value”, and a value set here replaces what
the catalogue declares. No override invents a capability the model lacks —
what it changes is what Norte Code *believes* the model does, and a model only
takes an image if it really does.

Each modality is one of `text`, `image`, `audio`, `video` or `pdf` (`document`
is accepted as a synonym). A list **replaces** the declared set rather than
adding to it: it can give a modality to a model the catalogue does not describe
and take one away from a model it overstates. `input` and `output` are
independent — leaving one out keeps whatever the catalogue or the fallback said
for that side.

The model the catalogue does not list needs this. `river` and every other
private endpoint named only under `providers` has no catalogue entry, so its
capabilities fall back to text in, text out; without an `input` list naming
`image`, pasting a picture is refused:

```json
{
  "providers": {
    "river": {
      "models": {
        "deepseek-ai/DeepSeek-V4.1-Flash": {
          "modalities": { "input": ["text", "image"], "output": ["text"] }
        }
      }
    }
  }
}
```

The override sits under its provider and is keyed by the model id **alone**:
a model name may carry a slash of its own — `deepseek-ai/DeepSeek-V4.1-Flash`,
`anthropic/claude-sonnet-4` — and a key that had to be split on the first `/`
to be read would split those too.

## `extensions`

Keys are `<wire-family>:<dotted.path>`, for example `openai.chat:provider` or
`anthropic.messages:metadata.user_id`. The value is JSON and is passed through
to the matching wire only. `--extension KEY=JSON` adds one for a single run.
`--debug` lists the configured keys, never their secret values.
