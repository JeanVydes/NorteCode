# Configuration

The full field-by-field version, including every validation range, is the
repository's [`CONFIG.md`](https://github.com/JeanVydes/NorteCode/blob/dev/CONFIG.md).

## Files and precedence

### Server configuration

1. User file: `$XDG_CONFIG_HOME/norte-code/service.json`,
   `~/.config/norte-code/service.json` or `%APPDATA%\norte-code\service.json`.
2. Project overlay: `.norte-code/service.json` in the workspace or nearest parent.

The overlay wins key by key (deep merge; arrays and scalars replace).
`version` must be `1`. Unknown keys
are reported as warnings, not applied. Validation errors name the field.

### CLI configuration

The CLI reads only `cli.json`, from the corresponding platform config
directory or `.norte-code/cli.json`. Use `--cli-config PATH` to select it.
Its service settings are:

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `service_url` | string | `http://127.0.0.1:8787` | RAEP service endpoint. |
| `service_type` | `local` or `remote` | `local` | Whether lifecycle commands may manage a local process. |
| `service_auth` | string or null | null | Raw bearer token, without the `Bearer` prefix. |
| `agent_id` | string | `norte-code` | Agent identifier sent during RAEP attachment. |
| `verbosity` | `low`, `medium` or `high` | `medium` | Output detail for the terminal client. |
| `queue_mode` | `reject`, `queue` or `interrupt` | `queue` | What a message typed while a turn runs does. |
| `service_lifecycle` | object | see below | Tokio workers and local service timings. |
| `reconnect` | object | see below | Client reconnect backoff. |
| `retention` | object | see below | Transcript and console retention. |

The three runtime blocks are client-owned. Liveness and readiness probe
timeouts stay fixed. Attachment size stays at 16 MiB and has no setting.
Question and review prompt text has no character limit setting.

| `service_lifecycle` key | Default | Meaning |
| --- | --- | --- |
| `tokio_workers` | `4` | Worker threads for a local service. `NORTE_CODE_TOKIO_WORKERS` wins when set. |
| `startup_wait_ms` | `20000` | Time to wait for readiness. |
| `startup_poll_ms` | `150` | Readiness check interval. |
| `stop_wait_ms` | `10000` | Time to wait for graceful stop. |
| `stop_poll_ms` | `100` | Process check interval while stopping. |
| `kill_settle_ms` | `200` | Grace after a kill signal. |

| `reconnect` key | Default | Meaning |
| --- | --- | --- |
| `retry_base_ms` | `400` | Delay before the first reconnect. |
| `retry_max_ms` | `5000` | Largest reconnect delay. |
| `give_up_attempts` | `3` | Failed attempts before reporting giving up. |
| `healthy_stream_s` | `10` | Healthy stream time before resets are cleared. |

| `retention` key | Default | Meaning |
| --- | --- | --- |
| `transcript_max_items` | `1000` | Largest transcript held in memory. |
| `transcript_keep_items` | `800` | Transcript rows kept after a trim. |
| `console_max_lines` | `500` | Largest console buffer. |
| `console_keep_lines` | `400` | Console lines kept after a trim. |

## Service envelope and agents

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `version` | integer | `1` | Schema revision. |
| `default_agent` | string | `norte-code` | Agent selected for new attachments. |
| `agents` | array | one agent | Named agent definitions. |
| `raep_limits` | object | see below | Service host capacity and retention. |

Each agent has a unique required `name`; `description`, `origin_url` and
`author` are optional metadata. The RAEP server exposes all named agents, and
`cli.json.agent_id` selects the agent used by the CLI.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `subagent` | string or null | null | Default child agent; it must also be in `delegates`. |
| `delegates` | array of agent ids | `[]` | Child profiles this agent may spawn. |
| `compaction` | object | see below | Automatic compaction. |
| `retry` | object | see below | Transient retry policy. |
| `provider_http` | object | see below | Provider transport policy. |
| `delegation` | object | see below | Child session bounds. |
| `mcp_limits` | object | see below | Shared MCP request timeout. |
| `reasoning` | object | `{"default":"auto"}` | Default reasoning setting. |
| `system_prompt` | string or null | null | Base prompt replacement; `AGENTS.md` is appended after it. |
| `approvals` | object | empty lists | Global allow/deny rules. |
| `max_iterations` | integer or null | null | Model iterations per turn. |
| `extensions` | object | `{}` | Vendor request fields. |
| `ui` | object | see below | Legacy server-side UI defaults retained for compatibility. |
| `cost` | object | `{"show":true}` | Cost display. |
| `budgets` | object | all null | Message, session, daily limits. |
| `tools` | object | see below | Web fetch, output cap and sandbox. |
| `skills` | object | see below | Skill discovery and scripts. |

