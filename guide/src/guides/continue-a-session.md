# Continue a session

## Find the key

Every run creates a session with a stable key. List them:

```sh
norte-code sessions
norte-code sessions --json
```

The listing shows the key, title, model, creation time and phase.

## Resume

```sh
norte-code resume <key>
norte-code resume <key> --budget 50000 --budget-unit tokens
norte-code resume <key> -i
```

`resume` continues a stopped session, a pending question, a budget stop or a
Ctrl-C interruption. `resume --budget` replaces the session budget first —
that is how a session stopped by `action = "stop"` is allowed to proceed.

A session remembers the directory it was working in, and `resume` returns to
it before building anything, so tools point at the right tree. If that
directory no longer exists, the resume reports it and continues where you are.

## Keep the conversation open

```sh
norte-code new
norte-code resume <key> -i
```

The prompt keeps the same session open after each turn. The chat commands:

| Command | Effect |
| --- | --- |
| `/help` | Show the command list. |
| `/rename [title]` | Name the session. Bare, it recomputes the name from the first message. |
| `/compact` | Summarise and continue on a fresh engine session. |
| `/model <model>` | Switch model, keeping the provider. |
| `/models` | Provider/model picker; type to filter, `Enter` expands, picking sets both. |
| `/provider <provider>` | Switch provider, keeping the model. |
| `/sessions` | Session picker: new, last, or a saved one. |
| `/reasoning <setting> [session\|message]` | Set the reasoning effort. |
| `/allow <tool> [session\|global]` | Allow a tool without asking. |
| `/deny <tool> [session\|global]` | Refuse a tool without asking. |
| `/mcp [list\|enable\|disable] [server]` | List the MCP servers, or switch one. |
| `/jobs [list\|check\|stop] [id]` | Background jobs: list, read one, or stop one. |
| `/plans` | List the session's saved plans. |
| `/plugins list` | List installed plugins. |
| `/plugins add <url> [package]` | Install a plugin from a git URL. |
| `/plugins update [name]` | Rebuild one plugin, or all of them. |
| `/plugins enable <name>` / `/plugins disable <name>` | Load or skip a plugin at next start. |
| `/debug` | What the process knows: model, tools, paths. |
| `/telemetry` | Request ledger for this session and every provider. |
| `/refresh`, `/reload` | Re-read the client `cli.json`; service-side changes require `service restart`. |
| `/restart` | Re-read config and rebuild the engine; the session stays saved. |
| `/menu`, `/back` | Every provider and its models, the session's own marked. |
| `/delete` | Delete this session, with its cost and telemetry, and leave. |
| `/exit`, `/quit` | Leave. |

Ctrl-D or Ctrl-C at the prompt also leaves. Commands with an optional
argument (`/models`, `/provider`, `/mcp`, `/jobs`) do something useful
without it instead of erroring.

A question from the model is answered with arrow keys over its choices; the
last row is "something else", and choosing it opens the input box. `Space`
marks a choice when several are allowed; `Escape` leaves it unanswered.

## Interruption

Ctrl-C during a turn cancels it: the running tool is asked to stop and the
turn ends uncommitted. A second Ctrl-C soon after only repeats "still
cancelling"; after five seconds it force-exits. The session stays resumable.

Ctrl-C at a question prompt does **not** submit an answer. The question
stays pending and the command prints how to resume.

## Compaction

After compaction the session key is unchanged; only the engine session
behind it is new. The old one stays in the database for audit.
