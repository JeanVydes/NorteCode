# Set a budget

A budget caps spend in one of three units and stops or warns when the cap is
hit. Cost is always provider-reported: Norte Code never derives a price, so
an unpriced request is counted as unpriced, not as free.

## One run

```sh
norte-code new --budget 20000 --budget-unit tokens --budget-action stop
```

The unit is `usd`, `tokens` or `requests`. The action is `warn`, `ask` or
`stop`. A one-off budget applies to the first message, not the whole session.

## A session

```json
{
  "budgets": {
    "session": { "unit": "usd", "limit": 2.0, "action": "stop" }
  }
}
```

The session budget survives turns and is replaced by `resume --budget`. A
model switch that would make the unit unmeasurable is refused.

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

After a stop, raise the budget and resume:

```sh
norte-code resume <key> --budget 5 --budget-unit usd
```

## Unpriced requests

A USD budget only sees reported cost. If a provider reports no cost, the
request counts as unpriced and neither satisfies nor consumes the USD limit.
Setting or resuming a USD budget is refused when the session or model
already has an unpriced request — use a token or request budget instead.

## Where to see the numbers

```sh
norte-code cost
norte-code cost --session <key>
norte-code telemetry --bucket day
```

`cost` is the money ledger; `telemetry` is the request log (latency, tokens,
cache, tools, status). A request can appear in telemetry with no cost.
