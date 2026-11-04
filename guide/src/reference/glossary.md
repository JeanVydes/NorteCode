# Glossary

**Budget** — A message, session or daily limit in USD, tokens or requests.
`warn` continues, `stop` ends the turn uncommitted, `ask` asks for a new
limit first.

**Catalogue** — The two files that say who serves models and what each one
can do: `providers.json` and `models.json`. Never a source of prices.

**Compaction** — Summarising a long transcript into a fresh engine session
while keeping the same session key. The old session stays for audit.

**Directory** — The root the file tools resolve against. It starts as the
process's current directory; a leading argument to `norte-code` moves the
process there before config is read. The footer names it.

**Effect journal** — Intent/finish records for effectful tools. A call
still at intent on startup is refused rather than replayed.

**Plan** — A Markdown proposal saved under
`~/.local/norte-code/<session>/plans/`, approved by a person before it
becomes `active`. See [Plans](../plans.md).

**Prompt** — The base system prompt plus the workspace's `AGENTS.md`,
appended. The latter is untrusted and capped at 64 KiB.

**Provider / model** — Two values, never one string. Where both are shown
they are joined as `provider/model` — a label, never parsed back.

**Request** — A `ClientRequest` to the service. Not a provider HTTP
request; telemetry records those separately.

**Sandbox** — Wraps `shell` in bubblewrap or `sandbox-exec`. Opt-in, not a
virtual machine, no unsandboxed fallback.

**Session** — What the client sees (a stable key). Behind it sits the
durable transcript; compaction is where the two diverge.

**Skill** — An Agent Skills directory with `SKILL.md`. Discovery is
passive; scripts are opt-in and run outside the sandbox.

**Telemetry** — Local request rows with latency, tokens, cache, tools and
status. Not remote analytics, not the money ledger.

**Turn** — One model round trip and its commit.

**Workspace** — The canonicalized directory that bounds file tools. A
policy boundary, not an OS jail.
