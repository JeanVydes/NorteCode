# Norte Code

**An open source coding agent for the terminal.** Norte Code combines a Rust
agent service with a keyboard-driven terminal interface. It supports local or
remote service connections, multiple agents, model providers, tools, plugins,
MCP servers, plans, and session telemetry.

Start with the [getting started guide](guide/src/getting-started.md), or install
the command line and open the interface:

```sh
cargo install --git https://github.com/JeanVydes/NorteCode --branch dev norte-code-cli --force
norte
```

The binary is named `norte` and installs into `~/.cargo/bin`. `--force` replaces
an existing installation. This command follows the `dev` branch.

## Use Norte Code

```sh
# Open the TUI. A local service is prepared for the current workspace.
norte

# Start a local service independently of a TUI.
norte service up
norte service status
norte service logs --follow

# Open the TUI without changing the service lifecycle.
norte tui
```

When the last TUI opened with `norte` exits, its local service is stopped. Use
`norte service up` when you want the service to remain detached. To connect to
a remote service, set `service_type` and `service_url` in `cli.json` as shown in
the [configuration guide](CONFIG.md).

Inside the TUI, use `/help` to see chat commands. Tabs include chat, sessions,
models, telemetry, and settings.

## Providers and models

Norte Code includes OpenAI, Anthropic, and Google wire adapters. Provider and
model entries are managed separately. Add a provider and its API wire with
the CLI:

```sh
norte service providers add my-provider --name "My Provider" --keys MY_PROVIDER_API_KEY
norte service providers add-wire my-provider \
  --schema openai \
  --base-url https://api.example.com/v1 \
  --auth bearer
```

Add model entries to `models.json`, or install a trusted catalogue plugin that
provides models dynamically. The API key value stays in the environment or in
the private `secrets.json` file. See [provider configuration](guide/src/guides/configure-a-provider.md)
for complete examples.

## Extend the service

Norte Code supports plugins for tools, slash commands, widgets, and catalogues.
Plugins run as native libraries, so install them only from sources you trust.

```sh
norte service plugins list
```

Add plugins and configure local or remote MCP servers using the [plugin guide](guide/src/guides/plugins.md)
and [CLI reference](guide/src/reference/cli.md).

## Documentation

- [Getting started](guide/src/getting-started.md)
- [Configuration reference](guide/src/reference/configuration.md)
- [CLI reference](guide/src/reference/cli.md)
- [Plugins and security](guide/src/guides/plugins.md)
- [Terminal interface](norte-code-tui/README.md)
- [Norte model and tool library](https://github.com/JeanVydes/Norte)
- [terminalui library](https://github.com/JeanVydes/terminalui)
- [Changelog](CHANGELOG.md)

## License

MIT. See [LICENSE](LICENSE).
