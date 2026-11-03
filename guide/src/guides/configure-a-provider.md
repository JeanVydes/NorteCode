# Configure a provider

There is no per-provider configuration any more. A provider is an entry in
`providers.json`, a model is an entry in `models.json`, and the configuration
names only which pair a new session opens on:

```json
{
  "version": 1,
  "default_provider": "openrouter",
  "default_model": "anthropic/claude-sonnet-4"
}
```

## The key

A key comes from the environment, the OS keyring, or a private file:

```json
{
  "providers": {
    "ollamacloud": {
      "name": "Ollama Cloud",
      "environment_keys": ["OLLAMACLOUD_API_KEY"],
      "wires": [
        { "api": "https://ollama.com/v1", "schema": "openai" }
      ]
    }
  }
}
```

`environment_keys` is tried in order, and the first one that is set and not empty
wins. It is a list because a gateway often reads its own variable and the one you
already have exported.

If no variable holds a key, the client reads the OS keyring — service
`norte-code`, account the provider id, so `ollamacloud` above — and then
`secrets.json` beside the configuration, written with mode 0600:

```json
{ "ollamacloud": "sk-..." }
```

The account is always the provider id, never the variable name.

A provider that names none needs no key, which is what a local endpoint is.

Selecting a model from `/model` or `/models` when the key is still missing opens
a masked prompt. What you type goes to the keyring, or to `secrets.json` when no
keyring is available, and the switch continues. Escape cancels the prompt and
leaves the current model in place. A provider whose sources are all empty is an
error naming the variables tried:

```text
could not resolve the API key for openai: no value in any of: OPENAI_API_KEY
```

There is no `api_key_file`, no `--api-key-env`, and no plaintext key field in
`config.json`: the environment, the keyring and the 0600 file are the sources, and
the prompt exists so there is nothing to keep in step with them.

## Three native providers

```json
{
  "providers": {
    "openai": {
      "name": "OpenAI",
      "environment_keys": ["OPENAI_API_KEY"],
      "wires": [{ "api": "https://api.openai.com/v1", "schema": "openai" }]
    },
    "anthropic": {
      "name": "Anthropic",
      "environment_keys": ["ANTHROPIC_API_KEY"],
      "wires": [{ "api": "https://api.anthropic.com/v1", "schema": "anthropic" }]
    },
    "google": {
      "name": "Google",
      "environment_keys": ["GEMINI_API_KEY", "GOOGLE_API_KEY"],
      "wires": [
        {
          "api": "https://generativelanguage.googleapis.com/v1beta",
          "schema": "google"
        }
      ]
    }
  }
}
```

The variable list is yours to write, and it is the only place a name is looked
up: the host does not guess at `GOOGLE_API_KEY` for you.

## OpenRouter

OpenRouter speaks the OpenAI chat schema, and its base URL is its own:

```json
{
  "providers": {
    "openrouter": {
      "name": "OpenRouter",
      "environment_keys": ["OPENROUTER_API_KEY"],
      "wires": [
        { "api": "https://openrouter.ai/api/v1", "schema": "openai" }
      ]
    }
  }
}
```

Upstream routing is a vendor extension, not a Norte Code setting:

```sh
norte-code new --extension 'openai.chat:provider.order=["deepinfra"]'
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

### A provider that serves more than one wire

A gateway may serve the same models over more than one API surface. Each is an
entry in `wires`, and `--wire` picks one:

```json
{
  "providers": {
    "openrouter": {
      "environment_keys": ["OPENROUTER_API_KEY"],
      "wires": [
        { "api": "https://openrouter.ai/api/v1", "schema": "openai" },
        { "api": "https://openrouter.ai/api/v1", "schema": "anthropic" }
      ]
    }
  }
}
```

```sh
OPENROUTER_API_KEY=... norte-code new \
  --model openrouter/anthropic/claude-sonnet-4.5 \
  --wire anthropic
```

With nothing named, the preferred wire is used: OpenAI, then Anthropic, then
Google. A schema the provider does not serve is an error listing the ones it
does.

The credential header follows from the schema — `Bearer` for OpenAI,
`x-api-key` for Anthropic, `x-goog-api-key` for Google. A gateway that serves
another vendor's *shape* over its own credential names one on the wire, because
the schema says how the body is shaped and not who is paying:

```json
{
  "providers": {
    "gateway": {
      "environment_keys": ["GATEWAY_API_KEY"],
      "wires": [
        { "api": "https://gateway.invalid/v1", "schema": "anthropic", "auth": "bearer" }
      ]
    }
  }
}
```

`auth` is one of `bearer`, `x_api_key`, `x_goog_api_key` or `disabled`, and it
is optional: leaving it out follows the schema. `disabled` sends no credential,
which is what a local endpoint wants when it has an `environment_keys` list for
another reason. There is still no `auth` in the configuration file — the wire is
the only place that may say it.

## A local endpoint

```json
{
  "providers": {
    "local": {
      "name": "A local server",
      "wires": [{ "api": "http://127.0.0.1:8080/v1", "schema": "openai" }]
    }
  }
}
```

No `environment_keys` means no credential is sent, which is what a local server
usually expects.

The model still has to be in `models.json` — a provider with no models has
nothing to open a session on:

```json
{
  "models": [
    {
      "id": "my-model",
      "provider_id": "local",
      "limit": { "context": 32768, "output": 4096 },
      "modalities": { "input": ["text"], "output": ["text"] },
      "supported_params": ["tools"]
    }
  ]
}
```

## Why did it fail?

- **No model selected**: pass `--model` or set `default_model` (with
  `default_provider`).
- **Unknown provider**: it is not in `providers.json`. The error lists the ones
  you have.
- **Unknown model**: the provider has no such entry in `models.json`.
- **Missing key**: the error names every variable it tried.
- **Wrong wire**: `--debug` prints the effective model and wire, but never the
  base URL or the key.
- **A typo in a file**: the error names the file, the line and every value that
  would have been accepted.

## Do not put keys in the repository

The schema has no plaintext key field. Prefer an environment variable. Keys are
redacted from command output and never reach a log or a model event.
