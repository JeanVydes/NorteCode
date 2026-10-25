# AGENTS.md

Conventions for contributors (human or automated) to Norte Code. If you are an
agent, read this before you touch anything.

## What this repository is

Norte Code is a host built on top of the Norte agent engine. Norte lives in its
own repository and is consumed as a git dependency; do not vendor or fork it
inside Norte Code. A change that belongs to Norte is made in the Norte
repository, with its own tests, guide and commit, and then picked up here by
bumping the dependency. The API we depend on is defined by Norte's guide
(`guide/src`) and its source (`norte-core`, `norte-tools`); those are the source
of truth.

## Git workflow

- `dev` is the working branch. Do not create `main`; it is reserved for releases.
- `git init -b dev`, remote `origin` = `https://github.com/JeanVydes/NorteCode`.
- Never force-push and never rewrite pushed history.
- Commits are [Conventional Commits](https://www.conventionalcommits.org/) with
  the crate as the scope, for example `feat(catalog): cache models.dev with ETag`.
  Types in use: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `build`,
  `ci`, `perf`.
- One logical change per commit: do not mix a refactor with a feature, and do not
  commit `WIP` or `fix typo` noise. On **local, unpushed** commits only, fix up
  and autosquash.
- Tag the end of each milestone with an annotated tag `milestone-N` and push it.
- Do not open pull requests unless asked. No commit signatures and no agent
  attribution: the work is the repository's, not the tool's.

### The subject line

At most **68 characters**, imperative, and it must stand on its own — it is what
`git log --oneline` shows, and the body is one flag away from invisible.

Keep it under the 72-character convention rather than at the edge of it: 74 and
75 characters read as 70 until someone counts. Three commits in this repository
missed it by two and three characters for exactly that reason.

Prefer the **why** in the subject when the *what* is already in the diff:

```
fix(agent): replay a session's history when the client asks for it
```

says more than *"add a replay flag to SendMessage"*, which is the diff restated.
A subject that says *"update"*, *"improve"* or *"refactor"* without naming the
change is not a subject.

### The body

The body explains **why the change exists**, not what the diff contains.

- **State the problem before the solution.** What was broken, or what could not
  be done, and what the alternatives were when there were any.
- **Do not claim a change is obvious, trivial, small or clear.** It is obvious
  now and it was not before; if it were, there would be no commit. A file that
  slipped into three commits reads as an obvious cleanup and was not one.
- **A change that removes a tradeoff says which one.** *"This is slower but
  simpler"* or *"this duplicates code but removes the coupling"* is the sentence
  a reader needs a year from now.
- **Say how it was verified**, with the command. The rule is in
  [Running commands](#running-commands), and a reader should not have to guess
  whether it was:

  ```
  cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace
  ```

- **A change that adds something says how to use it and how to turn it off**: the
  config key, the flag, the command. A feature nobody knows how to reach is one
  nobody uses.
- **A decision names the road not taken.** This repository has had several — move
  the runtime to its own crate, expose a second transport, let a call be
  detached — and the commit that made one is where the reason belongs, not a
  document that will lag behind it.

### Every commit compiles

Every commit must compile and pass `cargo fmt --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings` and
`cargo test --workspace`. Run them before committing, so history stays
bisectable. Tests ship in the same commit as the feature.

- Update `CHANGELOG.md` (Keep a Changelog style) in the commit that adds a
  user-visible feature.
- Commit as each feature or sub-feature is finished and verified. Larger or
  risky work goes on a short-lived `feat/<name>` or `spike/<name>` branch cut
  from `dev`, merged into `dev` with `--no-ff` after the checks pass, then
  deleted. Small steps commit straight to `dev`.

## Hygiene

- `.gitignore` covers `target/`, secrets, `*.db`, caches, logs and editor files.
- Never commit API keys, auth files, SQLite databases or catalog caches.
- `Cargo.lock` **is** committed (this is an application).
- Review `git diff --staged` before every commit. Run a secret scan
  (`gitleaks` when available) before each push.
- Never `git add -A` without looking at what it picked up. Scratch files belong
  outside the tree, and a commit that carries one is a commit that has to be
  followed by a removal.

## Running commands

- Never run a command that can wait on input without disabling the prompt. A
  command that blocks on a TTY hides the failure and stains the whole session:
  set `GIT_TERMINAL_PROMPT=0` for anything that reaches the network,
  `GIT_ASKPASS=/bin/true` and `SSH_ASKPASS=/bin/true` with it, and pass an
  explicit timeout to every build, test or network command. Fail fast and
  report the error; do not retry the same hanging call.
- Set `HOME` when a command needs the user's git credentials. A sandboxed or
  detached process can start with `HOME` unset, and git then finds neither
  `~/.gitconfig` nor the credential helper it names: the push fails with
  `No anonymous write access` or `$HOME not set` even though the user is logged
  in. `HOME=/home/<user> git push origin dev` is the fix, not a token.
- Push with the user's existing git credentials. If a push fails, report the
  exact error and stop. Never put a token in a remote URL and never write
  credentials to disk. Authentication is the user's, so a push that needs
  credentials is reported, not prompted for.
- Check the disk before a long build. `cargo` writes gigabytes into `target/`,
  and a full disk truncates a file mid-write: a formatter or a compiler that
  runs out of room leaves a half-written source file behind, which is far worse
  than a failed build. `df -h` first, and prefer `cargo clean -p <crate>` or
  removing `target/debug/incremental` over deleting source or caches you did not
  create.

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
- Anything that reaches a prompt is bounded, and says what it left out. A model
  cannot ask for what it cannot see, so a truncated list that does not admit it
  is truncated is worse than a short one.

## Providers and cost

- Provider-reported cost only. Never compute a price from a list price, and
  ignore the catalogue's `cost.*` fields.
- OpenAI, Anthropic, Gemini and OpenRouter (OpenAI-compatible chat) are the v1
  providers. Keys come from the environment or from config, are held in Norte's
  `Secret`, and never reach a model event, a log or a child process.
- The command tool clears the environment; pass only what is needed, and
  register every configured key with `with_secrets` so it is redacted.
