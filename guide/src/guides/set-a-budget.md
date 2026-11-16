# Set a budget

A budget caps spend in one of three units and stops or warns when the cap is
hit. Cost is always provider-reported: Norte Code never derives a price, so
an unpriced request is counted as unpriced, not as free.

## A session

```json
{
  "budgets": {
    "session": { "unit": "usd", "limit": 2.0, "action": "stop" }
  }
}
```

The session budget survives turns. A model switch that would make the unit unmeasurable is refused.

## A daily limit

```json
{
  "budgets": {
    "global_daily": { "unit": "requests", "limit": 200, "action": "stop" }
  }
}
```

The daily scope is UTC, across all sessions in the local database. There is
also a `message` scope for the current turn.

## What the actions mean

- `warn` — report and keep going.
- `stop` — end the turn uncommitted. The session is resumable.
- `ask` — end the turn like `stop`, but the interactive CLI asks for a new
  limit first. Type one to raise the budget and resume the same turn; leave
  it empty to leave the session stopped.

## Unpriced requests

A USD budget only sees reported cost. If a provider reports no cost, the
request counts as unpriced and neither satisfies nor consumes the USD limit.
Setting or resuming a USD budget is refused when the session or model
already has an unpriced request — use a token or request budget instead.


