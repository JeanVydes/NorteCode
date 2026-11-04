# Approve a tool

Rules decide whether a tool call runs, before anything happens. Denial
wins over allow, and a global deny wins over a session allow. A rule naming
a tool that does not exist does nothing — and does not say so.

## Write rules at the prompt

```text
/allow shell session
/allow edit global
/deny shell global
```

`/tools` lists every tool with its rule state (`allow`, `deny` or `ask`) and
whether the rule is this session's or global. It is the fastest way to check
a rule names a real tool.

The scope defaults to `session`. Global rules go to the user config's
`approvals.allow` / `approvals.deny`; session rules live with the session.
An allow replaces a deny for the same tool — that is how you revoke one.

Or write them directly:

```json
{
  "approvals": {
    "allow": ["read", "grep"],
    "deny": ["shell"]
  }
}
```

## What a denied call looks like

The call is refused before it runs. The model reads the rule's reason like
a failed call and adapts; nothing waits on stdin and the turn carries on.
The refusal still appears in the transcript, so a replay shows what the
model saw.

## Questions

`ask_user` parks the turn until a human answers. The model may offer an
`options` list and allow several answers; the terminal always appends an
"answer something else" row so free-form input is possible.
