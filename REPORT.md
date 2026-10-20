# Milestone report: 6 and 8

This is the close-out report for the compaction milestone (6) and the
hardening milestone (8). The benchmark is the repository checks plus targeted
unit and integration tests; it is not a claim that every provider or platform
was exercised.

## What shipped

### Milestone 6: compaction

- Automatic compaction at `compaction.threshold` (default `0.90`), with
  `compaction.trigger` set to `idle` (default) or `eager`.
- `/compact`, `ClientRequest::Compact` and the CLI path all use the same
  summarisation code.
- The old Norte session is retained and linked through `compacted_from`; the
  logical session and its title stay the same, and the new Norte session id is
  shown.
- The summary is a normal provider request with the session budget, telemetry
  and the configured compaction model/reasoning. Its reported cost enters the
  ledger and budgets.
- A failed summary leaves the old history and session untouched. A suffix never
  begins with an orphaned tool result, eager compaction never runs while an
  approval is parked, and it runs at most once per turn.
- Todo state is carried into the new session.

### Milestone 8: hardening

- Bounded retries for rate limits, overload, server errors and transport
  failures, with provider `retry_after`, capped backoff and no duplicate user
  message. `RetryScheduled` is visible to clients.
- Optional command sandbox as an executor wrapper: bubblewrap on Linux,
  `sandbox-exec` on macOS, fail-closed elsewhere. Approvals show the original
  command plus a sandbox badge.
- `--sandbox`, `--no-sandbox` and `--sandbox-network on|off`, with the
  incompatible combination rejected.
- Telemetry records the effective sandbox and network state; `--debug` shows
  them without keys or endpoint URLs.
- Per-request footer and JSONL fields: input/output tokens, tokens per second,
  total time, first-token latency, cache-read share, input:output ratio, tools
  and reported cost. Token rate is first output delta to request end, with a
  whole-request fallback when the generation window is too small.
- `AGENTS.md` is appended to the base prompt as untrusted, capped guidance.
- Interactive `/help`, `/allow`, `/deny` and `/refresh`, including revoking a
  saved deny by setting the opposite rule.
- `ARCHITECTURE.md`, `CONFIG.md` and the README architecture diagram.

## Verified here

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` and `cargo test --workspace` pass.
- Hermetic service tests cover compaction success, failure rollback, the
  idle/eager triggers, tool pairing, cost/budget accounting, retries, approval
  rules, session-rule persistence, the agent loop and protocol mapping.
- The CLI tests cover command parsing, approval parsing, token-rate calculation
  and footer rendering.
- Linux bubblewrap 0.13.0 was run on this machine: writes outside the workspace
  failed, `/tmp` was private, and the network was loopback-only by default.
- The macOS seatbelt profile has a pure unit test for network denial, writable
  roots and control-character rejection. It runs on every platform; the
  `sandbox-exec` execution path itself was not run here.
- OpenRouter live traffic was exercised. It reports prompt/output/cache counters
  and a USD cost on paid routes; the free route used here reports zero. Missing
  costs are stored as `NULL`, never zero.

## Not verified here

- Live OpenAI, Anthropic and Google/Gemini requests: the API keys were unset in
  this environment, so their cost/usage payloads were not probed.
- Actual `sandbox-exec` execution on macOS: this host is Linux. The profile
  generator is tested, but deprecation or availability of the binary on a given
  macOS release is not.
- The TUI (milestone 7) is still a stub. The CLI is the supported client.
- Budget `action = "ask"` is a resumable stop with a notice, not an immediate
  approval prompt. Raising the budget and resuming continues the parked turn.
- `skill_script` is not covered by the command sandbox; it remains opt-in
  behind `skills.allow_scripts`.
- There is no command to revoke a one-shot approval; session and global allow
  rules can be changed with `/allow` and `/deny`.

## Next work

- Milestone 7: the ratatui client over the existing protocol.
- A first-class budget-ask prompt if the parked-stop behaviour is not enough.
- Provider probe runs for OpenAI, Anthropic and Gemini once keys are available.
- A macOS runner in CI to execute the seatbelt path instead of only testing its
  profile.
