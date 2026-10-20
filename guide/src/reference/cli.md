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
| `--budget LIMIT` | A limit for this message. |
| `--budget-unit usd\|tokens\|requests` | What the limit measures. Default `usd`. |
| `--budget-action warn\|ask\|stop` | What to do at the limit. Default `stop` for `--budget`. |
| `--compaction-threshold FRACTION` | Override the compaction threshold in `(0, 1]`. |
| `--sandbox` / `--no-sandbox` | Override `tools.sandbox.enabled`. |
| `--sandbox-network on\|off` | Override sandbox networking; requires the sandbox. |
| `--reasoning SETTING` | `auto`, `off`, `minimal`, `low`, `medium`, `high`. |
| `--approval-mode ask\|auto-edit\|yolo` | Override the approval mode. |
| `--max-iterations N` | Cap model iterations for this run. |
| `--extension KEY=JSON` | Add a vendor request field; repeatable. |
| `--jsonl` | One protocol event per line on stdout. |
| `--quiet` | Redact reasoning and hide tool activity and usage. |
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
| `--bucket hour\|day\|month\|year\|total` | The time bucket. Default `day`. |
| `--json` | One JSON object. |

Telemetry is local request data: counts, tokens, cache, tools, latency and
status. It is not the money ledger.

## Exit status

`0` means success. A failed turn exits with `1`. `skills check` also exits with
`1` when it finds warnings. A usage or configuration error exits with `2` and a
message on stderr.
