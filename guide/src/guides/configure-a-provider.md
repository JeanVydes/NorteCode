# Configure a provider

## Name the model and the key

```json
{
  "version": 1,
  "default_model": "openrouter/anthropic/claude-sonnet-4",
  "providers": {
    "openrouter": {
      "api_key_env": "OPENROUTER_API_KEY"
    }
  }
}
```

Resolution order for a key:

1. `providers.<id>.api_key_env`
2. `providers.<id>.api_key_file`
3. The catalogue's `env` names for that provider

The catalogue entry also supplies the wire and the base endpoint. A configured
`endpoint` overrides the endpoint.

## Native providers

When no endpoint is configured, OpenAI, Anthropic and Google use Norte's native
presets:

```json
{
  "providers": {
    "openai": {"api_key_env": "OPENAI_API_KEY"},
    "anthropic": {"api_key_env": "ANTHROPIC_API_KEY"},
    "google": {"api_key_env": "GEMINI_API_KEY"}
  }
}
```

The exact provider id is the one in the catalogue. For Google, the catalogue may
also accept `GOOGLE_API_KEY`; `api_key_env` wins.

## OpenRouter

OpenRouter is an OpenAI-compatible chat endpoint:

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

Upstream routing is a vendor extension, not a Norte Code setting:

```sh
norte-code run \
  --extension 'openai.chat:provider.order=["deepinfra"]' \
  "review the patch"
```

The config form is:

```json
{
  "extensions": {
    "openai.chat:provider": {"order": ["deepinfra"]}
  }
}
```

A key whose wire family does not match the active model is ignored.

### Test the Messages wire

OpenRouter also serves an Anthropic-compatible endpoint at
`https://openrouter.ai/api/v1/messages`. Point a separate config at it:

```json
{
  "version": 1,
  "providers": {
    "openrouter": {
      "wire": "messages",
      "auth": "bearer"
    }
  }
}
```

```sh
OPENROUTER_API_KEY=... norte-code --config ./openrouter-messages.json run \
  --model openrouter/anthropic/claude-sonnet-4.5 \
  "Reply with exactly: ok"
```

The same override can stay out of the config for one run:

```sh
OPENROUTER_API_KEY=... norte-code run \
  --model openrouter/anthropic/claude-sonnet-4.5 \
  --wire messages --auth bearer \
  "Reply with exactly: ok"
```

The catalogue supplies the base URL (`https://openrouter.ai/api/v1`), and the
Messages adapter appends `messages`. If you pass `--endpoint`, pass that base,
not the full `/messages` URL. `auth` is needed because OpenRouter authenticates
gateways with `Authorization: Bearer`; the Messages wire defaults to `x-api-key`.
`wire` applies to the whole provider, so every OpenRouter model uses the
Messages wire while that file is active; remove the override to go back to the
chat wire. `--debug` prints the effective wire.

## Any OpenAI-compatible endpoint

Point the provider at the endpoint and keep the catalogue entry's model id:

```json
{
  "default_model": "local/my-model",
  "providers": {
    "local": {
      "endpoint": "http://127.0.0.1:8080/v1",
      "wire": "chat",
      "auth": "disabled"
    }
  }
}
```

`auth: "disabled"` sends no credential and needs no API key, which is what a
local server usually expects. A provider the catalogue does not list gets
conservative fallback capabilities; `model_overrides` can set its context and
output limits.

## Why did it fail?

- **No model selected**: pass `--model` or set `default_model`.
- **Unknown provider/model**: run `norte-code models` and use a listed pair.
- **Missing key**: check the environment variable named by `api_key_env`; a key
  file must exist and be readable.
- **Wrong endpoint**: `--debug` prints the effective model and wire, but never
  the endpoint URL or the key.
- **Catalogue offline**: `--offline` uses the cache or bundled snapshot; a
  provider absent from both cannot be resolved.

## Do not put keys in the repository

The config schema has no plaintext `api_key` field. Prefer an environment
variable or a file outside the repository. Keys are redacted from command output
and never reach a log or a model event.
