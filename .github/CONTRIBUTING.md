# Contributing

Thanks for looking. A few things before you open a pull request:

- Read [AGENTS.md](../AGENTS.md); the rules there apply to humans too.
- A new public entry point comes with a fuzz target.
- Run `cargo fmt --all`, `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
  and `cargo test --locked --all-features` locally.
