# Tools and approvals

## The tool set

Every engine gets the same configured registry:

| Tool | What it does | Approval |
| --- | --- | --- |
| `read` | Read a file. | No |
| `write` | Write a file. | Yes in `ask` and `auto-edit` |
| `edit` | Replace exact text in a file. | Yes in `ask` and `auto-edit` |
| `shell` | Run a shell script, with a cleared environment. | Yes in every mode except `yolo` |
| `glob` | Find paths matching a glob, honoring `.gitignore`. | No |
| `list_dir` | List one directory, directories first. | No |
| `grep` | Search file contents. | No |
| `todo` | Maintain the session's todo list. | No |
| `ask_user` | Park the turn and ask the human. | No |
| `web_fetch` | Fetch a URL; opt-in. | No |
| `skill` / `skill_resource` | Load a discovered Agent Skill or one of its resources. | No |
| `skill_script` | Run a skill script; opt-in and outside the command sandbox. | Yes |

Tool output is bounded before it crosses the wire. Failed tools return an error
text the model can read; they do not abort the engine.

## Approval modes

`approval_mode` selects the baseline:

| Mode | Behaviour |
| --- | --- |
| `ask` (default) | `write`, `edit`, `shell` and `skill_script` ask first. |
| `auto-edit` | Edits run; `shell` and `skill_script` still ask. |
| `yolo` | Every tool runs without asking. |

The mode is a host policy, not a promise about the model. A deny rule wins even
in `yolo`.

## Rules and scopes

Rules are tool-wide and live at two scopes:

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

## The approval panel

When a call parks, the client shows the tool, a semantic preview of its arguments
and, for commands, whether a sandbox will wrap it. The terminal choices are:

| Key | Meaning |
| --- | --- |
| `y` | Allow this call once. |
| `s` | Allow this tool for the session. |
| `a` | Allow this tool always (global). |
| `n` | Deny this call once. |
| `ns` | Deny this tool for the session. |
| `ng` | Deny this tool always (global). |

A message after a denial (`n <message>`, `ns <message>`, `ng <message>`) is sent
to the model as human feedback instead of a bare refusal.

Approval resumes the same turn. Input comes from the terminal, never from the
model. With no interactive terminal the session stays parked, which is what a
JSONL script expects.

## Grants and durable denials

A session allow becomes a Norte grant, so the engine does not ask again for the
same scope. Session denials are persisted with the logical session and survive a
restart. Global rules are written to the user config.

A one-shot approval is not revocable through a command today; session and global
allow rules can be changed with `/allow` and `/deny`.

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
