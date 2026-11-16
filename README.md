# Norte Code

Norte Code is a single-binary agent host. One command, `norte`, manages a
background service (sessions, execution, catalogues, plugins) and, when asked,
opens the terminal interface that talks to it. Everything else — providers,
models, MCP servers, plugins — is configured from the same command.

## Install

```sh
cargo install --git https://github.com/JeanVydes/NorteCode norte-code-cli --force
```

`--force` (re)installs the binary, so running the same command is also how you
update to the latest `dev` build. The binary installs into `~/.cargo/bin/norte`.
Everything else — the background service, the TUI, the catalogue, the
configuration — is managed from that one command.

## Run

```sh
# start the service if needed and open the terminal interface:
norte

# open only the terminal interface:
norte tui

# manage the background service:
norte service up
norte service down
norte service restart
norte service status
norte service catalog
norte service config
norte service logs [--live|-f|--follow]
norte service doctor
```

The service runs detached: closing the terminal interface does not stop it.

## Configure providers

Providers and models live in two files beside the configuration
(`providers.json`, `models.json`). They are empty by default — nothing is
fetched or bundled. Manage them from the CLI:

```sh
norte service providers add openrouter \
  --wire openai --base-url https://openrouter.ai/api/v1 \
  --key OPENROUTER_API_KEY

norte service providers list
norte service providers remove openrouter
```

`--key` names an environment variable; the value is never written to disk.
Keys can also come from a private `secrets.json` (mode 0600) beside the config.

## MCP servers

```sh
norte service mcps add fs --command npx --arg -y --arg @modelcontextprotocol/server-filesystem --arg /tmp
norte service mcps add remote --url https://mcp.example.com/mcp --header 'Authorization=Bearer x'
norte service mcps list
norte service mcps remove fs
```

MCP servers are connected when the service starts, so restart it after adding
or removing one.

## Plugins

Plugins are git repositories built as cdylibs and copied into the plugin
directory:

```sh
norte service plugins add https://github.com/JeanVydes/catalog-plugin --package catalog-plugin-openrouter
norte service plugins list
norte service plugins disable <name>
norte service plugins enable <name>
norte service plugins remove <name>
```

Plugins are loaded when the service starts, so restart it after installing,
enabling or disabling.

## Configuration

- `CONFIG.md` — the full configuration schema (`service.json`, `cli.json`,
  catalogue files, secrets).
- `AGENTS.md` — the crate layout and the rules that keep it that way.
- `guide/` — the user guide.

Selecting a model in the TUI checks the provider's `environment_keys`, then
`secrets.json`. If no key is available, a prompt asks for one; it is saved
to `secrets.json` (mode 0600) and the model switch continues.

## License

MIT. See [LICENSE](LICENSE).