Session titles are not configured: an untitled session is named by a one-shot
internal model request on its first user message. A title chosen by the user is
never replaced.

Keys resolve from the provider's `environment_keys` (first set and
non-blank wins), then `secrets.json` beside the config (mode 0600). There
is no plaintext key field in `service.json`.

## `compaction`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Allow automatic compaction. |
| `threshold` | `0.90` | Context fraction in `(0, 1]`. |
| `trigger` | `idle` | `idle` or `eager`. |
| `keep_recent` | `6` | Recent messages kept verbatim. |
Compaction uses the session's provider, model and effective reasoning.

## `retry`

| Key | Default | Meaning |
| --- | --- | --- |
| `max_attempts` | `4` | Retries after the first failure; `0` disables. |
| `max_delay_ms` | `60000` | Upper bound for one wait. |
| `backoff_ms` | `5000` | First wait; doubles. |

Only rate limits, overload, server errors and transport errors are retried.
`retry` handles failures after a call. `provider_http` bounds the transport
underneath the call; the two policies are separate.

## `provider_http`

| Key | Default | Meaning |
| --- | --- | --- |
| `connect_max_retries` | `3` | Attempts to establish a connection. |
| `connect_timeout_ms` | `10000` | Deadline for one connection attempt. |
| `request_timeout_ms` | `120000` | Deadline for the whole request. |
| `stream_idle_timeout_ms` | `300000` | Silent stream timeout. |

## `delegation`

| Key | Default | Meaning |
| --- | --- | --- |
| `max_depth` | `3` | Deepest chain of child sessions. |
| `max_concurrent` | `3` | Child sessions that may run at once. |
| `max_held_messages` | `20` | Delegate queue size: parent messages held for a running child. |

## `mcp_limits`

| Key | Default | Meaning |
| --- | --- | --- |
| `request_timeout_ms` | `60000` | Deadline for MCP handshakes, listings and calls. |

## `raep_limits`

This block is service-wide and sits outside `agents`.

| Key | Default | Meaning |
| --- | --- | --- |
| `replay_capacity` | `2000` | Frames kept for replay. |
| `replay_bytes` | `8388608` | Bytes retained across replay frames. |
| `max_concurrent_jobs` | `8` | Jobs that may run at once. |
| `job_output_retention_s` | `86400` | Seconds finished job output is kept. |
| `idempotency_capacity` | `4096` | Turn request fingerprints kept. |
| `max_queued_inputs` | `8` | Inputs waiting behind a running turn. |
| `max_frame_bytes` | `1048576` | Largest accepted frame. |
| `max_artifact_bytes` | `16777216` | Largest accepted uploaded artifact. |
| `max_export_bytes` | `67108864` | Largest exported bundle. |
| `export_ttl_s` | `3600` | Seconds a generated bundle can be downloaded. |
| `max_connections` | `64` | Concurrent WebSocket and SSE clients. |
| `command_timeout_ms` | `120000` | Deadline for handling one command. |

`NORTE_CODE_REPLAY_CAPACITY` and `NORTE_CODE_MAX_CONNECTIONS` still override the
matching fields when they are set to positive values.

## `reasoning`

`default` is `auto`, `off`, an effort name (`minimal`, `low`, `medium`,
`high`) or `{"budget": N}`. It is checked against the selected model.

## `approvals`

Tool access is deterministic: an explicit deny rejects a tool, an explicit
allow permits it, and an unconfigured tool follows the host default. The
`approvals.allow` and `approvals.deny` lists are global; `/allow` and `/deny`
edit session rules, or global rules when followed by `global`.

## `ui`

What the client shows, as opposed to what the engine does.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `verbosity` | string | `medium` | `low` (answer alone), `medium` (also tools and thinking), `high` (also arguments and results). |
| `queue_mode` | string | `queue` | `reject` (refuse while busy), `queue` (steer into the running turn), `interrupt` (cancel the turn, then steer). |
| `diff_style` | `unified`, `split` | `unified` | How an `edit` diff is drawn. |

`--verbose` overrides `ui.verbosity` for one run. `diff_style` is the same
diff in two shapes: `unified` (patch, fits narrow terminals) and `split`
(before/after side by side, shows changes inside a line).

## Delegation

Each agent may set `subagent` to its default child profile and `delegates` to
the list of configured agent names it is allowed to spawn. An empty
`delegates` list disables delegation; a configured `subagent` must also appear
in `delegates`. `spawn_agent.agent_id` may select another profile from that
allow-list. The selected profile supplies the child's model, system prompt
and disabled-tool policy. The allow-list is observable at runtime: the error
for an unallowed `agent_id` names the allowed profiles, and `list_agents`
also reports the allow-list for the session alongside the spawned children.

