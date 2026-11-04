# Norte Code

A terminal-based coding agent. Open a session in a directory, ask for work
in plain language, and the model reads, writes and runs commands there —
with your approvals, your budgets and your model.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/JeanVydes/NorteCode/dev/scripts/install.sh | sh
```

Or build it from source (Rust 1.88+):

```sh
cargo build --release
```

## Quick start

```sh
norte-code
```

Pick or create a session and start typing. A few things worth knowing:

- **Approvals** — tools run only under your rules. `/allow` and `/deny`
  decide per tool, per session or globally.
- **Plans** — the model proposes work as a Markdown file and asks your
  approval before it becomes active. `/plans` lists them.
- **Budgets** — cap spend in USD, tokens or requests: `--budget` on the
  command line or `budgets` in the config.
- **Sandbox** — opt-in OS containment for `shell` (`--sandbox`).

## Providers and models

Models come from two files you own beside the config — `providers.json`
and `models.json`. Nothing is fetched or bundled. Keys come from the
environment or a private `secrets.json` (mode 0600); they are never in
`config.json`.

Prefer not to maintain the files? The
[catalog-plugin](https://github.com/JeanVydes/catalog-plugin) offers a
ready-made catalogue as a plugin:

```sh
norte-code plugins add https://github.com/JeanVydes/catalog-plugin
```

## Documentation

- **[Guide](https://jeanvydes.github.io/NorteCode/)** — getting started,
  sessions, plans, approvals, budgets, sandbox, plugins and the full CLI
  and config reference. Run `mdbook serve guide` to read it locally.
- **[CONFIG.md](CONFIG.md)** — the full configuration schema.

## License

MIT. See [LICENSE](LICENSE).
