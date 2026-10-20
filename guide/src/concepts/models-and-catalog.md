# Models and the catalogue

## `provider/model`

A model reference is a provider id and a model id separated by the first slash:

```text
openrouter/anthropic/claude-sonnet-4
└────────┘ └───────────────────────────┘
provider    model id
```

The model id may contain slashes. The provider id selects the catalogue entry,
key source, endpoint and wire.

## Where models come from

The catalogue is models.dev. Norte Code downloads it, caches it on disk with an
ETag and a TTL, and falls back to the stale cache or the bundled snapshot when
the network is unavailable. `catalog.offline` and `--offline` forbid the network
entirely; `--refresh` forces a revalidation.

The catalogue maps a model to Norte capabilities: input/output modalities,
context and output limits, tool support, reasoning controls, streaming
granularity, cache mode, structured output and feature flags. The host does not
trust every numeric field blindly; implausible limits are dropped rather than
turned into a bad request.

`norte-code models` is the quickest way to see the flattened result:

```sh
norte-code models
norte-code models --provider openrouter --tools --reasoning
norte-code models --json
```

## Providers and wires

The provider entry's catalogue `npm` field selects the wire family. The `api`
field supplies the endpoint, and `env` names the key. A configured endpoint
overrides both. Native OpenAI, Anthropic and Google requests use Norte's presets
when no endpoint is configured; OpenRouter and other OpenAI-compatible services
use the chat wire.

A provider config can override the key source and endpoint:

```json
{
  "providers": {
    "openrouter": {
      "api_key_env": "OPENROUTER_API_KEY",
      "endpoint": "https://openrouter.ai/api/v1"
    }
  }
}
```

Key resolution is `api_key_env`, then `api_key_file`, then the catalogue's `env`
names. Keys are held in Norte's redacted secret type and never enter a model
event, a log or a child process.

## Reasoning

`--reasoning` accepts `auto`, `off`, `minimal`, `low`, `medium`, `high` or a
budget object in config. The setting is checked against the model before the
turn starts. With `auto`, the implicit default asks for `medium` when the model
declares it and otherwise uses the closest lower effort; a model with no declared
efforts keeps `auto`.

`ReasoningDowngraded` is emitted when a model switch forces a lower setting than
the one the session requested.

## Model overrides

The catalogue is not always right or complete. `model_overrides` can replace
`max_context`, `max_output` and the reasoning controls for one
`provider/model`:

```json
{
  "model_overrides": {
    "openrouter/anthropic/claude-sonnet-4": {
      "max_context": 200000,
      "max_output": 64000,
      "reasoning": {
        "supported": true,
        "can_disable": true,
        "efforts": ["low", "medium", "high"]
      }
    }
  }
}
```

Overrides win over catalogue values for that model.

## Switching models

`SetModel` (and the interactive model picker) switches at a turn boundary. The
logical session, its history, its approvals and its cost totals stay; a new
engine is built for the new model over the same stores. A switch that would make
a session budget unmeasurable is refused before it happens.

## What is not here

There is no local price table. A model's catalogue `cost.*` fields are ignored.
A provider that reports a cost in its usage payload is recorded; a provider that
does not leaves the request unpriced.
