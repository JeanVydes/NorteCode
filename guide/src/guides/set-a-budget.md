# Set a budget

## One run

```sh
norte-code run \
  --budget 20000 \
  --budget-unit tokens \
  --budget-action stop \
  "refactor the parser"
```

The unit is `usd`, `tokens` or `requests`. The action is `warn`, `ask` or `stop`.
A one-off budget applies to the message, not the whole session.

## A session

```json
{
  "budgets": {
    "session": {
      "unit": "usd",
      "limit": 2.0,
      "action": "stop"
    }
  }
}
```

The session budget survives turns and is replaced by `resume --budget`. A model
switch that would make the unit unmeasurable is refused.

## A daily limit

```json
{
  "budgets": {
    "global_daily": {
      "unit": "requests",
      "limit": 200,
      "action": "stop"
    }
  }
}
```

The daily scope is UTC, across all sessions in the local database.

## What the actions mean

- `warn` emits `BudgetWarning` and keeps going.
- `stop` emits `BudgetExceeded` and ends the turn uncommitted. The session is
  resumable.
- `ask` is currently a resumable stop with a notice, not an approval prompt.

After a stop, raise the budget and resume:

```sh
norte-code resume <key> --budget 5 --budget-unit usd
```

## Unpriced requests

A USD budget only sees reported cost. If a provider reports no cost for a
request, the request is counted as unpriced and does not satisfy or consume the
USD limit. The service does not invent a price from the catalogue, and it refuses
to set or resume a USD budget when the session or model already has an unpriced
request. Token and request budgets still work because those counters are
provider-reported too.

## Where to see the numbers

```sh
norte-code cost
norte-code cost --session <key>
norte-code telemetry --bucket day
```

`cost` is the money ledger. `telemetry` is the request log: latency, tokens,
cache, tools and status. A request can appear in telemetry with a `NULL` cost.
