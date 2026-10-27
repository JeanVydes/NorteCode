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

If the session is parked, stopped by a budget or interrupted, `run --session`
does not bypass the park; use `resume`.

## Resume

```sh
norte-code resume <key>
norte-code resume <key> --budget 50000 --budget-unit tokens
norte-code resume <key> --interactive
```

`resume` continues a parked approval, a pending question, a budget stop or a
Ctrl-C interruption. It re-emits approvals that are still waiting and replays a
pending question so an answer given after a restart reaches the parked tool.

`resume --budget` replaces the session budget before continuing. That is how a
session stopped by `action = "stop"` is allowed to proceed.

## Keep the conversation open

```sh
norte-code new
norte-code resume <key> -i
```

The prompt keeps the same session open after each turn. The chat commands are:

| Command | Effect |
| --- | --- |
| `/help` | Show the command list. |
| `/compact` | Summarise and continue on a fresh Norte session. |
| `/allow <tool> [session\|global]` | Allow a tool without asking. |
| `/deny <tool> [session\|global]` | Refuse a tool without asking. |
| `/refresh` | Re-scan skill roots and their tools. |
| `/exit`, `/quit` | Leave. |

Ctrl-D or Ctrl-C at the prompt also leaves.

## Interruption

Ctrl-C during a turn cancels it: the running tool is asked to stop and the turn
finishes uncommitted. A second Ctrl-C within five seconds only repeats "still
cancelling"; after five seconds it force-exits. The session stays resumable.

Ctrl-C at an approval panel or question prompt does **not** submit a decision.
The approval or question stays pending and the command prints how to resume.

## Compaction changes the engine, not the key

After compaction the logical key is unchanged; the session listing shows the new
Norte session id. The old Norte session remains in the database for audit and is
linked as `compacted_from`.
