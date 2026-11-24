# Contributing

Thanks for looking. A few things before you open a pull request:

- Read [AGENTS.md](../AGENTS.md); the rules there apply to humans too.
- A new public entry point comes with a fuzz target.
- Hydrate the fixtures with `cargo xtask test-corpus hydrate`, then run
  `cargo fmt --all --check`,
  `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`,
  `cargo nextest run --locked --workspace --all-features` and
  `cargo test --doc --locked --workspace --all-features` locally.
- [docs/testing.md](../docs/testing.md) explains the test and CI lanes, and
  [docs/test-corpus.md](../docs/test-corpus.md) how to add or change a
  fixture. Fixtures are generated, never committed, and use invented names.
