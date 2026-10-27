# Script against the protocol

## One event per frame

The `new` and `resume` commands read the terminal, so they carry no event stream.
A script drives the same protocol through the `Service` API, and this is the shape
of what it reads: one serialized `ServerEvent` per frame, stable enough to parse
because request and event frames use tagged serde JSON and deny unknown fields.

A successful turn contains frames like:

```json
{"request_id":1,"session":"…","kind":{"event":"turn_started","turn":1}}
{"request_id":1,"session":"…","kind":{"event":"delta","turn":1,"delta":{"delta":"text","index":0,"text":"…"}}}
{"request_id":1,"session":"…","kind":{"event":"request_reconstructed","turn":1,"index":1,"usage":{…},"cost_usd":0.001,"truncated":false,"duration_ms":1200,"tools":[]}}
{"request_id":1,"session":"…","kind":{"event":"committed","turn":1,"version":3}}
{"request_id":1,"session":"…","kind":{"event":"turn_finished","turn":1,"phase":"idle","committed":true}}
```

## Read the terminal state

The useful terminal frames are:

| Event | Meaning |
| --- | --- |
| `turn_finished` | The turn ended. `committed` says whether an assistant message landed. |
| `error` | The request failed terminally. |
| `approval_requested` | The turn is parked until a client decides. |
| `question_asked` | The turn is parked until a client answers. |

A script that only reads stdout cannot approve a tool. If stdin is not a
terminal, the CLI leaves the session parked rather than guessing. Use the
`Service` API directly for a fully programmatic client.

## Keep the session

The `session` field is the logical key. A client stores it and passes it back on
the next request, or a person continues it with `norte-code resume <key>`. The
key is shown in the footer while the session runs and listed by
`norte-code sessions --json`; do not parse human output for it.

## Errors and exit codes

A terminal `error` frame is a failed turn, not a successful one. A
command-line or configuration error exits `2` and writes a diagnostic to stderr.

## What not to do

- Do not assume every line is a turn event; `session_created`, `model_list` and
  `cost_updated` can appear.
- Do not assume `delta` text is the whole answer; concatenate text deltas in
  order.
- Do not treat an unpriced request as free. `cost_usd` is absent or `null` when
  the provider reported no cost.
