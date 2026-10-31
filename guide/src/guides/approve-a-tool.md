# Approve a tool

## What a rule decides

A rule names a tool and says whether calls to it run. Denial wins over allow,
and a global deny wins over a session allow. A rule that names a tool nobody has
does nothing, and says nothing about having done it.

The names are the ones the model calls the tools by, and they are what
`/tools` prints.

## Rules without a pending call

In the interactive prompt:

```text
/allow shell session
/allow edit global
/deny shell global
```

`/tools` lists every tool with the state of its rule — `allow`, `deny` or `ask`
— and whether the rule is this session's or every session's. It is the fastest
way to check that a rule you wrote names a tool that exists.

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
allow. A deny rule is not a suggestion: the engine does not execute the call,
and the model reads the rule's reason like a failed call and adapts.

## What is gone

The old approval panel — a parked turn that waited on a human to press `y`,
`s`, `a`, `n`, `ns` or `ng` — is removed. There is no parked state and nothing
waits on stdin. A rule decides before anything runs; a denied call is answered
immediately, and the turn carries on.

`/revoke` is gone with the grants it listed: a deny is revoked by writing an
allow rule for the same tool.

## Questions

`ask_user` is a tool, but its result is a human answer rather than a command
decision. The model may offer an `options` list and set `multiple: true`; the
terminal appends an "answer something else" entry so free-form input is always
possible. The answer resumes the parked turn through `AnswerQuestion`.
