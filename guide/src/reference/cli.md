# CLI

The binary is `norte-code`. Run with no arguments it opens a session: a menu
of the saved sessions, then the editor.

```text
norte-code [OPTIONS] [DIR] [COMMAND]
```

## The directory

A leading directory is where the work happens:

```sh
norte-code ~/src/thing
```

It is a positional, not a flag. The process moves there before reading
config, so the project config that applies is that directory's own. A
directory that does not exist is refused with its path. The footer names it
in front of the session line.

## Global options

| Option | Meaning |
| --- | --- |
| `--config PATH` | Use this user config file instead of the platform path. |
| `--verbose LEVEL` | `low` (answer alone), `medium` (default), `high` (reasoning, tool arguments and results). |
| `--debug` | Print startup details, model metadata, paths and extensions. |

## `new`

```text
norte-code new [OPTIONS]
```

Creates a session and stays in it: the first line typed is the first message.
There is no prompt argument — a session is read, not handed a message.

| Option | Meaning |
| --- | --- |
| `-m, --model MODEL` | Model id, overriding `default_model`. Opaque; may contain slashes. |
| `--provider PROVIDER` | Override the provider id. |
| `--system-prompt TEXT` | Replace the base system prompt for this session. |
| `--endpoint URL` | Override the provider endpoint. |
| `--wire SCHEMA` | `openai`, `anthropic` or `google`. Default: the provider's preferred. |
| `--budget LIMIT` | A limit for the first message. |
| `--budget-unit usd\|tokens\|requests` | What the limit measures. Default `usd`. |
| `--budget-action warn\|ask\|stop` | What to do at the limit. |
| `--compaction-threshold FRACTION` | Override the compaction threshold in `(0, 1]`. |
| `--sandbox` / `--no-sandbox` | Override `tools.confinement.sandbox.enabled`. |
| `--sandbox-network on\|off` | Override sandbox networking; requires the sandbox. |
| `--reasoning SETTING` | `auto`, `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` or `budget:<tokens>`. |
| `--max-iterations N` | Cap model iterations for this session. |
| `--extension KEY=JSON` | Vendor request field; repeatable. |

Without `--model`/`--provider`, the session opens on the config defaults —
and if those are unset, startup asks once and writes the answer to the config.

## The startup menu

A bare `norte-code` and `resume` with no key open the same menu: *New
session*, *Last session*, then saved sessions newest first. Arrow keys walk
it, `Enter` opens, `Escape` leaves, typing narrows.

## `resume`

```text
norte-code resume [OPTIONS] [SESSION]
```

Continues a session (key or prefix). With no `SESSION`, the menu asks.

| Option | Meaning |
| --- | --- |
| `--system-prompt TEXT` | Replace the base prompt for this resume. |
| `--model MODEL` | Resume with this model. |
| `--budget LIMIT` | Replace the session budget before resuming. |
| `--budget-unit`, `--budget-action` | Unit and action for that budget. |
| `--compaction-threshold FRACTION` | Override the threshold for this resume. |
| `--sandbox`, `--no-sandbox`, `--sandbox-network` | Sandbox overrides. |
| `--max-iterations N` | Iteration cap. |
| `--extension KEY=JSON` | Vendor request field; repeatable. |
| `-i, --interactive` | Keep the session open after each turn. |

A resume whose turn is over replays the transcript; an interrupted one
continues where it stopped.

## `models`

```text
norte-code models [--provider ID] [--tools] [--reasoning] [--json]
```

Lists what the catalogue knows. `--tools`/`--reasoning` filter to models
that call tools / reason.

## `skills`

```text
norte-code skills list [--json]
norte-code skills check
```

`list` prints valid skills; `check` validates every skill and reports the
skipped ones and why.

## `sessions`

```text
norte-code sessions [--json]
```

Lists logical sessions: key, title, model, creation time and phase.

## `cost`

```text
norte-code cost [--session KEY] [--daily] [--json]
```

The provider-reported cost ledger. `--session` adds the session's turns;
`--daily` groups by UTC day.

## `telemetry`

```text
norte-code telemetry [--session KEY] [--provider ID] [--model ID] [--bucket hour|day|week|month|year|total] [--json]
```

Local request data — counts, tokens, cache, tools, latency, status. Not the
money ledger. Default bucket is `day`.

## `mcp`

```text
norte-code mcp list
norte-code mcp add NAME COMMAND [ARGS...]
norte-code mcp enable NAME
norte-code mcp disable NAME
norte-code mcp remove NAME
```

Manages the MCP servers in the config file: list them, add or replace one
by name, switch one on/off, or remove it.

## `plugins`

Plugins are shared libraries under `~/.config/norte-code/plugins/` (or
`$NORTE_CODE_PLUGINS`). A plugin adds slash commands, a status line, or
providers and models to the catalogue. Installing a plugin clones and
builds untrusted repository code — including its `build.rs` — before
loading the resulting library. That is explicit by design: only do it for
sources you trust.

```text
norte-code plugins list
norte-code plugins add URL [--package PACKAGE]
norte-code plugins rm NAME
norte-code plugins update [NAME]
norte-code plugins enable NAME
norte-code plugins disable NAME
```

Changes take effect at the next start; plugins load once, before the
catalogue is read. The chat has the same commands under `/plugins …`.

## Chat commands

A session reads commands from the same line editor. A command is recognised
as soon as it starts with `/`; anything else is a message.

| Command | Meaning |
| --- | --- |
| `/help` | Show the command list. |
| `/compact` | Summarise the session and continue on a new one. |
| `/allow TOOL [session\|global]` | Allow a tool without asking. |
| `/deny TOOL [session\|global]` | Refuse a tool without asking. |
| `/tools` | List every tool the model can reach, with its rule state. |
| `/reasoning SETTING [session\|message]` | Set the reasoning override. |
| `/model PROVIDER/MODEL` | Switch the model at the next turn boundary. |
| `/models` | Provider/model picker; typing filters, `Enter` expands, picking sets both. |
| `/provider NAME` | Switch provider, keeping the model where it has one. |
| `/sessions` | Session picker: new, last, or a saved one. |
| `/refresh`, `/reload` | Re-read config, skills and MCP servers. |
| `/restart` | Re-read config and rebuild the engine at the next turn boundary. |
| `/mcp [list\|enable\|disable] [SERVER]` | List the MCP servers, or switch one. |
| `/jobs [list\|check\|stop] [ID]` | Background jobs: list, inspect, or stop one. |
| `/connection [list\|add\|remove] [PROVIDER]` | List, add or remove a saved provider key. |
| `/rename [TITLE]` | Name the session; bare recomputes from the first message. |
| `/debug` | Paths, model, catalogue, tools, servers. Never keys or URLs. |
| `/telemetry` | Request ledger: this session, and every provider. |
| `/menu`, `/back` | Every provider and its models, the session's own marked. |
| `/plans` | List the session's saved plans. |
| `/plugins …` | The plugin commands, same as the subcommands above. |
| `/delete` | Delete this session, with cost and telemetry, and leave. |
| `/exit`, `/quit` | Leave the chat. |

The editor suggests command names and arguments in dimmed text; Tab
accepts. While a turn runs, `/allow`, `/deny`, `/reasoning`, `/model`,
`/refresh`, `/help` and `/plugins …` act immediately; `/compact`,
`/models`, `/menu` and `/exit` wait for the turn to finish. Any other text
is steered into the running turn.

## Exit status

`0` success, `1` failed turn (`skills check` also exits `1` on warnings),
`2` usage or configuration error.
