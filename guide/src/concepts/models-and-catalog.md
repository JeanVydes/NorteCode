# Models and the catalogue

## `provider/model`

A model reference is a provider id and a model id separated by the first slash:

```text
openrouter/anthropic/claude-sonnet-4
└────────┘ └───────────────────────────┘
provider    model id
```

The model id may contain slashes. The provider id names an entry of
`providers.json`, which supplies the wire, the base URL and the variables a key
may come from.

Two providers may serve a model with the same id, and they are two models: the
pair is what identifies one.

## Where models come from

Two files beside your configuration, and nothing else:

| File | What it says |
| --- | --- |
| `providers.json` | Who serves models, over which wires, and where each one lives. |
| `models.json` | Every model, in a flat list, each naming its `provider_id`. |

They are split so a model can be added by appending to a list, without touching
a provider. Nothing is downloaded, cached or bundled: `norte-code` reads these
two files at startup, and a file that is missing is an error naming it.

They live in `$XDG_CONFIG_HOME/norte-code/` when that is set and
`~/.config/norte-code/` otherwise, beside `config.json`.

### `providers.json`

```json
{
  "providers": {
    "openai": {
      "name": "OpenAI",
      "doc": "https://platform.openai.com/docs",
      "environment_keys": ["OPENAI_API_KEY"],
      "wires": [
        { "api": "https://api.openai.com/v1", "schema": "openai" }
      ]
    }
  }
}
```

`environment_keys` is a list, tried in order: a gateway often reads its own
variable and the one you already have exported.

### `models.json`

```json
{
  "models": [
    {
      "id": "gpt-5",
      "provider_id": "openai",
      "name": "GPT-5",
      "limit": { "context": 400000, "output": 128000 },
      "modalities": { "input": ["text", "image"], "output": ["text"] },
      "supported_params": ["tools", "temperature", "structured_output"],
      "reasoning": {
        "default_enabled": true,
        "default_effort": "medium",
        "supported_efforts": ["none", "low", "medium", "high", "xhigh"],
        "budget_supported": true
      }
    }
  ]
}
```

`supported_params` is the list of request fields the model accepts, and it is a
closed set: `tools`, `temperature`, `structured_output`, `reasoning_effort`,
`logprobs`, `seed`, `stop` and the rest. A name that is not in it is refused
where the file is read, with the line and every name that would have worked — a
typo cannot pass as "the model does not support that".

`reasoning.mandatory` says the model always reasons. It cannot be turned off,
which is a fact about the model and not a default you may change.

`pricing` may be carried for reference. It is never a source of cost: Norte Code
reports what the provider said a request cost.

## Wires

A provider has `wires` and not one API: each is a base URL plus the **schema** it
speaks. The three schemas are the ones the ecosystem converges on:

| Schema | What it is |
| --- | --- |
| `openai` | Chat completions, the shape every compatible vendor speaks. |
| `anthropic` | Anthropic Messages. |
| `google` | Google `generateContent`. |

A session reaches a provider over its **preferred** wire when nobody says which,
and the preference is OpenAI, then Anthropic, then Google.

`--wire` names one:

```sh
norte-code new --provider openai --model gpt-5 --wire openai
```

A schema the provider does not serve is an error listing the ones it does.

The credential header follows from the schema — `Bearer` for OpenAI, `x-api-key`
for Anthropic, `x-goog-api-key` for Google — and a wire may name its own `auth`
when a gateway serves another vendor's shape over its own credential. It is
optional: leaving it out follows the schema. The configuration file cannot set
it, because a value there could only make the request wrong; the wire, which is
what the provider owns, is the one place that may.

The Responses API exists in Norte and is deliberately not reachable here: it is
another shape entirely, and this host speaks the one the three schemas share.

## Listing what you have

```sh
norte-code models
norte-code models --provider openai --tools --reasoning
norte-code models --json
```
