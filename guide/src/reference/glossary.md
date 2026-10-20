# Glossary

**Agent loop** — The host loop that decides when to call the engine again after
a tool result. Norte owns one engine turn; Norte Code owns the iterations around
it.

**Approval** — A durable decision that lets or refuses a parked tool call. It is
bound to the call and its arguments.

**Budget** — A message, session or daily limit in USD, tokens or requests. `warn`
continues, `stop` ends the turn uncommitted, `ask` currently behaves like a stop.

**Catalogue** — The models.dev data plus the local cache and bundled snapshot.
It supplies capabilities and endpoints; it does not supply prices to the ledger.

**Compaction** — Summarising a long transcript into a fresh Norte session while
keeping the logical session key. The old session stays for audit.

**Effect journal** — Intent/finish records for effectful tools. A call still at
intent on startup is refused rather than replayed.

**Engine** — Norte's durable turn machine: one model, one session, policies,
tools and a commit point.

**Event** — A `ServerEvent` on the client wire. It answers a `request_id` and may
carry a logical session key.

**Grant** — A live approval held by Norte for a scope (once, tool, session or
always).

**Logical session** — Norte Code's stable `SessionKey`. It survives model
switches and compaction.

**Norte** — The provider-agnostic agent engine this host depends on. It is not
forked here.

**Prompt** — The base system prompt plus project `AGENTS.md` instructions. The
latter is untrusted and capped.

**Provider** — A key source, endpoint and wire selected by the catalogue entry.
The model reference is `provider/model`.

**Request** — A `ClientRequest` sent to the service. It is not a provider HTTP
request; telemetry records provider attempts separately.

**Sandbox** — An executor wrapper that launches `command` under bubblewrap or
`sandbox-exec`. It is not a virtual machine and has no unsandboxed fallback.

**Session** — Ambiguous by design: a logical session is what the client sees; a
Norte session is the durable transcript behind it. Compaction is the case where
they diverge.

**Skill** — An Agent Skills directory with `SKILL.md` and optional resources.
Discovery is passive; scripts are opt-in.

**Telemetry** — Local request rows with latency, tokens, cache, tools and status.
It is not remote analytics and not the money ledger.

**Turn** — One model round trip and its commit. A tool loop spans several turns.

**Workspace** — The canonicalized directory that bounds file tools. It is a
policy boundary, not an OS jail.
