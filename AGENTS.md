# AGENTS.md

Conventions for contributors (human or automated) to Norte Code. If you are an
agent, read this before you touch anything.

## What this repository is

Norte Code is a host built on top of the Norte agent engine. Norte is a separate
repository and a git dependency; **never modify, vendor or fork Norte from here**.
If you need a Norte change, stop and report it instead of patching around it.
The API we depend on is defined by Norte's guide (`guide/src`) and its source
(`norte-core`, `norte-tools`); those are the source of truth.

## Git workflow

- `dev` is the working branch. Do not create `main`; it is reserved for releases.
- `git init -b dev`, remote `origin` = `https://github.com/JeanVydes/NorteCode`.
- Never force-push and never rewrite pushed history.
- Commits are [Conventional Commits](https://www.conventionalcommits.org/) with
  the crate as the scope, for example `feat(catalog): cache models.dev with ETag`.
  Types in use: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `build`,
  `ci`, `perf`.
- Subject line: imperative, at most 72 characters. The body explains the *why*
  when it is not obvious. One logical change per commit: do not mix a refactor
  with a feature, and do not commit `WIP` or `fix typo` noise. On **local,
  unpushed** commits only, fix up and autosquash.
- Every commit must compile and pass `cargo fmt --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings` and
  `cargo test --workspace`. Run them before committing, so history stays
  bisectable. Tests ship in the same commit as the feature.
- Update `CHANGELOG.md` (Keep a Changelog style) in the commit that adds a
  user-visible feature.
- Commit as each feature or sub-feature is finished and verified. Larger or
  risky work goes on a short-lived `feat/<name>` or `spike/<name>` branch cut
  from `dev`, merged into `dev` with `--no-ff` after the checks pass, then
  deleted. Small steps commit straight to `dev`.
- Tag the end of each milestone with an annotated tag `milestone-N` and push it.
- Do not open pull requests unless asked.

## Hygiene

- `.gitignore` covers `target/`, secrets, `*.db`, caches, logs and editor files.
- Never commit API keys, auth files, SQLite databases or catalog caches.
- `Cargo.lock` **is** committed (this is an application).
- Review `git diff --staged` before every commit. Run a secret scan
  (`gitleaks` when available) before each push.
- Push with the user's existing git credentials. If a push fails, report the
  exact error and stop. Never put a token in a remote URL and never write
  credentials to disk.

## Code

- `#![forbid(unsafe_code)]`; typed errors; no panics on external input
  (including the catalogue and config files).
- `missing_docs` is denied at the workspace level: document every public item.
- Library crates never write to stdout; the CLI and TUI own output.
- Prefer hermetically testable code. Tests use `norte-testing`
  (`ScriptedModel`, `ProbeModel`, `RecordingObserver`) instead of the network.
  Live-provider tests are `#[ignore]`d and key-gated.
- Keep the protocol crate free of Norte types: the wire is versioned serde
  types, mapped in one place.

## Providers and cost

- Provider-reported cost only. Never compute a price from a list price, and
  ignore the catalogue's `cost.*` fields.
- OpenAI, Anthropic, Gemini and OpenRouter (OpenAI-compatible chat) are the v1
  providers. Keys come from the environment or from config, are held in Norte's
  `Secret`, and never reach a model event, a log or a child process.
- The command tool clears the environment; pass only what is needed, and
  register every configured key with `with_secrets` so it is redacted.
