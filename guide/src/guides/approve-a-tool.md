# Approve a tool

## What parks

In `ask` (the default), `write`, `edit`, `shell` and `skill_script` park before
running. In `auto-edit`, writes and edits run; commands and skill scripts still
park. `yolo` runs everything. A global or session deny wins over the mode.

The names are the ones the model calls the tools by, and they are what
`/tools` prints: a rule that names a tool nobody has does nothing, and says
nothing about having done it.

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
/allow shell session
/allow edit global
/deny shell global
```

`/tools` lists every tool with the state of its rule — `allow`, `deny` or `ask`
— and whether the rule is this session's, every session's, or the approval
mode's. It is the fastest way to check that a rule you wrote names a tool that
exists.

The scope defaults to `session`. Global rules are written to the user config's
`approvals.allow` / `approvals.deny`; session rules are stored with the logical
session. An allow replaces an existing deny for the same tool.

The same rules can be written directly:

```json
{
  "approvals": {
    "allow": ["read", "grep"],
    "deny": ["shell"]
  }
}
```

## Deny is sticky

Denial wins over allow when both name a tool. A global deny wins over a session
allow. A deny rule is not a suggestion: the engine does not execute the call.

## Non-interactive clients

With no terminal, a parked approval stays parked. A JSONL script
should read `ApprovalRequested` and submit `Approve` or `Deny` through the
protocol. Guessing a decision on the user's behalf is deliberately not the
default.

## One-shot grants

One-shot approvals are Norte grants. `/revoke` lists the grants a session holds
and revokes one of them:

```text
/revoke
/revoke <grant-id>
/revoke <tool>
```

Without an argument the command lists every grant with its id, its tool, its
scope and whether it is still active. With an argument it revokes the grant
whose id matches, or the only active grant for that tool. Session and global
allow rules can be changed with `/allow` and `/deny`.

The protocol exposes the same list through `ListGrants` and `GrantList`, so a
client that is not the CLI can find the ids too.
