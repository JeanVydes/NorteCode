# Crates

The workspace is split so that a client never compiles the engine and the host
never depends on the CLI.

| Crate | Responsibility |
| --- | --- |
| `norte-code-protocol` | Versioned serde types for `ClientRequest` and `ServerEvent`. No Norte types. |
| `norte-code-agent` | `Service`: engines, sessions, agent loop, compaction, retries, approvals, sandbox, telemetry, skills. |
| `norte-code-config` | Typed config, merge, validation, approval-rule updates and secret resolution. |
| `norte-code-catalog` | models.dev client, ETag/TTL cache and mapping to Norte capabilities. |
| `norte-code-cost` | Cost ledger and budget evaluation. Provider-reported amounts only. |
| `norte-code-tools` | Host tools: `glob`, `list_dir`, `grep`, `todo`, `ask_user`, `web_fetch`. |
| `norte-code-store-sqlite` | Norte `Store` + `BlobStore`, logical sessions, approvals, questions, effects, costs and request telemetry. |
| `norte-code-cli` | The `norte-code` binary, its subcommands and the interactive prompt. |


## Dependency direction

```text
norte-code-cli ──► norte-code-agent ──► norte
       │                  │
       ├──► protocol      ├──► config
       ├──► catalog       ├──► catalog
       ├──► config        ├──► cost
       ├──► cost          ├──► tools
       ├──► skills        ├──► store-sqlite
       └──► store-sqlite  └──► skills
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
