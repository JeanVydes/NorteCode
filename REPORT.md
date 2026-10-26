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
- There is no TUI. The interactive CLI is the supported client; no full-screen
  client is planned.
- Budget `action = "ask"` is a resumable stop with a notice, not an immediate
  approval prompt. Raising the budget and resuming continues the parked turn.
- `skill_script` is not covered by the command sandbox; it remains opt-in
  behind `skills.allow_scripts`.
- There is no command to revoke a one-shot approval; session and global allow
  rules can be changed with `/allow` and `/deny`.

## Next work

- Polishing the interactive REPL: redraw on demand, cancelling the input line,
  file and model completion, and a persistent history.
- A first-class budget-ask prompt if the parked-stop behaviour is not enough.
- Provider probe runs for OpenAI, Anthropic and Gemini once keys are available.
- A macOS runner in CI to execute the seatbelt path instead of only testing its
  profile.

## Next work: the browser tool

Two things are open, and the second depends on the first.

**A CDP bug, reproduced live and not yet diagnosed.** `filter` and
`max_elements` reach the host but not the tool: calling `extract_dom` with
`max_elements: 3` on a page with thirteen elements returned thirteen, and with
`filter: "sign"` returned all thirteen too — with the same `request_digest` in
every call, which is the proof that the struct the tool saw was identical. The
same fields work when the Rust tool is called directly from a test, so the loss
is between the agent and the tool. Note the counter-example: `endpoint` and
`highlight` do reach it, and a bad `endpoint` fails the call as it should. The
next step is to print the argument document the executor is handed for one
`extract_dom` turn — `emit` a debug line in `norte-tools`, or log
`view.call.arguments` where the policy runs — and see which fields are there.
Suspects, in order: the schema the host advertises (a stale vendored
`norte-tools` in the dependency graph), a policy that rebuilds the arguments,
and `serde` dropping unknown fields on the way in.

**Then: a representation of the page, on demand.** `extract_dom` returns every
visible `a, button, input, textarea, select, [role=button]` — 1841 elements and
about 7500 tokens on a Wikipedia article — and the shape is flat and repetitive:
five keys per element, `"input_type": null, "placeholder": null` on most of
them, and the same page's worth of JSON sent again after every navigation. What
it should be instead:

- **A tree, not a list.** Send the DOM as a pruned tree: containers that hold
  one child collapse, and a subtree that is not what was asked for becomes a
  single summarised node — `section: 14 links` — that a later call expands by
  a path or an id. That is the "redact the leaves, expand on demand" idea: the
  model walks down only where it is going.
- **Query, don't dump.** The filter is already there but coarse (a substring of
  the label). What it wants: "every `a` with an `href`", "every `input`",
  "anything containing X", "only what is inside `#main`". That is a selector or
  a small query language over the tree, and it is what makes the first call
  cheap: a model looking for a search box asks for the inputs.
- **Only what is missing, and only what changed.** `href` for a link is one
  short string that saves a click and a navigation; `disabled`, `checked`,
  `selected` is one bool that saves a click that would fail. Both are absent
  today. After a navigation, sending the whole page again is the same tokens
  twice: a diff against the previous extraction would say what appeared and
  what went.
- **Say what was left out**, as `elements_matched` already does. The same rule
  the rest of this repository follows: a bounded answer that does not admit it
  is bounded is worse than a short one that does.

The shape of the tool's output is the contract, so this is a change to
`norte-tools` first and to the host only where it renders.

## Next work: tabs in the CLI

The client has no way to move between the things it is showing. Two levels,
both missing:

- **Session tabs** — the things inside one session: `chat`, the task list, and
  debug information. Debug information is printed at startup today and should be
  a tab the person opens, not a wall of text before the first prompt.
- **Session switcher** — the sessions themselves: change to another, open a
  new one, and explore the other sections the service already knows about.

The protocol already carries `ListSessions`, `SwitchSession`, `SessionList` and
`SessionSwitched`, and the service implements them; the CLI does not call any of
them, so this is client work over a wire that is already there. Two things to
respect while doing it: the footer row count is `INPUT_ROWS + 4` and the scroll
region has a test that pins it (`7a45fb7`), and the renderer's todo widget and
session status share the pinned row with the input editor.
