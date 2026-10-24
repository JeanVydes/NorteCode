# Norte Code

**A terminal-based agentic coding tool in Rust.** Norte Code is a host around
[Norte](https://github.com/JeanVydes/Norte): Norte owns the durable session, the
turn, streaming, tool dispatch and the commit point; Norte Code owns the agent
loop, the model catalogue, approvals, compaction, budgets and the terminal
experience.

Nothing in this repository forks Norte. If a change belongs in the engine, it is
reported upstream instead of patched around here.

## The mental model

Five ideas hold the host together.

- **The service is in-process.** `Service` is a library, not a daemon. A client
  submits `ClientRequest` values and consumes a stream of `ServerEvent` values.
  The CLI is the only client. There is no port to open.
- **A logical session survives its engine.** A `SessionKey` stays stable while
  the Norte `SessionId` behind it may change. Compaction is the clearest case:
  the transcript is summarised into a fresh Norte session, but the client keeps
  the same key, title and history of decisions.
- **Norte owns the commit; the host owns the loop.** A turn is one engine round
  trip and one commit. The host decides when to call the engine again after a
  tool call, when to ask a human, when to compact and when a retry is safe.
- **Effects are governed and journaled.** Tools see only the configured
  workspace, effectful calls pass an approval policy, and an intent is written
  before a write or command runs so a crash cannot silently repeat it.
- **Cost is what the provider reported.** Norte Code never derives a dollar
  amount from a catalogue list price. An unreported cost is `NULL`, not zero.

## What it is not

- Not a separate server. The protocol is transport-agnostic, but v1 ships only
  the in-process tokio transport.
- Not a model catalogue. Models come from models.dev, with local overrides and a
  cache; the host does not bake in a price table.
- Not a fork of Norte. Sessions, history, policies and commits are Norte's.
- Not finished. The service and CLI are usable.

## Where to go

- New here: [Getting started](getting-started.md).
- Want the vocabulary first: [The service and the wire](concepts/service-and-protocol.md),
  then [Sessions and turns](concepts/sessions-and-turns.md).
- Solving something concrete: the [guides](guides/run-a-prompt.md).
- Looking for a type or a command: [Crates](reference/crates.md) and the
  [CLI reference](reference/cli.md). The full Rust API is rustdoc
  (`cargo doc --workspace --no-deps --open`).
- Auditing what has actually been exercised: [Status and verification](reference/status.md).
