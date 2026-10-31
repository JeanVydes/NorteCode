# Tools and rules

## The tool set

Every engine gets the same configured registry:

| Tool | What it does |
| --- | --- |
| `read` | Read a file. |
| `write` | Write a file. |
| `edit` | Replace exact text in a file. |
| `shell` | Run a shell script, with a cleared environment. |
| `grep` | Search file contents, with a glob to narrow the files. |
| `todo` | Maintain the session's todo list. |
| `ask_user` | Park the turn and ask the human. |
| `web_fetch` | Fetch a URL; opt-in. |
| `skill` / `skill_resource` | Load a discovered Agent Skill or one of its resources. |
| `skill_script` | Run a skill script; opt-in and outside the command sandbox. |

Tool output is bounded before it crosses the wire. Failed tools return an error
text the model can read; they do not abort the engine.

### Withholding a tool

`tools.disabled` names tools a new session does not get:

```json
{
  "tools": {
    "disabled": ["shell", "browser_use"]
  }
}
```

A tool named here is never registered. It is not in the schema the model is
shown, not in the request, and not in the registry a call is looked up in, so
the model has no way to ask for it and pays nothing for its declaration.

The list is read when a session is **created** and kept for that session's whole
life. The declarations are part of the prompt's prefix, so a set that changed
under a live conversation would throw away the provider's cached prefix and
re-bill every message in it. Editing the list changes what the next session
gets; it does not reach back into one that already exists, and a resumed session
keeps the tools it was made with even if the file has moved on.

## Rules and scopes

A rule names a tool and says whether calls to it run. Denial wins over allow,
and a global deny wins over a session allow. Rules are tool-wide and live at two
scopes:

- **Global** rules in the user config's `approvals.allow` and `approvals.deny`.
- **Session** rules stored with the logical session.

Denial wins over allow. A global deny wins over a session allow. An allow rule
replaces an existing deny for the same tool, which is how the interactive prompt
revokes a saved deny.

In the REPL:

```text
/allow shell session
/deny shell global
```

The scope defaults to `session`. The name is the one the model calls the tool
by, which is what `/tools` prints beside each rule: a rule that names a tool
nobody has does nothing, and does not say so.

## What a denied call looks like

The engine consults the rule immediately before a tool call runs, when the
call's name, id and arguments are known and nothing has happened. A denied call
is answered right away: the model reads the rule's reason like a failed call
and adapts. Nothing waits on stdin, no turn parks, and the engine reports the
call started and finished as a failure so the live view shows what never ran.

A denied call still opens in the transcript: its result is committed with the
tool results, so a replay sees what the model saw.

## Questions

`ask_user` is a tool, but its result is a human answer rather than a command
decision. The model may offer an `options` list and set `multiple: true`; the
terminal appends an "answer something else" entry so free-form input is always
possible. The answer resumes the parked turn through `AnswerQuestion`.

## Skill tools

Agent Skills get three tools:

- `skill` loads the skill's instructions.
- `skill_resource` reads a listed resource.
- `skill_script` runs a script, only when `skills.allow_scripts` is true.

A skill's `allowed-tools` can pre-approve calls only when
`skills.honor_allowed_tools` is enabled. Scripts are not covered by the command
sandbox.
