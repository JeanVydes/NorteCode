---
name: norte-code
description: Use and configure Norte Code, including its service, providers, models, MCP servers, plugins, and client settings.
---

# Norte Code

Use `norte --help` and `norte service <command> --help` to inspect supported commands and options. `norte` starts the local service if needed and opens the TUI. `norte tui` attaches without starting it. `norte service up`, `down`, `restart`, `status`, `logs`, `doctor`, `config`, and `catalog` manage or inspect the service. Use `norte service providers`, `mcps`, and `plugins` with `--help` for their operations.

Configuration paths use `$XDG_CONFIG_HOME/norte-code` when set, otherwise `~/.config/norte-code`:

- `service.json`: service settings and `agents`, including each agent's `mcpServers`, `skills`, and delegation profiles. The service also loads `<workspace>/.norte-code/service.json` as an overlay. Inspect the current structure before editing.
- `cli.json`: CLI connection and lifecycle settings; the TUI reads its `ui` preferences here. `norte_code_skill` controls this bundled skill on CLI startup. The TUI uses `service.url` and `service.token` when that block exists; otherwise it uses `service_url` and `service_auth`.
- `providers.json` and `models.json`: provider wires and model catalog. Prefer the service provider commands or API when available; inspect their schema before editing. Plugin-contributed entries are owned by their plugins.
- `secrets.json`: stored provider credentials. Prefer the appropriate key management interface; never print secret values. Environment variables named in the provider catalog take precedence.
- `skills/`: user skill directories, each with a `SKILL.md`.

The default plugin directory is `~/.config/norte-code/plugins`, overridden by `NORTE_CODE_PLUGINS`. `sources.json` records installed plugin origins. Restart the service to load changes to installed plugins or MCP configuration.

The service writes `server.pid` and `server.log` under `$XDG_STATE_HOME/norte-code`, or `~/.local/state/norte-code`. The default session database and replay data are under `$XDG_DATA_HOME/norte-code`, or `~/.local/share/norte-code`. `NORTE_CODE_DATABASE` overrides the database path. Use `norte service status`, `logs`, and `doctor` before manually inspecting or changing runtime files.

The service workspace is the working directory where its process starts. A running service is shared by later CLI invocations and retains that workspace until restarted. It loads `AGENTS.md` and project configuration from that workspace. Start or restart the service from the desired directory when the workspace needs to change. Check `service status` first, since restart interrupts the existing service.

Preserve unrelated settings when editing configuration, validate JSON, and do not invent CLI options or assume a project client overlay is loaded. Changes to service profiles may require a service restart; preference changes in the TUI persist to the user `cli.json`.
