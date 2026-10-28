# AGENTS.md

Conventions for anyone — human or agent — working on Norte Code. Read this before
touching anything.

## What this repository is

Norte Code is a host on top of the Norte agent engine. Norte lives in its own
repository and is consumed as a git dependency: never vendor or fork it here. A
change that belongs to Norte is made there — with its own tests, guide and commit
— and picked up here by bumping the dependency. The API we build on is Norte's
guide (`guide/src`) and its source (`norte-core`, `norte-tools`); when this file
and that code disagree, the code is right.

## Git workflow

- `dev` is the working branch. Never create `main`; it is reserved for releases.
- Remote `origin` is `https://github.com/JeanVydes/NorteCode`.
- Never force-push, and never rewrite history that has been pushed.
- Commits are [Conventional Commits](https://www.conventionalcommits.org/) scoped
  to the crate: `feat(catalog): read the catalogue from two files`. Types in use:
  `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `build`, `ci`, `perf`.
- One logical change per commit. Do not mix a refactor with a feature, and do not
  commit `WIP` or `fix typo`. Fixup and autosquash only on local, unpushed commits.
- Tag the end of a milestone `milestone-N` (annotated) and push it.
- Do not open pull requests unless asked. No signatures, no agent attribution:
  the work is the repository's, not the tool's.

### The subject line

At most **68 characters**, imperative, and it must stand on its own: it is what
`git log --oneline` shows, and the body is one flag away from invisible. Do not
pad to reach the limit — a shorter subject reads better than one that needs
counting.

Prefer the **why** when the *what* is already in the diff:

```
fix(agent): replay a session's history when the client asks for it
```

says more than *"add a replay flag to SendMessage"*, which restates the diff.
`update`, `improve` and `refactor` name nothing, and a subject that names nothing
is not a subject.

### The body

The body explains **why the change exists**, not what the diff contains.

- **Problem before solution.** What was broken or impossible, and what the
  alternatives were.
- **Do not call a change obvious, trivial, small or clear.** It is obvious now;
  it was not before, or there would be no commit.
- **A change that removes a tradeoff names it.** *"Slower but simpler"* or
  *"duplicates code to remove the coupling"* is what a reader needs a year from
  now.
- **Name the road not taken.** Moving the runtime to its own crate, adding a
  transport, letting a call detach: the commit is where the reason belongs, not a
  document that will lag behind it.
- **A change that adds something says how to reach it and how to turn it off**:
  the flag, the config key, the command.
- **Say how it was verified**, with the command — see
  [Running commands](#running-commands):

  ```
  cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace
  ```

### Every commit compiles

Every commit must pass `cargo fmt --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings` and
`cargo test --workspace`, so history stays bisectable. Tests ship with the
feature.

- Update `CHANGELOG.md` (Keep a Changelog) in the commit that adds a
  user-visible change.
- Commit each finished, verified step. Larger or risky work goes on a
  short-lived `feat/<name>` or `spike/<name>` branch cut from `dev`, merged with
  `--no-ff` once the checks pass, then deleted. Small steps commit straight to
  `dev`.

## Hygiene

- `.gitignore` covers `target/`, secrets, `*.db`, caches, logs and editor files.
- Never commit API keys, auth files, SQLite databases or catalogue caches. The
  development keys live in a `.env` outside the tree; leave them there.
- `Cargo.lock` **is** committed: this is an application.
- Review `git diff --staged` before every commit, and never `git add -A` without
  reading what it picked up: scratch files belong outside the tree. Run a secret
  scan (`gitleaks` when available) before pushing.

## Running commands

- **Never run a command that can wait on input.** A call that blocks on a TTY
  hides the failure and stains the session. Reach the network with
  `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=/bin/true` and `SSH_ASKPASS=/bin/true`,
  and give every build, test or network command an explicit timeout. Fail fast
  and report the error; never retry a call that hung.
- **Set `HOME` for anything that needs the user's git credentials.** A sandboxed
  or detached process can start with `HOME` unset, and git then finds neither
  `~/.gitconfig` nor the credential helper it names: the push fails with
  `No anonymous write access` or `$HOME not set` even though the user is logged
  in. `HOME=/home/<user> git push origin dev` is the fix — not a token.
- **Push with the user's credentials.** If a push needs credentials, report the
  exact error and stop. Never put a token in a remote URL, never write
  credentials to disk, never prompt.
- **Check the disk before a long build.** `cargo` writes gigabytes into
  `target/`, and a full disk truncates a file mid-write: a formatter or compiler
  that runs out of room leaves a half-written source file, which is worse than a
  failed build. `df -h` first; prefer `cargo clean -p <crate>` or removing
  `target/debug/incremental` over deleting source or caches you did not create.

## Code

- **An enum, always, when the set is closed.** A `String` that holds one of three
  things is three bugs waiting: a typo compiles and runs, a `match` grows a
  catch-all arm that swallows the value nobody meant, and nothing tells a reader
  what the values are. Name the set, derive `Deserialize` on it, and let a bad
  name fail where the file is read — with the file, the line and every name that
  would have worked. The exceptions are a name a person chose (a session title,
  a model id, a path) and a value whose set is genuinely open.
- A `match` over an enum has no `_` arm when the enum is local: a variant added
  later must be a compile error, not a silent fall into the default case. That is
  the whole reason the enum exists.
- `#![forbid(unsafe_code)]`; typed errors; no panics on external input, including
  the catalogue and config files.
- `missing_docs` is denied workspace-wide: document every public item.
- Library crates never write to stdout; the CLI and TUI own output.
- Prefer hermetically testable code. Tests use `norte-testing` (`ScriptedModel`,
  `ProbeModel`, `RecordingObserver`) instead of the network; live-provider tests
  are `#[ignore]`d and key-gated.
- Keep the protocol crate free of Norte types: the wire is versioned serde types,
  mapped in one place.
- Anything that reaches a prompt is bounded **and says what it left out.** A
  model cannot ask for what it cannot see, so a truncated list that hides the
  truncation is worse than a short one.

## Providers and cost

- The catalogue is two files a person owns, beside the configuration:
  `providers.json` (who serves models, over which wires) and `models.json` (a
  flat list, each model naming its provider). **Nothing is fetched, cached or
  bundled.** A file that is missing is an error naming it, and a plugin that
  builds the same two shapes hands them to `Catalog::new`.
- A provider has `wires`, each a base URL plus the schema it speaks. The three
  schemas are `openai` (chat completions, the shape every compatible vendor
  speaks), `anthropic` and `google`. `--wire` picks one; the preferred order is
  OpenAI, then Anthropic, then Google. The Responses API is deliberately
  unreachable — this host speaks the shape the three share.
- A wire's credential header follows from its schema and from nothing else. A
  configuration that could change it could only make the request wrong.
- The catalogue is the sole authority on a model. The configuration does not
  override a model's limits, modalities or reasoning.
- Provider-reported cost only. Never derive a price from a list price; the
  catalogue may carry `pricing` for reference and it is never a source of cost.
- OpenAI, Anthropic, Gemini and OpenRouter (OpenAI-compatible chat) are the v1
  providers. Keys come from the environment, named by the provider's
  `environment_keys`, are held in Norte's `Secret`, and never reach a model
  event, a log or a child process.
- The command tool clears the environment: pass only what is needed, and register
  every configured key with `with_secrets` so it is redacted.
