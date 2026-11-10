# AGENTS.md

Rules for any AI agent working in this repository. Rust is the primary language, but every rule applies to any language unless stated otherwise. When a rule conflicts with a quick hack, the rule wins.

## 1. Architecture and file structure

- No god files. A file has one responsibility and one reason to change.
- No god structs, god traits or god modules. If a type owns unrelated concerns, split it.
- Soft limits: a file above ~300 lines or a function above ~40 lines must be justified or split.
- One public concept per file. Name the file after the main type it defines.
- Organize by domain (feature or bounded context) first, by technical layer second.
- Keep dependencies pointing in one direction. Domain code must not depend on infrastructure code (HTTP, DB, CLI, filesystem).
- Prefer small modules with a narrow public surface. Everything is private by default, expose the minimum.
- Do not create `utils`, `helpers`, `common` or `misc` dumping grounds. Name modules after what they do.

### Crate layout

Dependencies point one way only:

```
norte-code-wire        shapes only: serde derives, no socket, no runtime, no DB.
                       Compiles to wasm. Owns every type that travels.
  norte-code-protocol  the server's model: store, config, catalog, costs, plans,
                       skills, tools. Imports wire, never re-exports it.
  norte-code-client    the client protocol: frame decoding, SSE reading and the
                       native RAEP HTTP adapter (feature `native`). Every
                       client decodes through it; wasm clients compile it
                       without default features and supply only a transport.
  norte-code-agent     the agent loop and services.
  norte-code-plugin-host  dynamic plugin ABI and the command registry plugins
                       declare into. The only crate allowed to use `unsafe`.
  norte-code-raep      the server process and HTTP host.
  norte-code-app       the Dioxus client; depends on client, wire and raep-types.
```

Rules that follow from this:

- A type that crosses a process boundary lives in `norte-code-wire`, and only there. No crate re-exports it and no crate defines a second copy. Import `norte_code_wire::...` directly.
- `norte-code-protocol` must stay client-agnostic: no slash-command parsing, no presentation, no plugin command surface. Those live in `norte-code-plugin-host`.
- A client never decodes RAEP payloads or spells a URL on its own. It uses `norte-code-client`: one decoder, one SSE reader and one `Endpoint` catalog, so a wire change cannot half-land and a path cannot be built twice.
- A shape that crosses the wire lives in `norte-code-wire`. `norte-code-client` binds those shapes to methods and paths; it does not redefine them.
- Anything that needs a socket, a database, a clock or a filesystem does not belong in `norte-code-wire`.
- When moving a type, move its pure methods too. A method that needs the engine becomes an extension trait in the engine-side crate.
- A payload that the host serializes and a client deserializes is one struct with both derives, not two mirror definitions.

## 2. Types and data modeling

### No opaque strings
- Never pass meaningful data around as a raw `String` or `&str`. Wrap it in a newtype: `UserId`, `Email`, `ApiKey`, `FilePath`, `Url`.
- Validate on construction (`TryFrom`, `FromStr`, or a `new` returning `Result`) so that an invalid value cannot exist.
- Identifiers of different entities must be different types. `UserId` and `OrderId` are never interchangeable.

### Enums instead of strings
- If a value belongs to a closed set (status, kind, mode, role, event type, error category), it is an `enum`, never a string or an integer constant.
- Parse strings into enums at the boundary (`FromStr`, `serde`), and work with the enum everywhere inside.
- Use `match` without a wildcard arm when possible, so adding a variant forces the compiler to flag every place that needs updating.
- Booleans that represent states or modes should be enums. `Visibility::Public` is better than `is_public: bool`, and two or more booleans that interact should always become an enum.

### Structs instead of dynamic data
- Never use `json!`, `serde_json::Value`, `HashMap<String, _>` or `Map<String, Value>` to represent data whose shape is known. Define a struct and derive `Serialize` and `Deserialize`.
- Request bodies, responses, config, events, messages and database rows are all typed structs.
- Dynamic values are only acceptable when the shape is truly unknown at compile time (for example, proxying arbitrary payloads). Isolate them at the boundary and document why.
- Use `#[serde(rename_all = "...")]` and `#[serde(tag = "...")]` on typed enums and structs instead of building keys by hand.

### General modeling rules
- Make illegal states unrepresentable. Prefer types that cannot express an invalid combination over runtime checks.
- Use `Option` and `Result` honestly. No sentinel values such as `""`, `-1` or `0` meaning "missing".
- Avoid primitive obsession: units, durations, amounts and percentages get their own types (`Duration`, `Money`, `Percent`).
- Prefer the builder or typestate pattern when a type has many optional fields or an ordered construction.

## 3. Building strings and structured output

- Do not scatter `format!`, string concatenation or `join` to construct paths, URLs, queries, commands, prompts, messages or identifiers.
- If building something requires `format!`, move that logic into a dedicated factory, builder or formatter type (`UrlBuilder`, `QueryBuilder`, `CommandFactory`, `PromptRenderer`) with a typed API.
- Implement `Display` on types that have a canonical text form, and use that instead of ad hoc formatting.
- Never build SQL, JSON, HTML, shell commands or URLs by string interpolation. Use a query builder, serializer, template engine or `Command` API with typed arguments.
- Log and error messages with simple interpolation are fine. This rule targets construction of data and protocol output.

