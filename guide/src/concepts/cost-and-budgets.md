# Cost and budgets

## Provider-reported cost only

Norte Code does not compute a price from a catalogue list price. The cost ledger
stores the amount a provider reported for a request, or `NULL` when it reported
none. The catalogue's `cost.*` fields are ignored.

That rule has consequences:

- A free route can report zero, and zero is different from "unknown".
- An unpriced request is visible in `norte-code cost` and in the budget's
  "unpriced" count.
- A USD budget cannot be enforced over an unpriced request; the host skips that
  request rather than pretending it cost nothing. Setting or resuming a USD
  budget after the session or model has an unpriced request is refused, with a
  suggestion to use a token or request budget instead.

The ledger is one row per billed request with the model, turn, request index,
token counters, reported cost and timestamp.

## Scopes and units

Budgets have three scopes:

| Scope | Lifetime |
| --- | --- |
| `message` | One user message / turn. |
| `session` | The logical session, across turns. |
| `global_daily` | The UTC day, across sessions. |

Each budget measures one unit: `usd`, `tokens` or `requests`. The unit must be
measurable for the session's model; a model switch that would make a session
budget unmeasurable is refused.

## Actions

| Action | What happens |
| --- | --- |
| `warn` | Emit `BudgetWarning` and continue. |
| `stop` | Emit `BudgetExceeded` and end the turn uncommitted. The session is resumable. |
| `ask` | Currently a resumable stop with a notice, not an approval prompt. Raise the budget and resume to continue. |

`warn` can fire repeatedly as the total grows. `stop` and `ask` do not silently
truncate history: the turn ends at the safe point and the session stays intact.

## Config and flags

```json
{
  "budgets": {
    "message": {"unit": "tokens", "limit": 50000, "action": "warn"},
    "session": {"unit": "usd", "limit": 2.0, "action": "stop"},
    "global_daily": {"unit": "requests", "limit": 200, "action": "stop"}
  }
}
```

`run` and `resume` accept a one-off budget:

```sh
norte-code new --budget 20000 --budget-unit tokens --budget-action stop
```

`resume --budget` replaces the session budget before continuing, which is how a
stopped session is allowed to proceed.

## Reporting

```sh
norte-code cost
norte-code cost --session <key>
norte-code cost --daily
norte-code cost --json
```

`cost` is the money ledger. `telemetry` is the request log: latency, tokens,
cache, tools and status. They answer different questions and are stored in
different tables.
