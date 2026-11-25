# Configuring the service and catalog

First inspect the existing JSON and `references/configuration.md` for every field, accepted type, defaults and nested limits. This guide adds relationships and corrections. Preserve entries and unknown fields; validate before saving. Do not claim that an option works solely because it parses.

## service.json

Top-level `version` is 1; `default_agent` names an entry in `agents`; `raep_limits` configures the host. Each `agents` entry is an object: `name` is its unique identifier; `description`, `origin_url`, and `author` are metadata. `default_provider` and `default_model` must be supplied together and refer to a provider id and model id in the catalog. Model ids can contain slashes: never split them to infer provider. `system_prompt` can replace the base prompt; workspace `AGENTS.md` is appended. `delegates` is the allowlist of agent names and `default_subagent_delegation` must be one of those names; an empty list disables delegation.

Per-agent sections are `compaction` (enabled, threshold, trigger, keep_recent), `retry` (max_attempts, max_delay_ms, backoff_ms), `provider_http` (connect_max_retries, connect_timeout_ms, request_timeout_ms, stream_idle_timeout_ms), `delegation` (max_depth, max_concurrent, max_held_messages), `mcp_limits.request_timeout_ms`, `reasoning.default`, `approvals.allow` and `approvals.deny`, `max_iterations`, `extensions`, `cost.show`, `budgets.message`, `budgets.session`, `budgets.global_daily`, `tools`, `plugins`, `skills` and `mcpServers`. Consult the tables in `configuration.md` for accepted values and constraints; in particular tool restrictions and budgets change runtime behavior. MCP entries live inside the target agent's `mcpServers`, not at service root. `tools.screenshots` is obsolete. `skills.honor_allowed_tools` is parsed but not enforced in the current implementation.

Example of minimal profile selection within an existing service object:

```json
{"name":"assistant","default_provider":"example","default_model":"vendor/model","delegates":[],"skills":{"enabled":true,"interop":true,"paths":[],"allow_scripts":false}}
```

Do not replace the full `agents` array with a partial example: project overlay arrays replace global arrays. Inspect defaults and all agent entries first.

## providers.json

`providers` is a map keyed by provider id. Each provider has `name` (display name), optional `doc` URL, `environment_keys` (ordered environment variable names, never secret values), and `wires` (available API surfaces). A wire requires `api` (base URL) and `schema`: `openai`, `responses`, `anthropic`, or `google`. Optional `auth` is `bearer`, `x_api_key`, `x_goog_api_key`, or `disabled`; absent uses the schema's default. Optional `headers` is a string-to-string map of additional static HTTP headers. Wire objects reject unknown fields. When multiple compatible wires exist, inspect the available command options and actual catalog; do not assume a provider's wire or credentials.

```json
{"providers":{"example":{"name":"Example","environment_keys":["EXAMPLE_API_KEY"],"wires":[{"api":"https://api.example.com/v1","schema":"openai"}]}}}
```

The example URL is a placeholder. Never put a key into `environment_keys`, URL, headers, or an answer. Environment keys are resolved first, then the provider's entry in `secrets.json`. Providers and models supplied by plugins are not owned by these JSON files.

## models.json

`models` is an array. Each entry identifies a model with `id` (the exact API model id) and `provider_id` (a key in `providers`). Optional `canonical_slug`, `name`, `description`, `family`, `knowledge`, `release_date`, `last_updated`, `open_weights`, and `status` describe it. `modalities.input` and `.output` list `text`, `image`, `audio`, `video`, or `file`; use only confirmed capabilities. `limit.context` and `limit.output` are token limits or null. `supported_params` is a closed list documented in `configuration.md`; unsupported names fail parsing. `reasoning` has `mandatory`, `default_enabled`, `default_effort`, `supported_efforts`, and `budget_supported`. `pricing.input`, `pricing.output`, and `pricing.cached` are optional reference prices per token, not the source of reported request cost.

```json
{"models":[{"id":"vendor/model","provider_id":"example","name":"Example model","modalities":{"input":["text"],"output":["text"]},"limit":{"context":32768,"output":4096},"supported_params":["tools"]}]}
```

The example capabilities and limits are illustrative, not model facts. Look up authoritative model capabilities before configuring a real model. After editing, use `norte service catalog`, `norte service doctor`, and the service's reload mechanism or restart as appropriate; check errors instead of assuming a live process has reloaded.
