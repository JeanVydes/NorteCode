# Script with JSONL

The CLI can run without a terminal. Send one message per input line and read
one JSON object per output line:

```sh
printf '%s\n' 'Summarize this repository' | norte-code new --jsonl
```

Server events retain their protocol shape. Client-side diagnostics use
`"event":"client_diagnostic"` and identify connection, session creation,
input, request start/failure/finish, and client completion. This makes a
failed headless run distinguishable from an empty response.

## A successful turn

```json
{"request_id":1,"session":"…","kind":{"event":"turn_started","turn":1}}
{"request_id":1,"session":"…","kind":{"event":"delta","turn":1,"delta":{"delta":"text","index":0,"text":"…"}}}
{"request_id":1,"session":"…","kind":{"event":"request_reconstructed","turn":1,"index":1,"usage":{…},"cost_usd":0.001,"truncated":false,"duration_ms":1200,"tools":[]}}
{"request_id":1,"session":"…","kind":{"event":"committed","turn":1,"version":3}}
{"request_id":1,"session":"…","kind":{"event":"turn_finished","turn":1,"phase":"idle","committed":true}}
```

## Service events

| Event | Meaning |
| --- | --- |
| `turn_finished` | The turn ended. `committed` says whether an assistant message landed. |
| `error` | The request failed terminally. |
| `question_asked` | The turn is parked until a client answers. |

The CLI connects to the service configured in `cli.json`; it does not create
an in-process service. For a custom transport client, use the RAEP endpoint
described in the protocol reference.

## Keep the session

The `session` field is the logical key. Store it and pass it back on the
next request, or a person continues it with `norte-code resume <key>`. It is
shown in the footer and listed by `norte-code sessions --json`; do not parse
human output for it.

## Errors and exit codes

A terminal `error` frame is a failed turn. A command-line or configuration
error exits `2` with a diagnostic on stderr.

## What not to assume

- Not every line is a turn event: `session_created`, `model_list` and
  `cost_updated` can appear.
- `delta` text is not the whole answer: concatenate text deltas in order.
- An unpriced request is not free: `cost_usd` is absent or `null` when the
  provider reported no cost.
