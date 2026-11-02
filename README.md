# Norte Code

A terminal-based agentic coding tool, in Rust.

## Installing

```sh
curl -fsSL https://raw.githubusercontent.com/JeanVydes/NorteCode/dev/scripts/install.sh | sh
```

## Running

```sh
norte-code
```

## Documentation

- **The guide** — [the hosted guide](https://jeanvydes.github.io/NorteCode/)
  explains the service, the protocol, sessions, tools, approvals, compaction,
  budgets and the CLI. It is written in Markdown and built with
  [mdBook](https://rust-lang.github.io/mdBook/); run `mdbook serve guide` to read
  it locally, and CI publishes it to GitHub Pages from `dev`.
- **Configuration** — [`CONFIG.md`](CONFIG.md) is the full schema; the guide has
  a compact reference.
- **Rustdoc** — every crate documents its public surface:

  ```sh
  cargo doc --workspace --no-deps --open
  ```

## License

MIT. See [LICENSE](LICENSE).
