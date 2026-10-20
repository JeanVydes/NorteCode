# Norte Code

A terminal-based agentic coding tool, in Rust.

Norte Code is a host that sits on top of [Norte](https://github.com/JeanVydes/Norte),
the provider-agnostic agent engine. Norte owns the session, the turn, streaming,
tool dispatch, policies and durability; Norte Code owns the agent loop, the
model catalogue, approvals, compaction, cost tracking and the user interface.
Norte Code never forks Norte.

> Status: under construction. The workspace skeleton is in place; the service,
> CLI and TUI are built milestone by milestone. Nothing here is released yet.

## Layout

| Crate | What it is |
| --- | --- |
| `norte-code-protocol` | The versioned serde types on the client wire. No Norte types. |
| `norte-code-agent` | The service library: engines, agent loop, policies, sessions, compaction. |
| `norte-code-config` | `config.json` loading, validation, merging and secrets. |
| `norte-code-catalog` | The models.dev client, its disk cache and the mapping to Norte capabilities. |
| `norte-code-cost` | The cost ledger and budget evaluation. Provider-reported cost only. |
| `norte-code-tools` | The tools Norte does not ship: `glob`, `list_dir`, `grep`, `todo`, `ask_user`, `web_fetch`. |
| `norte-code-store-sqlite` | A Norte `Store` + `BlobStore` over SQLite, plus Norte Code's own tables. |
| `norte-code-cli` | The headless `norte-code` command. |
| `norte-code-tui` | The interactive terminal UI. |

## Building

```sh
cargo build
cargo test
```

The engine is a git dependency (`norte`, branch `dev`). `Cargo.lock` pins the
exact revision and is committed.

## Running

```sh
norte-code run "explain this repository"
norte-code run --session <key> "continue with the tests"
norte-code sessions
norte-code skills list
```

Sessions live in a SQLite database under the platform data directory
(`$XDG_DATA_HOME/norte-code/sessions.db`, or
`~/.local/share/norte-code/sessions.db`). A run creates a new session unless
`--session` names an existing key. Parked approvals and pending questions are
in that database, so a session can be resumed after a restart.

## Network

The only network destinations are the configured model endpoints and the model
catalogue URL (default `https://models.dev/catalog.json`). Norte Code has no
telemetry.

## License

MIT. See [LICENSE](LICENSE).
