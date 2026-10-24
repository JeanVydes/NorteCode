# Task: build "Norte Code", a terminal-based agentic coding tool in Rust

## Goal
Norte Code is a Claude-Code/Codex-style coding agent for the terminal. It is a
SEPARATE project (own repo, own Cargo workspace) that depends on the published
`norte` crate (provider-agnostic agent engine). It must not modify or fork Norte.
Read Norte's guide first (https://github.com/JeanVydes/Norte, branch `dev`, folder
`guide/src`: concepts/*, guides/*, reference/requirements.md), then read the source
of norte-core (engine.rs, capability.rs, layer.rs, policy.rs) and norte-tools.
Norte's docs and code are the source of truth for its API; pin an exact version or
git rev. Naming: app "Norte Code", binary `norte-code`, crates prefixed
`norte-code-` so they never collide with Norte's own `norte-*` crates.

## Repository and git workflow
- Location: ~/Projects/norte-code (create it). Remote: https://github.com/JeanVydes/NorteCode
  (already created and empty). Working branch: `dev` (create with `git init -b dev`).
  Do not create `main` yet; it is reserved for future releases.
- Step 0, before any code: mkdir -p ~/Projects/norte-code && cd into it;
  `git init -b dev`; `git remote add origin https://github.com/JeanVydes/NorteCode.git`;
  run `git ls-remote origin` and confirm the remote is empty (if it is not, stop and
  ask); confirm `git config user.name` and `user.email` are set (if not, stop and
  ask; never change global git config). The first commit contains only: Cargo
  workspace skeleton, README stub, LICENSE (MIT, matching Norte), .gitignore,
  rustfmt/clippy config, CI workflow, AGENTS.md (contributor conventions, including
  this git workflow), CHANGELOG.md. Then `git push -u origin dev`.
- Commits: Conventional Commits with the crate as scope, e.g. `feat(catalog): cache
  models.dev with ETag`. Types: feat, fix, refactor, test, docs, chore, build, ci,
  perf. Imperative subject of at most 72 characters; the body explains the why when
  it is not obvious. One logical change per commit: no mixing refactors with
  features, no "WIP" or "fix typo" noise (use fixup and autosquash on LOCAL,
  unpushed commits only).
- Every commit must compile and pass `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings` and `cargo test --workspace`, so history stays
  bisectable. Run them before committing. Tests ship in the same commit as the
  feature. Update CHANGELOG.md (Keep a Changelog style) in the commit that adds a
  user-visible feature.
- Granularity: commit as each feature or sub-feature is finished and verified; a
  milestone is many commits. Push `dev` after each milestone and after significant
  feature groups. Larger or risky work (compaction safe-point spike, SQLite store,
  TUI) goes on short-lived `feat/<name>` or `spike/<name>` branches cut from `dev`
  and merged into `dev` with `--no-ff` after the checks pass; delete the branch after
  the merge. Small steps commit straight to `dev`.
- Never force-push; never rewrite pushed history. Tag the end of each milestone
  with an annotated tag `milestone-N` and push the tags. Do not open PRs unless asked.
- Hygiene: .gitignore covers target/, .env*, *.db, catalog caches, logs, editor
  files and any local state directory. Never commit API keys, auth files, SQLite
  databases or caches. Review `git diff --staged` before every commit and run a
  secret scan (gitleaks if installed) before each push. Commit Cargo.lock (this is
  an application). No large generated files.
- Push authentication: use the user's existing git credentials (SSH key or
  credential helper). If a push fails, report the exact error and stop. Never put a
  token in the remote URL and never write credentials to disk.

## Architecture: service + thin clients
Workspace crates:
- `norte-code-protocol`: serde types only, versioned. No Norte types on the wire.
  ClientRequest: send message (optional per-message budget and reasoning override),
  cancel turn, approve/deny/revoke, resume, steer, session ops (create/list/switch/
  fork/rename/delete), set_model, set_reasoning(scope), set_budget(scope), compact
  (manual), list_models (with each model's available reasoning options),
  refresh_catalog, cost_report, list_skills, refresh_skills. ServerEvent: stream
  deltas, tool started/finished, approval requested, request reconstructed (usage
  + reported cost), commit, compaction started/finished, budget warning/exceeded,
  cost update, cost not reported, model changed, reasoning downgraded, catalog
  updated, skills list/updated and skill activated, errors. Every event carries
  its session id.
- `norte-code-agent` (service library): owns the Norte engines, the agent loop,
  policies, layers, session manager, compaction, project instructions (AGENTS.md).
  Async API: submit ClientRequests, receive a Stream<ServerEvent>. Transport-agnostic.
- `norte-code-config`: config.json loading, validation, merging, secrets handling.
- `norte-code-catalog`: models.dev client, disk cache, mapping to Norte Capabilities.
- `norte-code-cost`: cost ledger and budget evaluation (provider-reported cost only).
- `norte-code-tools`: tools that Norte does not ship (see Tools).
- `norte-code-skills`: Agent Skills discovery, validation and progressive-disclosure
  loading (see Skills). It owns the skill tools' logic; `norte-code-agent` registers
  them in the shared tool registry.
- `norte-code-store-sqlite`: Norte `Store` + `BlobStore` over SQLite, plus Norte
  Code's own tables (logical sessions, compaction links, cost ledger, settings).
  Must pass `norte_testing::store_conformance`. CAS on version, one transaction
  per commit.
- `norte-code-cli`: headless `norte-code run "prompt"` (text and JSONL output, exit
  codes, `--budget`, `--model`, `--reasoning`, non-interactive approval flags) plus
  subcommands: `sessions`, `models`, `cost`, `config`.
- `norte-code-tui`: built with `ratatui` + `crossterm` on tokio. See TUI.
- Later, not v1: `norte-code-server` (unix socket / stdio JSON-RPC). Design the
  protocol and a `Transport` trait now; v1 ships only the in-process tokio transport.
Clients hold no agent logic and never touch Norte directly. Use edition 2024 or the
newest edition Norte supports, and an MSRV no lower than Norte's own.

## TUI (ratatui)
Structure it as model / update / view: one `App` state, one reducer that applies
ServerEvents and key events, and pure render functions, so the UI is testable
without a terminal. Use ratatui's `TestBackend` for rendering tests (snapshot tests
with `insta` are welcome). Requirements: streamed text and a collapsible reasoning
panel; tool-call panels showing the bytes returned; approval prompts; session
switcher; model picker and a reasoning selector that lists only valid options;
status bar with model, context-used %, session cost (as reported) and budget state;
Ctrl-C cancels the turn; resume a session after a restart. Terminal hygiene:
restore the terminal on exit AND on panic (panic hook), handle resize, bracketed
paste and Unicode width, never write to stdout from library code, keep the render
loop off the network path (events arrive over a channel), and bound every buffer.

## Tools
The agent uses Norte's built-in tools from `norte-tools`: `read`, `write`, `edit`,
`command` (read pages files and can regex-search one file; write creates/replaces
whole files; edit does atomic exact search/replace; command runs a program or
shell script with bounded output). Review their schemas and limits before wiring
them. Everything else lives ONLY in `norte-code-tools`, never in Norte:
- `glob` / `list_dir`: gitignore-aware file discovery, bounded results.
- `grep`: project-wide regex search (use the `ignore` + `regex` crates), bounded
  matches and line width, same discipline as Norte's `read` (paging, continuation
  hint, escaped control characters).
- `todo`: a task list the model maintains; state is host metadata (outside the
  reserved `norte.` prefix) and is carried through compaction.
- `ask_user`: park the turn and ask the human a question (reuse the approval/
  parking mechanism; must survive a restart).
- `web_fetch`: OFF by default, opt-in in config.json. Output is untrusted content
  (Norte NFR-SEC-1): clip, escape, never feed it as a system message.
- Skill tools: `skill`, `skill_resource` and (opt-in) `skill_script`. They are
  host tools focused on Agent Skills; their logic lives in `norte-code-skills`.
  See Skills for the contract. They are registered by default when skills are
  enabled, never by Norte.
Every new tool follows Norte's conventions: strict argument parsing
(`deny_unknown_fields`), schemars descriptions written for the MODEL with one
example argument object, serde-tagged enums also tagged with
`#[schemars(tag = "...")]`, bounded output, failures returned as `ToolOutput::error`
with an actionable message (never an engine error), honour the cancellation token
from `ToolContext`.
Command tool: Norte's `CommandTool` clears the environment, so pass only what is
needed via `with_env` (PATH etc.). NEVER pass provider API keys to child
processes, and register every configured API key via `with_secrets` so it is
redacted from output. Measurement hook: in Norte's own agent-cost benchmark 14 of
19 `command` calls failed and the trail does not record why. Record the error
text/code of every failed tool call in Norte Code's local logs and surface it in
`--verbose`, so schema or policy problems are visible rather than guessed.

## Skills (Agent Skills specification)
Norte Code implements the Agent Skills format from https://agentskills.io/specification.
Skills are user-authored capabilities (instructions plus optional scripts,
references and assets) that live in the workspace or in the user config, NOT in
Norte and not compiled into the binary. They are the first-class way to add
custom behaviour without adding a Norte tool or a plugin system. The format is
the source of truth; when the spec and this document disagree, follow the spec
and update this document.

### Layout and discovery
A skill is a directory whose name equals its `name` and that contains a required
`SKILL.md`, plus optional `scripts/`, `references/`, `assets/` and arbitrary
extra files:
```
skill-name/
├── SKILL.md          # required: YAML frontmatter + Markdown body
├── scripts/          # optional executable code
├── references/       # optional documentation loaded on demand
├── assets/           # optional templates and static resources
└── ...
```
Discovery roots, in increasing precedence (a later root shadows an earlier one
with the same `name`, and shadowing is reported, not silent):
1. the bundled `assets/skills/` directory (examples only, never executable);
2. the user skills dir, `<config-dir>/skills/` (e.g. `~/.config/norte-code/skills`);
3. `skills.paths` from config.json, in order;
4. the project skills dir, `<workspace>/.norte-code/skills/`.
Also accept `<workspace>/.agents/skills/` and `~/.agents/skills/` as
interoperable aliases, at the same precedence as their Norte Code equivalents,
when `skills.interop` is true (default true). Only direct children of a root are
skills; do not recurse arbitrarily. Do not follow symlinks that resolve outside
the discovery root. A root that does not exist is not an error. Scan once when
the service starts and again on `RefreshSkills`; a malformed skill is skipped
with a warning that names the path and the reason, and never aborts startup.

### `SKILL.md` contract
The file starts with `---`-delimited YAML frontmatter followed by Markdown.
Parse it with a maintained YAML parser; never hand-roll YAML. Required fields:
- `name`: 1-64 characters, only `a-z`, `0-9` and `-`, no leading/trailing hyphen,
  no consecutive hyphens, and it MUST equal the parent directory name.
- `description`: 1-1024 characters, non-empty; it should say what the skill does
  and when to use it, because it is the only text loaded at startup.
Optional fields, validated when present:
- `license`: short license name or a reference to a bundled license file.
- `compatibility`: at most 500 characters; environment requirements.
- `metadata`: a mapping of string keys to string values (not nested values).
- `allowed-tools`: a space-separated string of pre-approved tools
  (experimental; see below).
Unknown frontmatter keys are preserved for diagnostics and do not invalidate the
skill. The Markdown body is unrestricted, but keep `SKILL.md` focused: the host
warns above 500 lines and recommends moving detail to `references/`. File
references inside the body are relative to the skill root and should stay one
level deep.
Validation runs on load, on refresh and in `norte-code skills check`; the last
prints every skill (valid or skipped), its root and any warning. A test fixture
covers every invalid-name example from the spec (`PDF-Processing`, `-pdf`,
`pdf--processing`, directory/name mismatch) and the length limits.

### Progressive disclosure and activation
The registry keeps three tiers, and context use follows them:
1. **Metadata** (~100 tokens per skill): `name`, `description`, root and
   `allowed-tools`, loaded at startup for every valid skill. This is what a model
   sees first, through the `skill` tool's `list` action.
2. **Instructions** (<5000 tokens recommended): the full `SKILL.md` body, loaded
   only when the model activates the skill.
3. **Resources**: files under `scripts/`, `references/`, `assets/` or elsewhere in
   the skill root, read only on demand.

Activation is per session and explicit: the `skill` tool returns the body as a
tool result, or the TUI can activate a skill for the next message. Never rewrite
the system prompt or earlier history to inject a skill (cache stability); a
loaded body is a normal tool result in the session and stays in the transcript.
The service tracks active skill names per session so `allowed-tools` and the UI
can use them. Activating the same skill twice is a no-op that returns the body
again. A skill body is untrusted content (NFR-SEC-1): return it as a tool result,
escape control characters, apply the same output caps as `read`, and never
promote it to a system message.

### Skill tools (all in `norte-code-tools` + `norte-code-skills`)
- `skill`: `{"action": "list"}` returns name + description + root for every
  discoverable skill (bounded); `{"action": "activate", "name": "..."}` validates
  the name, marks it active and returns the full `SKILL.md` body followed by a
  one-level listing of `references/`, `scripts/` and `assets/`. Unknown names
  return an actionable error listing the closest valid names. Add
  `{"action": "deactivate", "name": "..."}` for symmetry.
- `skill_resource`: `{"skill": "...", "path": "references/REFERENCE.md"}` reads a
  file inside that skill's root using the same bounds, paging and control-character
  escaping as Norte's `read`. Reject absolute paths, `..` components and symlinks
  that escape the root. Relative paths resolve from the skill root.
- `skill_script`: `{"skill": "...", "script": "scripts/extract.py", "args": [...]}`
  runs a bundled script. OFF by default (`skills.allow_scripts`); when enabled it
  requires approval under the `ask` policy and respects `auto-edit`/`yolo` like
  `command`. Resolve the interpreter from the shebang, pass `args` as separate
  argv entries (never through a shell), set the working directory to the
  workspace root, expose the skill root only through `SKILL_DIR`, clear the
  environment and pass only `PATH` plus configured safe variables, never provider
  API keys, and bound time and output. A path outside `scripts/` is refused.
- `skills check` is a CLI subcommand, not a model tool. `ListSkills` and
  `RefreshSkills` are client requests; clients render progress, they never parse
  skill files.
`allowed-tools` is experimental: parse `Bash(git:*)`, `Read`, `Write`, `Edit` and
path-scoped forms into approval rules for calls made while that skill is active.
It can only pre-approve; it never overrides a `Deny`, path re-rooting, budgets,
secrets redaction or the sandbox. Because a project is untrusted input, honoring
it requires `skills.honor_allowed_tools: true` (default false) and is reported in
the approval UI. Unknown entries are warnings, not errors.

### Security and limits
- Never execute a skill at discovery time; parsing is inert.
- `skills.enabled` (default true) gates discovery; `skills.allow_scripts` (false),
  `skills.honor_allowed_tools` (false) and `web_fetch` (false) are separate
  opt-ins. Document the risk of running third-party scripts.
- Cap the number of skills, file size, total body size and resource reads; a
  truncated result says how to continue.
- Scripts inherit the same approval/journal and redaction story as `command`.
  Register API keys with `with_secrets`; pass no secrets in argv or env.
- Skills are files, not a plugin ABI: no dynamic loading, no Rust code, and no
  network access except what a user-approved script or `web_fetch` does.

## Providers, API keys, and the models.dev catalog
Providers in v1: OpenAI (Responses wire, via norte-openai), Anthropic (Messages),
Gemini (generateContent) and OpenRouter (OpenAI-compatible Chat wire through a
`VendorProfile` with base URL https://openrouter.ai/api/v1). Everything works with
API keys the user provides: env vars (OPENAI_API_KEY, ANTHROPIC_API_KEY,
GEMINI_API_KEY or GOOGLE_API_KEY, OPENROUTER_API_KEY) and/or `api_key_env` /
`api_key_file` entries in config.json. Do not store plaintext keys in config.json
by default; if an optional auth file is supported it must be created 0600. Keys
are held in Norte's `Secret` type, never logged, never in events or exports.
Norte has no model catalogue by design, so Norte Code owns one, sourced from
https://models.dev/catalog.json:
- The models.dev README also lists api.json and models.json, and documents per-
  model fields (limit.context/input/output, modalities.input/output, attachment,
  reasoning, tool_call, temperature, interleaved, structured output) and a provider
  `api` URL. The exact JSON shape was NOT verified when this prompt was written.
  Fetch the live file first, inspect it, choose the endpoint that actually carries
  full model data (fall back to api.json if catalog.json is an index), and generate
  typed serde structs with `#[serde(default)]` that tolerate unknown fields. Check
  how provider ids are spelled (Gemini is probably `google`, not `gemini`) and map
  them to Norte wires in ONE place.
- Fetch policy: never block startup on the network. Disk cache
  (~/.cache/norte-code/catalog.json) with ETag/If-Modified-Since, TTL in config
  (default 24 h), `norte-code models refresh`, use the stale cache on network
  failure, ship a small bundled snapshot as the last fallback, offline mode flag.
  Validate on load: size cap, schema check, sanity bounds on numbers; a bad
  download never replaces a good cache. The catalog URL is configurable. Say
  clearly in the README that the catalog URL and the model endpoints are the only
  network destinations (no telemetry).
- Mapping catalog entry -> `norte_core::Capabilities`: modalities (pdf ->
  Document), `limit.context` -> max_context, `limit.output` -> max_output, tool_call
  flag -> ToolKind::Function (a model without tool calling is not selectable for
  the agent), structured-output flag -> modes, cache mode by wire family
  (Anthropic: Explicit, OpenAI/Gemini/OpenRouter: Automatic), streaming Delta.
  Reasoning mapping is in its own section. Fields the catalog does not carry
  (parallel tool calls, forced tool choice, reasoning efforts) default
  conservatively and can be set per model under `model_overrides` in config.json.
  The catalog is community data: config overrides always win, and a missing
  `limit.context` disables compaction for that model with a visible warning
  instead of a guess. The catalog's `cost.*` fields are NOT used for anything.

## Models and sessions
- Multi-session: create, list, switch, fork, rename, delete. Sessions run
  concurrently in the service (one active turn per session; extra input goes
  through steering). Background sessions keep streaming; the TUI shows their
  state. Per-session settings: model, reasoning, approval mode, budget, workspace
  root. Document the risk of two sessions editing the same workspace.
- Change model mid-session: a Norte engine holds ONE model fixed at build time, so
  keep an engine per (provider, model, capabilities), all sharing the same `Arc<dyn
  Store>`, `BlobStore`, tool registry, policies, layers and observers; a switch
  makes the session's next turn use another engine. Prove with a test that two
  engines over one store continue the same session. Rules: only at a turn
  boundary (phase Idle, or after resolving an approval); during a tool loop wait or
  refuse with a clear message; catch `Error::ContinuityBlocked` and explain it.
  Before switching, check history modalities against the new model's capabilities,
  re-check context size (the new window may be smaller: compact first or refuse),
  re-resolve reasoning (see Reasoning), and check that an active USD budget is
  still measurable (see Cost). Warn that the provider prompt cache does not carry
  over to a different model, so the next request re-bills the full prefix.

## Reasoning (follows what the model allows)
Reasoning controls are resolved per model from the catalog, never assumed.
- Source: models.dev `reasoning` (boolean) and `interleaved` (boolean or object
  naming `reasoning_content` / `reasoning_details`). Newer schema docs also show a
  structured reasoning-options object with min/max budget; its field names were not
  verified. Inspect the live JSON and use it if present. Map to Norte's
  `Reasoning { supported, can_disable, efforts, supports_budget, max_budget }`.
  Norte has only `max_budget`; enforce any minimum budget in Norte Code's clamp.
- When the catalog only says `reasoning: true`, do not guess: declare supported,
  expose only `Automatic`, and let `model_overrides.<model>.reasoning` in
  config.json declare efforts, budget support and `can_disable`.
- Norte's `check_reasoning` is strict: an undeclared effort, a budget above
  `max_budget`, or `Disabled` on a reasoning model that cannot disable is a typed
  `Error::Capability`. The UI must therefore offer ONLY valid choices: hide the
  control if unsupported; list Off only if `can_disable`; list only declared
  efforts; show a budget slider only up to `max_budget`.
- Scopes: session default (`set_reasoning`, /reasoning, config default) and
  per-message override (a field on send, `--reasoning`), passed through the turn
  builder's `.reasoning(..)`. `list_models` returns each model's available
  reasoning options so clients render them without catalog logic.
- On a model switch, re-resolve the current setting against the new model. If it is
  no longer valid, downgrade to `Automatic` or the nearest declared effort and
  emit a visible `ReasoningDowngraded {from, to, reason}` event. Norte never
  downgrades silently, and neither does Norte Code.
- Token headroom: Norte's latency benchmark had 2 of 5 rounds spend the whole 512
  output tokens on reasoning and return no answer text. With reasoning on, never set
  a tight `max_output_tokens`; detect a `Length` finish with no text and tell the
  user (offer to retry with more headroom). Reasoning tokens are part of billed
  output.
- Compaction summaries use the lowest effort or reasoning off, when the model
  allows it (`compaction.reasoning` in config.json).
- Replay: reasoning blocks carry continuity tokens that must go back unchanged
  within a tool loop, which is why a model switch mid-loop can be blocked. Check
  whether Norte's OpenAI-chat decoder handles `reasoning_content` and
  `reasoning_details` before adding any host quirk, and use `no_reasoning_effort`
  only if a live test shows the endpoint rejects the effort field.
- Display: stream `StreamDelta::Reasoning` into a collapsible panel; respect each
  part's visibility (Full, Summary, Redacted, Omitted).

## Compaction
Norte never compacts or edits history; its history is the path from root to head,
so messages cannot be dropped from the middle. Compaction is therefore a NEW Norte
session seeded from the old one, linked through Norte Code's own tables, while the
old session stays intact for audit and undo. The user still sees one logical
conversation.
- Trigger: when used context >= `compaction.threshold` (default 0.90, a fraction of
  the current model's context window). Used context = the provider-reported
  tokens of the most recent request (verify per wire whether cache read/write
  tokens are inside `input_tokens`; Norte counters are `Option`, None is not
  zero). If no usage was reported, fall back to a bytes/4 estimate flagged
  approximate. Evaluate before every model request and again after a model switch.
- Never split a tool call from its results (a Tool message holds all results of
  one assistant turn). Never compact in the middle of a tool message.
- Safe point: Norte's docs do not say how to stop the engine at an iteration
  boundary inside a long tool loop. SPIKE FIRST (on a `spike/` branch) and document
  the choice. Candidates: a Layer that fails the request before it is sent (session
  stays resumable per FR-ERR-5), then compact and resume; or drive the loop with
  shorter turns.
- Algorithm: (1) send a summarization request that reuses the existing transcript as
  the prefix (cache-friendly) plus one final instruction message, using the SAME
  model by default (config `compaction.model` may override, but a different model
  forfeits the cached prefix); (2) create a fresh session and `append_history`
  [system + project instructions, the summary message, the todo state, the last
  `keep_recent` messages verbatim] (valid only from Idle); (3) mark the summary
  message with host metadata outside the `norte.` prefix; (4) point the logical
  session at the new Norte session and record `compacted_from`.
- The summary must keep: the user's goal, decisions and constraints, files touched
  and why, commands run and failures, open todos, and the user's latest request.
- Safety: if the summary request fails, change nothing and report it. Never
  discard history without a successful summary. Compaction cost goes into the cost
  ledger and counts toward budgets. Manual `/compact` and `ClientRequest::Compact`
  exist. Emit CompactionStarted/Finished events. A new session id changes the
  provider cache key; this is intentional and should be shown to the user.

## Cost monitoring and budgets (everything opt-in)
Norte never prices, and neither does Norte Code. The only dollar figure is the one
the provider reports in the usage payload of each response (Norte keeps it
verbatim in `Usage::vendor`; OpenRouter uses the key `cost`). Never compute a
price from list prices, and ignore the catalog's `cost.*` fields entirely.
- A request whose payload carries no cost is counted and shown as "not reported",
  never priced. Token counters (always from the provider) are shown either way.
- Probe first: make one live call per provider and record in ARCHITECTURE.md
  whether its usage payload carries a cost. Expect OpenRouter to report it and the
  native OpenAI, Anthropic and Gemini APIs to report tokens only (unverified).
- Ledger in SQLite: one row per model request (session, turn, request index, model
  ref, token counters, reported cost or NULL). Roll up per message, per session,
  per day, with "n requests not reported" shown beside any USD total. Status bar,
  JSONL output and `norte-code cost` use the same numbers; display is a config switch.
- Budgets: scopes `message`, `session`, optional `global_daily`; units `usd`,
  `tokens`, `requests`; actions warn at X%, pause for approval, or stop. Set per
  session (config default, creation, /budget, set_budget) and per message
  (--budget, a field on send). None is enabled unless configured.
- A `usd` budget is only valid for a provider that reports cost. Validate at
  set time; when the user switches model mid-session to a provider that does not
  report cost, block the switch or ask to replace the budget with a token budget.
  Never run a USD budget that cannot be measured.
- Enforcement: Norte policies only gate tool calls, so enforce in a Norte `Layer`
  that checks the ledger's real spend before each model request and rejects with a
  typed `BudgetExceeded` reason, plus a post-request check. There is no worst-case
  pre-flight (it would need a price). The last request can overshoot by one
  request; say so in the docs. A stopped turn stays resumable ("raise budget and
  resume" or "stop"). Keep counting after errors: Norte commits and bills every
  request made before an iteration-limit error. Compaction requests count too.

## config.json
User config at the platform config dir (e.g. ~/.config/norte-code/config.json) with
an optional per-project overlay (.norte-code/config.json); project overrides user.
Ship a JSON Schema, a version field and strict validation with line-accurate
errors; warn on unknown keys. Every key has a documented default:
{
  "version": 1,
  "default_model": "provider/model",                    // from the catalog
  "providers": { "openai": {"api_key_env": "OPENAI_API_KEY"}, ..., "openrouter": {...} },
  "catalog": { "url": "https://models.dev/catalog.json", "ttl_hours": 24, "offline": false },
  "compaction": { "enabled": true, "threshold": 0.90, "keep_recent": 6,
                  "model": null, "reasoning": "lowest" },
  "reasoning": { "default": "auto" },                   // auto | off | {effort} | {budget}
  "approval_mode": "ask",                               // ask | auto-edit | yolo
  "max_iterations": null,                               // null = unlimited
  "extensions": { "openai.chat:provider": { "order": ["deepinfra"] } },
  "cost": { "show": true },
  "budgets": { "message": null, "session": null, "global_daily": null },
  "tools": { "web_fetch": false },
  "skills": { "enabled": true, "paths": [], "interop": true,
              "allow_scripts": false, "honor_allowed_tools": false,
              "max_skills": 128 },
  "model_overrides": { "provider/model": { "max_context": 0, "reasoning": {} } }
}
(The example values are illustrative. Define the final schema in code and
document it.)

## Using Norte correctly (from its guide)
- Norte has no agent loop: write it in `norte-code-agent`. Tool results are not
  stream events: read `engine.history()` when `RequestReconstructed` fires to show
  them. Log `RequestReconstructed` once per model request for cost and context
  accounting.
- Policies are not a sandbox: path re-rooting/escape refusal via `Transform` (it
  replaces the WHOLE arguments object, copy fields you keep), command allowlist,
  `RequireApproval` for writes/commands by default. `Deny` messages must say what
  shape would be accepted, because the model reads them and every retry is a billed
  request. Document that real containment needs an OS sandbox; implement an
  optional bubblewrap/seatbelt layer as a stretch goal. Keep early tool execution
  off if any policy reads `PolicyView::results`.
- Cache stability: no timestamps/ids/per-request data in cache-critical messages
  (use the ephemeral cache class), never rewrite history, surface
  `PrefixInvalidated` and cache read/write tokens to the user.
- Retry is host policy: bounded retry using `retry_after` for transient errors,
  via `resume` rather than resending the user message.
- Approvals are durable: a session parked at AwaitingApproval resumes after a
  restart. For `command`/`write`, keep an intent/done journal keyed by (session,
  call_id): Norte guarantees commit-at-most-once, not effect-at-most-once.

## Quality bar
No panics on external input (including the catalog and config files).
`#![forbid(unsafe_code)]`, typed errors. Hermetic tests with `norte-testing`
(`ScriptedModel`, `ProbeModel`, `RecordingObserver`): tool loop, approval park,
kill-and-resume, cancellation, policy deny/transform, store conformance, compaction
(trigger, failure leaves history untouched, tool-call pairing, new-session link),
model switch (two engines, one store; ContinuityBlocked path), reasoning resolution
for each descriptor shape plus the switch downgrade and the Length-with-no-text
case, budget layer (warn/stop/resume, USD budget on a non-reporting provider),
cost ledger (reported, not reported, mixed), catalog mapping and cache fallback
(offline, stale, corrupt download), config validation, skills discovery and
validation (every spec name rule, shadowing, malformed YAML, traversal attempts,
progressive disclosure sizes, script opt-in), TUI rendering with `TestBackend`.
Live-provider tests are `#[ignore]`d and key-gated. CI: fmt,
clippy -D warnings, tests, cargo-deny/audit.

## Milestones (finish and verify each before the next)
0. Repository setup exactly as in "Repository and git workflow": init on `dev`,
   workspace skeleton, CI, first commit, push.
1. Protocol, in-process service with in-memory store, headless CLI one-shot with
   streaming output, config loading, one provider end to end.
2. Catalog client + mapping, all four providers, model picker, mid-session model
   change, reasoning resolution.
3. Built-in tools + `norte-code-tools` (glob, grep, todo, ask_user), policies and
   approval modes; Agent Skills discovery, validation, progressive disclosure and
   the `skill` / `skill_resource` / opt-in `skill_script` tools.
4. SQLite store, multi-session, restart-resume of parked approvals.
5. Cost ledger and budgets (layer + UI), cost reports.
6. Compaction (spike on the safe point first), manual /compact.
7. TUI (ratatui).
8. Hardening: AGENTS.md loading, retries, sandbox option, docs.
At the end of each milestone: all checks green, CHANGELOG and README updated, the
commits clean and pushed to `dev`, an annotated `milestone-N` tag pushed, and a
short report listing what is verified by tests and what is not.

## Non-goals for v1
Multi-agent orchestration, RAG/vector memory, web UI, the network server transport,
a plugin system or a tool ABI, OAuth/subscription login (API keys only). Agent
Skills are files loaded by the host, not a plugin system.

## Deliverables
Code, tests, README with an architecture diagram, ARCHITECTURE.md (protocol,
transport trait, compaction and model-switch design, cost sources and the
per-provider cost probe results), a documented config.json schema, a clean `dev`
history on https://github.com/JeanVydes/NorteCode.