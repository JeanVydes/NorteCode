# Configuration

Norte Code reads two kinds of file: its own settings, and the catalogue of
providers and models.

**Server settings** are `service.json`, merged from two optional files:

1. `$XDG_CONFIG_HOME/norte-code/service.json`, else
   `~/.config/norte-code/service.json`, else `%APPDATA%\norte-code\service.json`.
2. `.norte-code/service.json` in the workspace or the nearest parent directory
   that has one. The project overlay wins key by key over the user file.

**Client settings** are `cli.json`, read only by `norte-code`:

1. `$XDG_CONFIG_HOME/norte-code/cli.json`, else
   `~/.config/norte-code/cli.json`, else `%APPDATA%\norte-code\cli.json`.
2. `.norte-code/cli.json` in the workspace or nearest parent directory.

The client never reads `service.json`, `providers.json` or `models.json`; the
server owns execution, configuration and catalogue discovery.

**The catalogue** is two files beside the server configuration, not inside it:
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
  "default_agent": "norte-code",
  "agents": [{
    "name": "norte-code",
    "description": "Default assistant",
    "origin_url": null,
    "author": null,
    "default_provider": "openrouter",
    "default_model": "anthropic/claude-sonnet-4",
    "compaction": {
      "enabled": true,
      "threshold": 0.90,
      "trigger": "idle",
      "keep_recent": 6
    },
    "retry": {
      "max_attempts": 4,
      "max_delay_ms": 60000,
      "backoff_ms": 5000
    },
    "provider_http": {
      "connect_max_retries": 3,
      "connect_timeout_ms": 10000,
      "request_timeout_ms": 120000,
      "stream_idle_timeout_ms": 300000
    },
    "delegation": {
      "max_depth": 3,
      "max_concurrent": 3,
      "max_held_messages": 20
    },
    "mcp_limits": {
      "request_timeout_ms": 60000
    },
    "reasoning": { "default": "auto" },

    "max_iterations": null,
    "extensions": {},
    "cost": { "show": true },
    "budgets": {},
    "tools": {
      "ask_user_timeout_ms": 600000,
      "verbose_tool_output_chars": 4000,
      "read_limits": {
        "max_file_bytes": 16777216,
        "max_output_bytes": 65536,
        "max_lines": 2000,
        "max_matches": 200,
        "max_line_width": 500
      }
    },
    "skills": {
      "limits": {
        "max_skill_file_bytes": 1048576,
        "max_body_bytes": 65536,
        "default_resource_bytes": 262144,
        "recommended_lines": 500,
        "max_shebang_bytes": 4096,
        "max_script_output_bytes": 1048576,
        "max_script_model_bytes": 32768,
        "default_script_timeout_ms": 120000,
        "max_script_timeout_ms": 600000
      }
    },
    "mcpServers": {}
  }],
  "raep_limits": {
    "replay_capacity": 2000,
    "replay_bytes": 8388608,
    "max_concurrent_jobs": 8,
    "job_output_retention_s": 86400,
    "idempotency_capacity": 4096,
    "max_queued_inputs": 8,
    "max_frame_bytes": 1048576,
    "max_artifact_bytes": 16777216,
    "max_export_bytes": 67108864,
    "export_ttl_s": 3600,
    "max_connections": 64,
    "command_timeout_ms": 120000
  }
}
```

## Service envelope and agents

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `version` | integer | `1` | Schema revision; must be `1`. |
| `default_agent` | string | `"norte-code"` | Agent selected for new attachments. |
| `agents` | array | one default agent | Named server-owned agent definitions. |
| `compaction` | object | see below | Automatic context compaction. |
| `retry` | object | see below | Bounded retries for transient provider failures. |
| `provider_http` | object per agent | see below | Transport policy for provider requests. |
| `delegation` | object per agent | see below | Bounds for child sessions. |
| `mcp_limits` | object per agent | see below | Shared MCP request timeout. |
| `raep_limits` | object | see below | Service host capacity and retention limits. |
| `reasoning` | object | see below | Default reasoning setting. |
| `system_prompt` | string or `null` | `null` | Replaces the built-in base prompt. `AGENTS.md` is appended after it. |
| `approvals` | object | `{"allow":[],"deny":[]}` | Tool-wide rules that outlive a session. |
| `max_iterations` | integer or `null` | `null` | Model iterations per turn; `null` is unlimited. |
| `extensions` | object | `{}` | Vendor request fields keyed `<wire-family>:<dotted.path>`. |
| `cost` | object | `{"show":true}` | Whether to display reported cost. |
| `budgets` | object | all `null` | Optional message, session and daily limits. |
| `tools` | object | see below | Tool switches and confinement. |
| `skills` | object | see below | Agent Skills discovery and execution. |
| `mcpServers` | object | `{}` | MCP servers whose tools the agent may call. |

Every object in `agents` has a required unique `name`; `description`,
`origin_url` and `author` are optional metadata. The remaining fields in an
agent are the operational settings shown below. Providers and models live in
`providers.json` and `models.json`; an agent only names its default pair. The
configuration never overrides a model's capabilities — the catalogue is the
only authority on what a model accepts. The RAEP server exposes every named
agent; `cli.json.agent_id` selects which one the CLI attaches to.

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
`x-goog-api-key`, and `disabled` sends none. A key is not written in this file.
It comes from the environment first, named by `environment_keys` and tried in
order; if none is set, the client reads `${XDG_CONFIG_HOME}/norte-code/secrets.json`,
which is written with mode 0600. Selecting a model from `/model` or `/models`
when the key is still missing opens a masked prompt and saves what you type to
`secrets.json`. Keys are held in the client's redacted `ApiKey` type and never
enter a model event, a log or a child process.

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

Eager compaction never splits a tool call from its result, and runs at most once
per turn. `/compact` triggers the same path on demand. A failed summary leaves
the old history and session id unchanged.

Compaction always uses the session's provider, model and effective reasoning.

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

`retry` controls what happens after a provider call fails. `provider_http`
controls the transport underneath each call. They are separate policies and
are not added together.

## `provider_http`

This block is per agent and applies to every provider adapter used by it.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `connect_max_retries` | integer `0..=10` | `3` | Attempts made while establishing a connection. |
| `connect_timeout_ms` | integer `1..=60000` | `10000` | Deadline for one connection attempt. |
| `request_timeout_ms` | integer `1..=3600000` | `120000` | Deadline for the whole request. Must not be shorter than `connect_timeout_ms`. |
| `stream_idle_timeout_ms` | integer `1..=3600000` | `300000` | Time a stream may send no bytes before it is treated as stuck. |

## `delegation`

This block is per agent and bounds the child sessions that agent can create.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `max_depth` | integer `1..=8` | `3` | Deepest chain of child sessions. |
| `max_concurrent` | integer `1..=64` | `3` | Child sessions that may run at once. |
| `max_held_messages` | integer `1..=1000` | `20` | Delegate queue size: parent messages held for a running child and replayed when it finishes. |

The agent's `delegates` list still decides which profiles may be spawned. These
limits only bound how many children run and how deep they go.

## `mcp_limits`

This block is per agent and is shared by every server in `mcpServers`.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `request_timeout_ms` | integer `1000..=600000` | `60000` | Deadline for a handshake, tool listing or tool call. |

## `raep_limits`

This block is service-wide, outside `agents`.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `replay_capacity` | integer `> 0` | `2000` | Frames kept for replay. |
| `replay_bytes` | integer `> 0` | `8388608` | Bytes retained across replay frames. |
| `max_concurrent_jobs` | integer `> 0` | `8` | Jobs that may run at once. |
| `job_output_retention_s` | integer `> 0` | `86400` | Seconds finished job output is kept. |
| `idempotency_capacity` | integer `> 0` | `4096` | Turn request fingerprints kept. |
| `max_queued_inputs` | integer `> 0` | `8` | Inputs waiting behind a running turn. |
| `max_frame_bytes` | integer `> 0` | `1048576` | Largest accepted frame. |
| `max_artifact_bytes` | integer `> 0` | `16777216` | Largest accepted uploaded artifact. |
| `max_export_bytes` | integer `> 0` | `67108864` | Largest exported bundle. |
| `export_ttl_s` | integer `> 0` | `3600` | Seconds a generated bundle can be downloaded. |
| `max_connections` | integer `> 0` | `64` | Concurrent WebSocket and SSE clients. |
| `command_timeout_ms` | integer `1..=600000` | `120000` | Deadline for handling one command. |

`NORTE_CODE_REPLAY_CAPACITY` and `NORTE_CODE_MAX_CONNECTIONS` still override the
matching fields when they are set to positive values.

## `reasoning`

`default` is `"auto"`, `"off"`, an effort name such as `"low"`, `"xhigh"` or
`"max"`, or `{"budget": 2048}`. `--reasoning` overrides it for one run or
resume. The value is checked against the selected model before the turn starts,
so `xhigh` and `max` are accepted only where the model declares them.

## `approvals`

`approvals.allow` and `approvals.deny` contain tool names. A denied tool is
refused before it runs; an allowed tool runs. Denial wins when both lists name
the same tool. These rules apply to every session. In the interactive prompt,
`/allow <tool> [session|global]` and `/deny <tool> [session|global]` edit the
same rules; `session` is the default and `global` writes this file.

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

## Remote service

Set `service_type` to `remote`, `service_url` to the service base URL, and
`service_auth` to the raw bearer token when authentication is enabled. The
service runs in its own working directory: its workspace, project instructions,
skills, tools, providers, and session database belong to that machine. A
client's current directory is not sent as a workspace during session attach.

The server listens on `127.0.0.1:8787` by default. `NORTE_CODE_BIND` changes
the listening address; `NORTE_CODE_AUTH_TOKEN` configures a bearer token.
For network access use TLS through a reverse proxy or an encrypted tunnel.
The service itself serves HTTP. Protect the server's environment and files,
since its tools execute with the server process's permissions.

The TUI reconnects its event stream and asks for events after its last
contiguous sequence. If the replay journal no longer holds them, the client
warns and reloads the saved history after an active turn finishes. Output that
was never persisted cannot be reconstructed. Session creation retries reuse an
idempotency key while the server process remains running; its key cache is not
persisted across a server restart.

## `cli.json`

The CLI-only file contains:

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `service_url` | string | `"http://127.0.0.1:8787"` | RAEP service endpoint. |
| `norte_code_skill` | boolean | `true` | Install the CLI-managed Norte Code skill at startup; `false` removes the managed skill. |
| `service_type` | `"local"` or `"remote"` | `"local"` | Local enables lifecycle commands; remote rejects start/stop/restart. |
| `service_auth` | string or `null` | `null` | Raw bearer token. Leave `null` for an unauthenticated local service. |
| `agent_id` | string | `"norte-code"` | Agent identifier sent during RAEP attachment. |
| `verbosity` | `"low"`, `"medium"` or `"high"` | `"medium"` | `low` shows the answer alone, `medium` also shows that tools run and that the model is thinking, `high` adds reasoning, tool arguments and results. `--verbose` overrides it for one run. |
| `queue_mode` | `"reject"`, `"queue"` or `"interrupt"` | `"queue"` | What a message typed while a turn runs does. `queue` steers it into the running turn, `interrupt` cancels that turn first. |
| `diff_style` | `"unified"` or `"split"` | `"unified"` | How an `edit` call is drawn: a patch (`-`/`+`) or two columns before/after. |
| `charts.contrast` | integer `0..=100` | `75` | Strength of the halftone dot ramp. |
| `charts.density` | integer `0..=100` | `82` | Coverage of candidate dots in halftone charts. |
| `charts.dot_size` | `"small"`, `"medium"` or `"large"` | `"medium"` | Character scale used by line, pie and radar charts. |
| `service_lifecycle` | object | see below | Tokio workers and local service process timings. |
| `reconnect` | object | see below | Client reconnect backoff. |
| `retention` | object | see below | Client transcript and console retention. |

The three runtime blocks are client-owned. They never change a model request.
Liveness and readiness probe timeouts stay fixed and are not configurable.

| `service_lifecycle` key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `tokio_workers` | integer `1..=1024` | `4` | Worker threads for a service started by this CLI. `NORTE_CODE_TOKIO_WORKERS` wins when set. The CLI clamps the result to available CPUs. |
| `startup_wait_ms` | integer `> 0` | `20000` | Time to wait for the local service to report ready. |
| `startup_poll_ms` | integer `> 0` | `150` | Interval between readiness checks. Must not exceed `startup_wait_ms`. |
| `stop_wait_ms` | integer `> 0` | `10000` | Time to wait for a graceful stop. |
| `stop_poll_ms` | integer `> 0` | `100` | Interval between process checks while stopping. Must not exceed `stop_wait_ms`. |
| `kill_settle_ms` | integer `0..=10000` | `200` | Grace after sending a kill signal. |

| `reconnect` key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `retry_base_ms` | integer `1..=60000` | `400` | Delay before the first reconnect. |
| `retry_max_ms` | integer `1..=3600000` | `5000` | Largest delay between reconnects. Must be at least `retry_base_ms`. |
| `give_up_attempts` | integer `1..=20` | `3` | Failed attempts before the client reports giving up. |
| `healthy_stream_s` | integer `1..=3600` | `10` | Time a healthy stream must stay open before resets are cleared. |

| `retention` key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `transcript_max_items` | integer `> 0` | `1000` | Largest number of transcript rows held in memory. |
| `transcript_keep_items` | integer `> 0` | `800` | Rows kept after a trim. Must not exceed `transcript_max_items`. |
| `console_max_lines` | integer `> 0` | `500` | Largest number of console lines held in memory. |
| `console_keep_lines` | integer `> 0` | `400` | Lines kept after a trim. Must not exceed `console_max_lines`. |

The attachment size stays at 16 MiB and has no setting. Question and review
prompt text has no character limit setting.

## `tools`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `web_fetch` | boolean | `false` | Register the `web_fetch` tool. |
| `disabled` | array of strings | `[]` | Tools a new session does not get, by the name the model calls them. A tool named here is not registered at all: no schema, no request, no entry in the registry. Read when a session is created and kept for its life, because the declarations are part of the prompt prefix; editing the file changes what the next session gets. |
| `command_max_model_bytes` | integer `1024..=1048576` | `131072` | Bytes of each `command` stream and summary that reach the model. The tool still captures up to its own larger ceiling; this bounds the text the model sees, eliding the middle with a marker. |
| `ask_user_timeout_ms` | integer `1000..=86400000` | `600000` | How long `ask_user` waits for a client answer. |
| `verbose_tool_output_chars` | integer `1..=100000` | `4000` | Longest tool output the verbose client renderer shows. |
| `read_limits.max_file_bytes` | integer `> 0` | `16777216` | Largest file the `read` tool opens. |
| `read_limits.max_output_bytes` | integer `> 0` | `65536` | Largest text `read` returns. Must not exceed `max_file_bytes`. |
| `read_limits.max_lines` | integer `> 0` | `2000` | Lines returned by a read without a line range. |
| `read_limits.max_matches` | integer `> 0` | `200` | Matches returned by a regex read. |
| `read_limits.max_line_width` | integer `> 0` | `500` | Width at which a long line is cut, in characters. |
| `shell_limits.allow_indefinite` | boolean | `true` | Whether a `shell` call asking for `timeout_ms: 0` runs without a bound. Off, the default bound is applied instead. Detached jobs use the same setting. |
| `shell_limits.default_timeout_ms` | integer `1..=86400000` | `120000` | Bound applied when a `shell` call asks for none, or asks for zero while indefinite runs are off. |
| `shell_limits.max_timeout_ms` | integer `1..=86400000` | `600000` | Ceiling for a `shell` call's requested bound. A larger request is clamped to it. |
| `confinement.workspace_only` | boolean | `true` | Confine tool paths to the workspace. Off, `read`, `write`, `edit`, `glob`, `list_dir`, `grep` and `command`'s `cwd` may name any path. A special file such as a FIFO is refused either way. |
| `confinement.sandbox.enabled` | boolean | `false` | Run `command` inside bubblewrap (Linux) or `sandbox-exec` (macOS). |
| `confinement.sandbox.network` | boolean | `false` | Allow network access inside the sandbox. |
| `confinement.sandbox.writable` | array of paths | `[]` | Extra writable directories besides the workspace. Relative paths resolve against the workspace; missing paths are ignored. |

`read_limits` applies to the `read` tool only. `write` and `edit` keep their own
built-in limits.

On Linux the workspace and extras are writable, the rest of the filesystem is
read-only, `/tmp` is a private tmpfs and the process tree gets new namespaces.
On macOS the seatbelt profile keeps reads, denies writes outside the workspace,
the extras and the temporary roots, and denies the network unless enabled. A
missing platform binary fails closed; there is no unsandboxed fallback.
`--sandbox`, `--no-sandbox` and `--sandbox-network on|off` override the sandbox
for one run or resume. The two halves of `confinement` are separate: the
workspace gate is a check on a tool's arguments and the sandbox is an operating
system isolation of one child process.

## `mcpServers`

Servers that speak the Model Context Protocol. An entry is either a local
program over its standard input and output, or a remote Streamable HTTP endpoint.
The block keeps the shape the rest of the ecosystem uses, so a configuration
written for another client works here unchanged. Handshakes, listings and tool
calls all use the agent's `mcp_limits.request_timeout_ms`.

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
| `limits.max_skill_file_bytes` | integer `> 0` | `1048576` | Largest `SKILL.md` accepted. |
| `limits.max_body_bytes` | integer `> 0` | `65536` | Largest body returned by `skill`. Must not exceed `max_skill_file_bytes`. |
| `limits.default_resource_bytes` | integer `> 0` | `262144` | Largest file returned by `skill_resource`. Must not exceed `max_skill_file_bytes`. |
| `limits.recommended_lines` | integer `> 0` | `500` | Line count after which a skill reports a warning. |
| `limits.max_shebang_bytes` | integer `> 0` | `4096` | Longest shebang line accepted from a bundled script. |
| `limits.max_script_output_bytes` | integer `> 0` | `1048576` | Script output kept per stream. |
| `limits.max_script_model_bytes` | integer `> 0` | `32768` | Script output that reaches the model per stream. Must not exceed `max_script_output_bytes`. |
| `limits.default_script_timeout_ms` | integer `> 0` | `120000` | Timeout used when a script call asks for none. |
| `limits.max_script_timeout_ms` | integer `> 0` | `600000` | Longest timeout a script call may ask for. Must be at least the default. |

Skills are owned by the service. Changes to these keys take effect after
`norte-code service restart`; `/refresh` only rereads the client's `cli.json`.

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