## 4. Abstraction and reuse

- Prefer abstraction over repetition. If the same shape, logic or constant appears a second time, extract it. At the third time it is mandatory.
- Share types across modules and crates. Do not redefine the same struct, enum or constant in several places. Put shared types in a single owner module (or a `types`/`domain` crate) and import them.
- Use traits and generics to express common behavior instead of copy-pasting similar implementations.
- Use `From`, `TryFrom`, `AsRef` and `Into` for conversions instead of repeated manual mapping code.
- Magic numbers and repeated literals become named constants, or better, typed configuration.
- Abstraction must pay for itself. Do not introduce a trait with a single implementation and no foreseeable second one, and do not add layers "just in case".

## 5. Tests

- Never put test modules in the same file as production code. No inline `#[cfg(test)] mod tests { ... }` blocks.
- Unit tests live in separate files next to the code they cover (for example a sibling `foo_tests.rs` wired with `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;`) or in the crate-level `tests/` directory for integration tests. A module tree uses `#[cfg(test)] mod tests;` pointing at `tests.rs` in the same directory, never an inline block.
- Tests use the same typed structs and enums as production code. No hand-written JSON strings or stringly-typed fixtures when a struct exists.
- Share fixtures and builders through a dedicated test support module instead of duplicating setup in each test.
- Test names describe behavior: `rejects_expired_token`, not `test1`.
- Every bug fix comes with a test that fails before the fix.

## 6. Comments

- Avoid excessive comments. Code should explain itself through clear names and types.
- Comment only the non-obvious parts: invariants, tricky algorithms, non-trivial trade-offs, safety requirements (`// SAFETY:` for `unsafe`), external constraints.
- Explain the why, never the what. If a comment restates the code, delete it.
- Comments describe the present state of the code only. NEVER write comments about the past or about change history. Forbidden examples: "changed from X", "previously did Y", "refactored", "fixed bug", "removed old logic", "no longer", "legacy", "temporary workaround until...", "new implementation". Version history belongs in git.
- Do not leave commented-out code. Delete it.
- Keep comments short, ideally one line. No banners, separators or decorative blocks.
- Use doc comments (`///`) for the public API only, and keep them concise and accurate.
- TODO comments are allowed only if they describe a concrete pending task in the present tense and reference an issue when one exists.

## 7. Style and writing rules

- Never use em dashes anywhere: code, comments, docs, commit messages, or responses. Use a comma, colon, parentheses or a period instead.
- Never use emojis in any context: code, comments, logs, error messages, docs, commits, or responses.
- Use plain ASCII punctuation in code and comments.
- Names are descriptive and consistent with the domain vocabulary. Avoid abbreviations unless they are universal (`id`, `url`, `http`).
- Follow the language's idiomatic conventions and formatter (`rustfmt`, `clippy` with warnings denied).

## 8. Error handling

- Use typed errors, not strings. Define error enums (for example with `thiserror`) with one variant per failure cause.
- Never return `Result<_, String>` or `Box<dyn Error>` from library code. Reserve `anyhow` for application entry points.
- No `unwrap()`, `expect()` or `panic!` in production paths. They are allowed in tests and for provably impossible states, with a justification.
- Add context when propagating errors, and keep the original source attached.
- Handle errors at the layer that can act on them. Do not swallow errors silently.

## 9. Rust specifics

- Prefer borrowing over cloning. Do not `clone()` just to satisfy the borrow checker without understanding why.
- Prefer iterators and combinators over manual index loops when they read clearly.
- Use `impl Trait` and generics for static dispatch by default, `dyn Trait` only when runtime polymorphism is needed.
- Derive standard traits (`Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`, `Default`) where they make sense, and implement them manually only when needed.
- Mark types `#[non_exhaustive]` only on public API enums and structs that are expected to grow.
- Keep `unsafe` out unless strictly required. Wrap it in a small, safe, documented abstraction.
- Keep `Cargo.toml` dependencies minimal. Do not add a crate for something the standard library already does.
- Async code must not block the runtime. Use async-aware primitives and `spawn_blocking` when needed.

## 10. Workflow for agents

- Read the surrounding code and match existing conventions before writing new code.
- Make the smallest change that solves the problem. Do not refactor unrelated code in the same change.
- Before finishing, run formatter, linter and tests. Do not report success if any of them fail.
- Do not invent APIs, crates or fields. Verify they exist.
- When a request conflicts with these rules, follow the rules and mention the conflict briefly instead of silently breaking them.
- Keep commit messages short, imperative and in the present tense, with no emojis and no em dashes.

## Quick checklist

Before submitting, verify that the change has:

- No god files, god structs or god modules
- No raw strings where a newtype or enum belongs
- No `json!` or `Value` where a struct belongs
- No scattered `format!` building data, a factory or builder instead
- No inline test modules in production files
- No duplicated types or logic, shared abstractions instead
- No excessive comments, none about the past, none that restate code
- No em dashes and no emojis