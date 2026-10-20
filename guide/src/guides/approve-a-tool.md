# Approve a tool

## What parks

In `ask` (the default), `write`, `edit`, `command` and `skill_script` park
before running. In `auto-edit`, writes and edits run; commands and skill scripts
still park. `yolo` runs everything. A global or session deny wins over the mode.

When a call parks, the turn is durable. You can answer now, or exit and resume
later.

## The prompt

The terminal shows the tool, a semantic preview of the arguments and, for a
command, the sandbox badge. The options are:

```text
y    allow once
s    allow this tool for the session
a    allow this tool always
n    deny once
ns   deny this tool for the session
ng   deny this tool always
```

A message after a denial reaches the model as feedback:

```text
n use the existing helper instead
```

`y`, `s` and `a` do not accept a message.

## Rules without a pending call

In the interactive prompt:

```text
/allow command session
/allow edit global
/deny command global
```

The scope defaults to `session`. Global rules are written to the user config's
`approvals.allow` / `approvals.deny`; session rules are stored with the logical
session. An allow replaces an existing deny for the same tool.

The same rules can be written directly:

```json
{
  "approvals": {
    "allow": ["read", "glob"],
    "deny": ["command"]
  }
}
```

## Deny is sticky

Denial wins over allow when both name a tool. A global deny wins over a session
allow. A deny rule is not a suggestion: the engine does not execute the call.

## Non-interactive clients

With no terminal, a parked approval stays parked. A JSONL script or a future TUI
should read `ApprovalRequested` and submit `Approve` or `Deny` through the
protocol. Guessing a decision on the user's behalf is deliberately not the
default.

## One-shot grants

One-shot approvals are Norte grants. There is no command to revoke a one-shot
grant; session and global allow rules can be changed with `/allow` and `/deny`.
