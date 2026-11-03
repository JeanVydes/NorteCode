# Continue a session

## Find the key

Every run creates a logical session with a stable key. List them:

```sh
norte-code sessions
norte-code sessions --json
```

The listing shows the key, title, model, creation time and phase.

## Continue inside `run`

```sh
norte-code resume <key>
```

`--session` reuses the existing history and approval rules. Without it, `run`
starts a new session.

If the session is stopped by a budget or interrupted, `run --session`
does not bypass the stop; use `resume`.

## Resume

```sh
norte-code resume <key>
norte-code resume <key> --budget 50000 --budget-unit tokens
norte-code resume <key> --interactive
```

`resume` continues a stopped session, a pending question, a budget stop or a
Ctrl-C interruption. It replays a pending question so an answer given after a
restart reaches the parked tool.

`resume --budget` replaces the session budget before continuing. That is how a
session stopped by `action = "stop"` is allowed to proceed.

A session remembers the directory it was working in, and `resume` goes back
there before it builds anything: the tools are rooted at the process's own
directory, so resuming a session about one tree from inside another pointed
every read, write and command at the wrong one. A session stored before the
directory was recorded has none, and `resume` stays where the command was typed,
which is what it always did. A directory that no longer exists is reported and
the resume continues where it is.

## Keep the conversation open

```sh
norte-code new
norte-code resume <key> -i
```

The prompt keeps the same session open after each turn. The chat commands are:

| Command | Effect |
| --- | --- |
| `/help` | Show the command list. |
| `/rename [title]` | Name the session. Bare, it recomputes the name from the first message the session holds. |
| `/compact` | Summarise and continue on a fresh Norte session. |
| `/model <model>` | Switch model, keeping the provider. |
| `/models` | Open the provider/model picker; `Enter` expands a provider, and picking a model sets both. |
| `/provider <provider>` | Switch provider, keeping the model. |
| `/sessions` | Open the session picker: new session, last session, or a saved one. |
| `/reasoning <setting> [session\|message]` | Set the reasoning effort. |
| `/allow <tool> [session\|global]` | Allow a tool without asking. |
| `/deny <tool> [session\|global]` | Refuse a tool without asking. |
| `/mcp [list\|enable\|disable] [server]` | List the MCP servers, or switch one. |
| `/jobs [list\|check\|stop] [id]` | List the background jobs, read one, or stop one. |
| `/plugins list` | List installed plugins and their state. |
| `/plugins add <url>` | Install a plugin from a git URL. |
| `/plugins update [name]` | Rebuild one plugin, or all of them. |
| `/plugins enable <name>` / `/plugins disable <name>` | Load or skip a plugin at the next start. |
| `/debug` | Report what the process knows: model, tools, paths. |
| `/telemetry` | The request ledger for this session and every provider. |
| `/refresh`, `/reload` | Re-read the config, the skills and the MCP servers. |
| `/restart` | Re-read the config and rebuild the engine; this session stays saved. |
| `/menu`, `/back` | Show every provider and the models each lists, with the session's own marked. It keeps the session; pick with `/provider` and `/model`. |
| `/delete` | Delete this session, with its cost and telemetry, and leave. |
| `/exit`, `/quit` | Leave. |

Ctrl-D or Ctrl-C at the prompt also leaves. Commands with an optional argument —
`/models`, `/provider`, `/mcp`, `/jobs` — do something useful without it
instead of erroring, so asking is never a mistake.

A question from the model is answered the same way the startup menu is: the
choices are drawn where the draft is drawn and the arrow keys walk them. The
last row is "something else", and choosing it is what opens the input box; until
it is chosen there is no box, because the choices are the answer. `Space` marks
a choice when the model allows several, and `Escape` leaves the question
unanswered rather than inventing a selection nobody made.

`/rename` writes a name without calling anything when given one. Bare, it
recomputes the name from the first message the session holds, using the same
32-character rule a new session gets at creation — which is how a session
opened before that rule gets a name, since its automatic title was made when it
was created.

## Interruption

Ctrl-C during a turn cancels it: the running tool is asked to stop and the turn
finishes uncommitted. A second Ctrl-C within five seconds only repeats "still
cancelling"; after five seconds it force-exits. The session stays resumable.

Ctrl-C at a question prompt does **not** submit an answer.
The question stays pending and the command prints how to resume.

## Compaction changes the engine, not the key

After compaction the logical key is unchanged; the session listing shows the new
Norte session id. The old Norte session remains in the database for audit and is
linked as `compacted_from`.
