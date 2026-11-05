# Configuration

The full field-by-field version, including every validation range, is the
repository's [`CONFIG.md`](https://github.com/JeanVydes/NorteCode/blob/dev/CONFIG.md).

## Files and precedence

1. User file: `$XDG_CONFIG_HOME/norte-code/config.json`,
   `~/.config/norte-code/config.json` or `%APPDATA%\norte-code\config.json`.
2. Project overlay: `.norte-code/config.json` in the workspace or nearest parent.

The overlay wins key by key (deep merge; arrays and scalars replace).
`--config PATH` replaces the user path. `version` must be `1`. Unknown keys
are reported as warnings, not applied. Validation errors name the field.

## Top level

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `version` | integer | `1` | Schema revision. |
| `default_provider` | string or null | null | Provider for new sessions. |
| `default_model` | string or null | null | Model id for new sessions; may contain slashes. |
| `compaction` | object | see below | Automatic compaction. |
| `retry` | object | see below | Transient retry policy. |
| `reasoning` | object | `{"default":"auto"}` | Default reasoning setting. |
| `system_prompt` | string or null | null | Base prompt replacement; `AGENTS.md` is appended after it. |
| `approval_mode` | `ask`, `auto-edit`, `yolo` | `ask` | When tools ask. |
| `approvals` | object | empty lists | Global allow/deny rules. |
| `max_iterations` | integer or null | null | Model iterations per turn. |
| `extensions` | object | `{}` | Vendor request fields. |
| `ui` | object | see below | What the client shows. |
| `cost` | object | `{"show":true}` | Cost display. |
| `budgets` | object | all null | Message, session, daily limits. |
| `tools` | object | see below | Web fetch, output cap and sandbox. |
| `skills` | object | see below | Skill discovery and scripts. |

Session titles are not configured: a new session's title is its first user
message truncated to 32 characters, written once.

Keys resolve from the provider's `environment_keys` (first set and
non-blank wins), then `secrets.json` beside the config (mode 0600). There
is no plaintext key field in `config.json`.

## `compaction`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Allow automatic compaction. |
| `threshold` | `0.90` | Context fraction in `(0, 1]`. |
| `trigger` | `idle` | `idle` or `eager`. |
| `keep_recent` | `6` | Recent messages kept verbatim. |
| `provider` | null | Summary model provider; set with `model` or not at all. |
| `model` | null | Summary model id; null uses the session model. |
| `reasoning` | `lowest` | Summary reasoning effort. |

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

## `approval_mode` and `approvals`

`ask` asks for `write`, `edit`, `shell` and `skill_script`. `auto-edit`
lets edits run and still asks for commands and scripts. `yolo` runs
everything. `approvals.allow` / `approvals.deny` are tool-wide; denial
wins. `/allow` and `/deny` edit the same rules; `global` writes the user
config.

## `ui`

What the client shows, as opposed to what the engine does.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `verbosity` | string | `medium` | `low` (answer alone), `medium` (also tools and thinking), `high` (also arguments and results). |
| `diff_style` | `unified`, `split` | `unified` | How an `edit` diff is drawn. |

`--verbose` overrides `ui.verbosity` for one run. `diff_style` is the same
diff in two shapes: `unified` (patch, fits narrow terminals) and `split`
(before/after side by side, shows changes inside a line).

## `tools.subagent`

Settings for the `subagent` tool. A subagent is a new session that runs one
turn and answers; the model delegates a task to it and reads back what it
said. The child starts blank, so the prompt carries everything it needs, and
a child never has `subagent` — nesting stops at one.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | Register the `subagent` tool. Spawning agents spends tokens on its own, so it is off until it is turned on. |
| `provider` | string or null | `null` | Provider for child sessions; null inherits the parent's provider. |
| `model` | string or null | `null` | Model for child sessions; null inherits the parent's model. |
| `allow` | array or null | `null` | Tools exposed to child sessions; null inherits the parent's tool set. |
| `deny` | array or null | `null` | Additional tools withheld from child sessions; null inherits the parent's restrictions. `ask_user`, `browser_use` and `subagent` are always denied. |
| `max_concurrent` | int | `3` | How many subagents may run at once. Past the cap a spawn is refused, not queued. |
| `max_iterations` | int or null | `null` | How many model iterations one child turn may run. `null` inherits the session's limit. |

With `detach: true` the call becomes a job (`job-N`), like a detached shell:
`manage_jobs` reads and stops it, and its result arrives as a message when it
ends.

The `subagent` tool accepts only `prompt`, `title` and `detach`. The child
starts without the parent's conversation history. It uses the configured
provider/model when present, otherwise the exact provider/model of the parent.
Its tool allow/deny policy is frozen when the child session is created.
`ask_user`, `browser_use` and `subagent` are never available to a child.

## `tools.auto_turn_on_job_end`

When enabled, a detached shell or subagent starts a new turn as soon as its
job finishes, so the model can read the result without waiting for another
user message. The result is still queued through the normal steering path, and
an active turn is never interrupted. It is disabled by default because each
automatically started turn can spend model tokens.

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
