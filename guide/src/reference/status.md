# Status and verification

Norte Code is under construction. The service and CLI are usable; the TUI is a
stub. This page records what has actually been exercised, not what the code
aspires to.

## What is implemented

- The in-process service with logical sessions, streaming, cancellation,
  model switching (immediate when idle, queued for the next turn boundary when
  busy), session and one-message reasoning overrides, steering into the running
  turn, compaction, retries, approvals, questions, budgets, telemetry and the
  SQLite directory.
- The CLI: `run`, `resume`, `models`, `skills`, `sessions`, `cost`, `telemetry`,
  interactive mode, JSONL, approvals, budgets and sandbox flags.
- The interactive commands `/help`, `/compact`, `/allow`, `/deny`,
  `/reasoning`, `/model`, `/revoke`, `/refresh` and `/exit`, with dimmed
  suggestions for the command and its arguments. Text typed while a turn runs
  is steered into that turn and reaches the request it is about to make.
- The host tools, Agent Skills discovery, workspace policy and the command
  sandbox wrapper.
- The versioned protocol and the mdBook guide you are reading.

## Verified here

- `cargo fmt --all --check`.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- `cargo test --workspace` passes all workspace suites in parallel.
- Hermetic service tests cover the agent loop, compaction rollback, costs and
  budgets, retries, approvals, session rules, the effect journal and protocol
  mapping.
- CLI tests cover command parsing, approval parsing, the todo footer widget,
  footer rendering and the command suggestion engine.
- Linux bubblewrap 0.13.0 was exercised: writes outside the workspace failed,
  `/tmp` was private and the network was loopback-only by default.
- The macOS seatbelt profile has a unit test for network denial, writable roots
  and control-character rejection.
- OpenRouter live traffic was exercised; paid routes report usage and a cost,
  the free route reports zero, and missing costs stay missing.

## Not verified here

- Live OpenAI, Anthropic and Google/Gemini requests: the keys were unset in the
  development environment.
- Actual `sandbox-exec` execution on macOS: the host was Linux. The profile
  generator is tested, but the binary's availability and deprecation on a given
  macOS release are not.
- The TUI (milestone 7) is still a stub.
- `skill_script` is outside the command sandbox and remains opt-in.

## Test caveat

A few service tests exercise concurrent turns and can be sensitive to machine
load. `cargo test --workspace` is the normal gate. If a failure looks timing
related, rerun the suite single-threaded before treating it as a regression. The
deterministic command used for close-out is:

```sh
cargo test --workspace -- --test-threads=1
```

The full milestone close-out is in the repository's
[`REPORT.md`](https://github.com/JeanVydes/NorteCode/blob/dev/REPORT.md).
