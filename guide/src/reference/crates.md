# Crates

The workspace is split so that a client never compiles the engine and the host
never depends on the CLI.

| Crate | Responsibility |
| --- | --- |
| `norte-code-protocol` | Versioned serde types for `ClientRequest` and `ServerEvent`, plus the configuration, catalogue, cost ledger, host tools, skills and the SQLite store. No Norte types. |
| `norte-code-agent` | `Service`: engines, sessions, agent loop, compaction, retries, approval rules, sandbox, telemetry, skills. |
| `norte-code-cli` | The `norte-code` binary, its subcommands and the interactive prompt. |


## Dependency direction

```text
norte-code-cli ──► norte-code-agent ──► norte
```

`norte-code-protocol` depends on neither `norte` nor `norte-code-agent`.
`norte-code-agent` and the CLI depend on Norte as a git dependency pinned in
`Cargo.lock`.

## Public entry points

- `norte_code_agent::Service`
  - `with_interceptor` registers a host interceptor: the engine consults it at
    the safe points of a turn, and it may add messages to the history or stop
    the turn with a reason, which the client hears as `TurnStopped`.
- `norte_code_agent::EventStream`
- `norte_code_agent::ModelFactory`, `ModelSpec`, `VendorModels`, `Wire`
- `norte_code_agent::effective_reasoning`, `model_list`, `PromptSize`
- `norte_code_protocol::{ClientRequest, ClientRequestKind, ServerEvent, ServerEventKind}`

The full API is in rustdoc:

```sh
cargo doc --workspace --no-deps --open
```
