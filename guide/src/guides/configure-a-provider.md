# Configure a provider

There is no per-provider configuration. A provider is an entry in
`providers.json`, a model is an entry in `models.json`, and the config only
says which pair a new session opens on:

```json
{
  "version": 1,
  "default_provider": "openrouter",
  "default_model": "anthropic/claude-sonnet-4"
}
```

A provider and a model are two values, never one string: a model id may carry
slashes of its own, so `provider/model` is a display label only.

## The two catalogue files

Both live beside `config.json` and are yours to edit. Nothing is fetched or
bundled: a missing file is written empty and the next error names it.

`providers.json` — who serves models, over which wires:

```json
{
  "providers": {
    "openai": {
      "name": "OpenAI",
      "environment_keys": ["OPENAI_API_KEY"],
      "wires": [{ "api": "https://api.openai.com/v1", "schema": "openai" }]
    }
  }
}
```

`models.json` — every model, each naming its provider:

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

The three wire schemas are `openai` (chat completions, the shape every
compatible vendor speaks), `anthropic` and `google`. `--wire` picks one;
the preferred order is OpenAI, then Anthropic, then Google. The credential
header follows from the schema and from nothing else.

## The key

A key comes from the provider's `environment_keys` (tried in order, first
set and non-blank wins), then from `secrets.json` beside the config
(mode 0600, keyed by provider id). A provider that names none needs no key
— that is what a local endpoint uses.

If the key is still missing when you pick a model in `/model` or `/models`,
a masked prompt asks for it and saves it. There is no plaintext key field
in `config.json`: the environment and the 0600 file are the sources.

## Common providers

OpenAI, Anthropic and Google are the three native wires; OpenRouter is
OpenAI-compatible chat at `https://openrouter.ai/api/v1`. Upstream routing
on OpenRouter is a vendor extension:

```sh
norte-code new --extension 'openai.chat:provider.order=["deepinfra"]'
```

A local endpoint needs no key at all — just a provider entry with a wire
pointing at `http://127.0.0.1:8080/v1` and the model in `models.json`.

## Why did it fail?

- **No model selected** — pass `--model` or set `default_model`.
- **Unknown provider/model** — not in `providers.json` / `models.json`. The
  error lists what you have.
- **Missing key** — the error names every variable it tried.
- **Wrong wire** — `--debug` prints the effective model and wire, never the
  base URL or the key.
- **A typo in a file** — the error names the file, the line and every value
  that would have worked.

Never put keys in the repository. Keys are redacted from command output
and never reach a log or a model event.
