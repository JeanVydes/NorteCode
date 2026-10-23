# CLI

The binary is `norte-code`. It is a client of the in-process `Service`; it does
not start a daemon.

## Global options

| Option | Meaning |
| --- | --- |
| `--config PATH` | Use this user config file instead of the platform path. |
| `--offline` | Never touch the network for the catalogue. |
| `--refresh` | Revalidate the catalogue even when the cache is fresh. |
| `--verbose` | Show tool failures and more detail. |
| `--debug` | Print startup details, model metadata, paths and extensions. |

## `run`

```text
norte-code run [OPTIONS] [PROMPT]
```

`PROMPT` is optional only with `--interactive`.

| Option | Meaning |
| --- | --- |
| `-m, --model PROVIDER/MODEL` | Override the default model. |
| `--system-prompt TEXT` | Replace the base system prompt for this run. |
| `--endpoint URL` | Override the provider endpoint. |
| `--wire WIRE` | Override the provider wire: `responses`, `chat`, `messages` or `generate-content`. |
| `--auth AUTH` | Override the credential scheme: `bearer`, `x-api-key` or `x-goog-api-key`. |
| `--budget LIMIT` | A limit for this message. |
| `--budget-unit usd\|tokens\|requests` | What the limit measures. Default `usd`. |
| `--budget-action warn\|ask\|stop` | What to do at the limit. Default `stop` for `--budget`. |
| `--compaction-threshold FRACTION` | Override the compaction threshold in `(0, 1]`. |
| `--sandbox` / `--no-sandbox` | Override `tools.sandbox.enabled`. |
| `--sandbox-network on\|off` | Override sandbox networking; requires the sandbox. |
| `--reasoning SETTING` | `auto`, `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` or `budget:<tokens>`. |
| `--approval-mode ask\|auto-edit\|yolo` | Override the approval mode. |
| `--max-iterations N` | Cap model iterations for this run. |
| `--extension KEY=JSON` | Add a vendor request field; repeatable. |
| `--jsonl` | One protocol event per line on stdout. |
| `--quiet` | Redact reasoning and hide tool activity and usage. The `command` tool still prints the command it ran. |
| `--session KEY` | Continue an existing logical session. |
| `-i, --interactive` | Keep the session open after each turn. |

## `resume`

```text
norte-code resume [OPTIONS] <SESSION>
```

Continues a parked, stopped or interrupted session.

| Option | Meaning |
| --- | --- |
| `--system-prompt TEXT` | Replace the base prompt for this resume. |
| `--budget LIMIT` | Replace the session budget before resuming. |
| `--budget-unit`, `--budget-action` | Unit and action for that budget. |
| `--compaction-threshold FRACTION` | Override the threshold for this resume. |
| `--sandbox`, `--no-sandbox`, `--sandbox-network` | Sandbox overrides. |
| `--max-iterations N` | Keep the config limit when omitted. |
| `--extension KEY=JSON` | Vendor request field; repeatable. |
| `--jsonl`, `--quiet`, `-i, --interactive` | Output and interaction modes. |

## `models`

```text
norte-code models [OPTIONS]
```

| Option | Meaning |
| --- | --- |
| `--provider ID` | Only this provider. |
| `--tools` | Only models that call tools. |
| `--reasoning` | Only models that reason. |
| `--json` | One JSON object per line. |

## `skills`

```text
norte-code skills list [--json]
norte-code skills check
```

`list` prints the valid skills; `check` validates every skill and reports the
ones that were skipped and why.

## `sessions`

```text
norte-code sessions [--json]
```

Lists the logical sessions in the local database, including the key, title,
model, creation time and phase.

## `cost`

```text
norte-code cost [--session KEY] [--daily] [--json]
```

Reports the provider-reported cost ledger. `--session` adds the session's turns;
`--daily` groups by UTC day.

## `telemetry`

```text
norte-code telemetry [OPTIONS]
```

| Option | Meaning |
| --- | --- |
| `--session KEY` | Only this logical session. |
| `--provider ID` | Only this provider. |
| `--model ID` | Only this model. |
| `--bucket hour\|day\|week\|month\|year\|total` | The time bucket. Default `day`. |
| `--json` | One JSON object. |

Telemetry is local request data: counts, tokens, cache, tools, latency and
status. It is not the money ledger. The report is a table: a bar shows how
many tokens each group moved against the busiest one, coloured by the cache —
green for the prompt the provider served from its cache, red for the part it
had to read — and the columns carry the requests, the tokens in and out, the
reasoning tokens, the cache hit rate weighted by tokens, the time to the first
token, the latency and the cost. The groups are ordered by volume, and the
time groups are named after `--bucket` (`by day`, `by week`, `by hour`, …). A
session that moved less than one percent of the total is left out of the
session table, and the report says how many. `--json` carries the same
numbers.

## Chat commands

`norte-code run --interactive` and `norte-code resume --interactive` read
commands from the same line editor. A command is recognised as soon as it
starts with `/`; anything else is a message. Commands that take arguments
reject unknown values instead of forwarding them to the model.

| Command | Meaning |
| --- | --- |
| `/help` | Show the command list. |
| `/compact` | Summarise the session and continue on a new one. |
| `/allow TOOL [session\|global]` | Allow a tool without asking. |
| `/deny TOOL [session\|global]` | Refuse a tool without asking. |
| `/reasoning SETTING [session\|message]` | Set the reasoning override. `SETTING` is `auto`, `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` or `budget:<tokens>`. The scope defaults to `session`. |
| `/model PROVIDER/MODEL` | Switch the model at the next turn boundary. |
| `/revoke [GRANT-OR-TOOL]` | List the approval grants, or revoke one. |
| `/refresh` | Re-scan the skills and their tools. |
| `/exit`, `/quit` | Leave the chat. |

The editor suggests the command names and their arguments in dimmed text as
you type, and Tab accepts the suggestion.

While a turn is running, the commands that make sense there are handled
immediately (`/allow`, `/deny`, `/reasoning`, `/model`, `/revoke`, `/refresh`,
`/help`). `/compact` and `/exit` wait for the turn to finish. Any other text is
steered into that turn: it reaches the request the turn is about to make, and
it stays durable until the engine reports it applied.

## Exit status

`0` means success. A failed turn exits with `1`. `skills check` also exits with
`1` when it finds warnings. A usage or configuration error exits with `2` and a
message on stderr.
