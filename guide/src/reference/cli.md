# CLI

The binary is `norte`. It is the only entry point for Norte Code: it
manages the background service and, when asked, opens the terminal
interface. The service is what owns sessions, execution, catalogues and
plugins; `norte` talks to it over HTTP.

```text
norte [COMMAND]
```

## Global options

| Option | Meaning |
| --- | --- |
| `--version`, `-V` | Print the version and exit. |
| `--help`, `-h` | Show this help. |

## Subcommands

### `norte` (no subcommand)

Starts the service if it is not already running (detached, in the
background) and then opens the terminal interface in the current terminal.
Ctrl-C in that terminal closes the interface; the service keeps running.

### `norte tui`

Opens only the terminal interface in the current terminal, without touching
the service.

### `norte service up`

Starts the service detached. Prints the PID and the log path; if the
service is already up it says so and does nothing.

### `norte service down`

Stops the running service with `SIGTERM` and waits for it to exit.

### `norte service restart`

`down` followed by `up`.

### `norte service status`

Prints the endpoint, PID, liveness, readiness, default agent, number of
agents loaded, replay state, and the pidfile and log paths, with colour.

### `norte service catalog`

Prints the providers and models the host knows, grouped by provider, with
colour.

### `norte service config`

Prints the service configuration — agents (delegates, model, provider,
reasoning, approvals, skills, MCP servers), plus the client configuration
(service URL, agent id, verbosity).

### `norte service logs`

Prints the service log. With `--live` (or `-f`/`--follow`) it follows new
lines until interrupted.

### `norte service doctor`

Checks paths, files, the service and the pidfile, and prints a summary
(`N ok · M warn · K fail`) with the recommended next step.

### `norte service plugins …`

Manage plugins installed on this machine (a git repository is cloned, built
as a cdylib and copied into the plugin directory).

| Form | Meaning |
| --- | --- |
| `plugins list` | Show installed plugins, enabled/disabled, and their recorded source. |
| `plugins add <URL> [--package NAME] [--all]` | Install from a git repository. With no `--package`, every plugin package the workspace can build is installed. |
| `plugins remove <NAME>` | Remove an installed plugin. |
| `plugins disable <NAME>` | Disable a plugin (rename to `.disabled`). |
| `plugins enable <NAME>` | Re-enable a disabled plugin. |

Plugins are loaded when the service starts, so restart it (`norte service
restart`) after installing, enabling or disabling.

### `norte service providers …`

Edit `providers.json` (who serves models, and over which wires).

| Form | Meaning |
| --- | --- |
| `providers list` | Show providers — the catalogue files and any a plugin injected. |
| `providers add <ID> [--name DISPLAY] [--keys KEY1,KEY2,…]` | Add or replace a provider record; `--keys` is a comma-separated list of environment variables the key comes from. Wires are managed separately below. |
| `providers add-wire <ID> --schema SCHEMA [--base-url URL] [--auth AUTH]` | Add (or replace) a wire. `--schema` is `openai`, `anthropic` or `google`; `--auth` is `bearer`, `x_api_key`, `x_goog_api_key` or `disabled` (default: the schema's own header). |
| `providers remove-wire <ID> --schema SCHEMA[,SCHEMA…]` | Remove one or more wires. |
| `providers get-wires <ID>` | Show the wires of a provider. |
| `providers remove <ID>` | Remove a provider and every model that names it. |

When the service is running, the catalogue is refreshed in place; otherwise
the files are written and picked up on the next start.

### `norte service mcps …`

Edit the MCP servers in `service.json`.

| Form | Meaning |
| --- | --- |
| `mcps list [--agent NAME]` | Show configured servers, plus the live view when the service is up. |
| `mcps add <NAME> --command CMD [--arg A] [--env K=V] [--allow T] [--deny T] [--agent NAME] [--disabled]` | Add a local (stdio) server. |
| `mcps add <NAME> --url URL [--header K=V] …` | Add a remote (Streamable HTTP) server. |
| `mcps remove <NAME> [--agent NAME]` | Remove a server. |

MCP servers are connected when the service starts, so restart it
(`norte service restart`) after adding or removing.

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
| `/refresh`, `/reload` | Re-read the client `cli.json`. Service configuration, providers, models, skills and MCP servers require `service restart`. |
| `/restart` | Re-read config and rebuild the engine at the next turn boundary. |
| `/mcp [list\|enable\|disable] [SERVER]` | List the MCP servers, or switch one. |
| `/jobs [list\|check\|stop] [ID]` | Background jobs: list, inspect, or stop one. |
| `/connection [list\|add\|remove] [PROVIDER]` | List, add or remove a saved provider key. |
| `/rename [TITLE]` | Name the session; bare recomputes from the first message. |
| `/debug` | Paths, model, catalogue, tools, servers. Never keys or URLs. |
| `/telemetry` | Request ledger: this session, and every provider. |
| `/menu`, `/back` | Every provider and its models, the session's own marked. |
| `/plans` | List the session's saved plans. |
| `/plugins …` | Plugin commands, same as the subcommands above. |
| `/delete` | Delete this session, with cost and telemetry, and leave. |
| `/exit`, `/quit` | Leave the chat. |

The editor suggests command names and arguments in dimmed text; Tab
accepts. While a turn runs, `/allow`, `/deny`, `/reasoning`, `/model`,
`/refresh`, `/help` and `/plugins …` act immediately; `/compact`,
`/models`, `/menu` and `/exit` wait for the turn to finish. Any other text
is steered into the running turn.

## Exit status

`0` success, `1` failed turn, `2` usage or configuration error.