The child starts with the task only. `context_mode: "summary"` adds a bounded,
redacted excerpt of the parent's history. Children can spawn further children
up to `delegation.max_depth`. Child turns share the parent's
budget ledger; separate budget-share reservation is not implemented.

With `detach: true`, the child runs as a `job-N`; `manage_jobs` can inspect or
stop it, and its result is delivered to the parent when the run finishes.
`message_agent`, `stop_agent`, `wait_agent`, `list_agents` and
`inspect_agent` provide child lifecycle controls.

The tool accepts `lifetime: "turn"` or `"session"`, but the host does not yet
enforce different cleanup behavior for those values. Automatic idle expiry,
parent-close cleanup, full event inspection, approval routing and
delegation-tool idempotency are also not implemented.

## `tools.auto_turn_on_job_end`

When enabled, a detached shell or subagent starts a new turn as soon as its
job finishes, so the model can read the result without waiting for another
user message. The result is still queued through the normal steering path, and
an active turn is never interrupted. It is disabled by default because each
automatically started turn can spend model tokens.

RAEP sessions use the RFC `job_turn_policy` session option instead: `always`
(the RAEP default), `on_failure`, or `never`. A controller can set it through
`SESSION_ATTACH` or `SESSION_CONFIGURE`; observer attachments cannot change it.

## `budgets`

`message`, `session` and `global_daily` each accept null or:

| Key | Meaning |
| --- | --- |
| `unit` | `usd`, `tokens` or `requests`. |
| `limit` | A positive number. |
| `action` | `warn`, `ask` or `stop`. |

`ask` is a resumable stop with a notice. See [Set a budget](../guides/set-a-budget.md).

## `tools`

| Key | Default | Meaning |
| --- | --- | --- |
| `web_fetch` | `false` | Register `web_fetch`. |
| `auto_turn_on_job_end` | `false` | Start a turn when a detached shell or subagent finishes. |
| `disabled` | `[]` | Tools a new session does not get, by model name; frozen at creation. |
| `command_max_model_bytes` | `131072` | Bytes of each `shell` stream that reach the model; range `1024` to `1048576`. |
| `ask_user_timeout_ms` | `600000` | How long `ask_user` waits for an answer. |
| `verbose_tool_output_chars` | `4000` | Longest tool output shown by the verbose renderer. |
| `read_limits.max_file_bytes` | `16777216` | Largest file `read` opens. |
| `read_limits.max_output_bytes` | `65536` | Largest text `read` returns. |
| `read_limits.max_lines` | `2000` | Lines returned without a line range. |
| `read_limits.max_matches` | `200` | Matches returned by a regex read. |
| `read_limits.max_line_width` | `500` | Width at which a long line is cut. |
| `shell_limits.allow_indefinite` | `true` | Grant `timeout_ms: 0` as an unbounded call; detached jobs use the same setting. |
| `shell_limits.default_timeout_ms` | `120000` | Bound used when no time is asked for, or zero is asked for while indefinite runs are off. |
| `shell_limits.max_timeout_ms` | `600000` | Ceiling for a requested bound; larger requests are clamped. |
| `confinement.workspace_only` | `true` | Confine tool paths to the workspace. |
| `confinement.sandbox.enabled` | `false` | Wrap commands in the OS sandbox. |
| `confinement.sandbox.network` | `false` | Allow network in the sandbox. |
| `confinement.sandbox.writable` | `[]` | Extra writable roots. |

`read_limits` applies to `read` only. `write` and `edit` keep their built-in
limits.

## `skills`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Discover and register skills. |
| `paths` | `[]` | Extra roots after the user root. |
| `interop` | `true` | Read `.agents/skills` roots. |
| `allow_scripts` | `false` | Register `skill_script`; outside the command sandbox. |
| `honor_allowed_tools` | `false` | Let a skill pre-approve calls. |
| `max_skills` | `128` | Maximum valid skills. |
| `limits.max_skill_file_bytes` | `1048576` | Largest `SKILL.md` accepted. |
| `limits.max_body_bytes` | `65536` | Largest body returned by `skill`. |
| `limits.default_resource_bytes` | `262144` | Largest file returned by `skill_resource`. |
| `limits.recommended_lines` | `500` | Line count before a warning. |
| `limits.max_shebang_bytes` | `4096` | Longest accepted shebang line. |
| `limits.max_script_output_bytes` | `1048576` | Script output kept per stream. |
| `limits.max_script_model_bytes` | `32768` | Script output reaching the model per stream. |
| `limits.default_script_timeout_ms` | `120000` | Timeout when a call asks for none. |
| `limits.max_script_timeout_ms` | `600000` | Longest timeout a call may ask for. |

## `extensions`

Keys are `<wire-family>:<dotted.path>`, values are JSON. A key whose wire
family does not match the active model is ignored.
