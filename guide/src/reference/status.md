# Status and verification

Norte Code is under construction. The service and CLI are usable; the TUI is a
stub. This page records what has actually been exercised, not what the code
aspires to.

## What is implemented

- The in-process service with logical sessions, streaming, cancellation,
  model switching, compaction, retries, approvals, questions, budgets, telemetry
  and the SQLite directory.
- The CLI: `run`, `resume`, `models`, `skills`, `sessions`, `cost`, `telemetry`,
  interactive mode, JSONL, approvals, budgets and sandbox flags.
- The host tools, Agent Skills discovery, workspace policy and the command
  sandbox wrapper.
- The versioned protocol and the mdBook guide you are reading.

## Verified here

- `cargo fmt --all --check`.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- `cargo test --workspace -- --test-threads=1` passes all workspace suites.
- Hermetic service tests cover the agent loop, compaction rollback, costs and
  budgets, retries, approvals, session rules, the effect journal and protocol
  mapping.
- CLI tests cover command parsing, approval parsing and footer rendering.
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
- `budget.action = "ask"` is a resumable stop with a notice, not an immediate
  approval prompt.
- `skill_script` is outside the command sandbox and remains opt-in.
- There is no command to revoke a one-shot approval.

## Test caveat

A few service tests are sensitive to full parallel execution and have shown
intermittent failures while the same suite passes with `--test-threads=1`. The
failure was reproduced on the commit before the service/CLI module split, so it
is not a regression from that refactor. The deterministic command used for
close-out is:

```sh
cargo test --workspace -- --test-threads=1
```

The full milestone close-out is in the repository's
[`REPORT.md`](https://github.com/JeanVydes/NorteCode/blob/dev/REPORT.md).
