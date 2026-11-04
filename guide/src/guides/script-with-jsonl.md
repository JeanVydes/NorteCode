# Script against the protocol

`new` and `resume` keep the terminal, so they carry no event stream. A
script speaks the protocol instead: one serialized `ServerEvent` per frame,
stable to parse because frames are tagged serde JSON.

## A successful turn

```json
{"request_id":1,"session":"…","kind":{"event":"turn_started","turn":1}}
{"request_id":1,"session":"…","kind":{"event":"delta","turn":1,"delta":{"delta":"text","index":0,"text":"…"}}}
{"request_id":1,"session":"…","kind":{"event":"request_reconstructed","turn":1,"index":1,"usage":{…},"cost_usd":0.001,"truncated":false,"duration_ms":1200,"tools":[]}}
{"request_id":1,"session":"…","kind":{"event":"committed","turn":1,"version":3}}
{"request_id":1,"session":"…","kind":{"event":"turn_finished","turn":1,"phase":"idle","committed":true}}
```

## Terminal frames

| Event | Meaning |
| --- | --- |
| `turn_finished` | The turn ended. `committed` says whether an assistant message landed. |
| `error` | The request failed terminally. |
| `question_asked` | The turn is parked until a client answers. |

If stdin is not a terminal, the CLI leaves the session parked rather than
guessing. For a fully programmatic client, use the `Service` API directly
(`norte_code_agent::Service` + `ClientRequest`) — full API in rustdoc.

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
