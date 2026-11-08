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

## Service envelope and agents

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `version` | integer | `1` | Schema revision. |
| `default_agent` | string | `norte-code` | Agent selected for new attachments. |
| `agents` | array | one agent | Named agent definitions. |

Each agent has a unique required `name`; `description`, `origin_url` and
`author` are optional metadata. The RAEP server exposes all named agents, and
`cli.json.agent_id` selects the agent used by the CLI.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `subagent` | string or null | null | Default child agent; it must also be in `delegates`. |
| `delegates` | array of agent ids | `[]` | Child profiles this agent may spawn. |
| `compaction` | object | see below | Automatic compaction. |
| `retry` | object | see below | Transient retry policy. |
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

Session titles are not configured: a new session's title is its first user
message truncated to 32 characters, written once.

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
and disabled-tool policy.

The child starts with the task only. `context_mode: "summary"` adds a bounded,
redacted excerpt of the parent's history. Children can spawn further children
up to the host's maximum delegation depth. Child turns share the parent's
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
| `command_max_model_bytes` | `131072` | Bytes of each `shell` stream that reach the model; range `1024`–`1048576`. |
| `confinement.workspace_only` | `false` | Confine tool paths to the workspace. |
| `confinement.sandbox.enabled` | `false` | Wrap commands in the OS sandbox. |
| `confinement.sandbox.network` | `false` | Allow network in the sandbox. |
| `confinement.sandbox.writable` | `[]` | Extra writable roots. |

## `skills`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Discover and register skills. |
| `paths` | `[]` | Extra roots after the user root. |
| `interop` | `true` | Read `.agents/skills` roots. |
| `allow_scripts` | `false` | Register `skill_script`; outside the command sandbox. |
| `honor_allowed_tools` | `false` | Let a skill pre-approve calls. |
| `max_skills` | `128` | Maximum valid skills. |

## `extensions`

Keys are `<wire-family>:<dotted.path>`, values are JSON. A key whose wire
family does not match the active model is ignored.
