# Agent and contributor rules for mediaprobe

These rules apply to every automated agent and every human contributor.

## What this repository is

A pure-Rust reader of container and stream metadata for media files: the
container, its tracks and their codecs, HDR signalling, languages and
chapters, read within a fixed budget and with no external tools. The
published crate currently reserves the name and has no API; the parser lands
here once its public API is settled.

## Correctness rules

- **No panics on any input.** Malformed, truncated or hostile input returns
  an error; it never panics, never reads or writes out of bounds and never
  does unbounded work or unbounded reads.
- **`unsafe` is allowed, under rules.** Every `unsafe` block carries a
  `// SAFETY:` comment proving it sound: the invariant it relies on and the
  check that established it. Code with `unsafe` is covered by Miri wherever
  Miri can run it; a test that Miri cannot run is ignored under `cfg(miri)`,
  never the lane.
- **Fuzzing.** Every public entry point that reads caller-supplied bytes has
  a fuzz target under `fuzz/`, added in the same change as the entry point.

## Tests

- A test gives the same result on a slow, loaded or shared runner as on an
  idle workstation. No fixed sleeps, no deadlines, no timeouts and no
  elapsed-time assertions.
- Fixtures use invented names only. No real media titles, release names or
  file names in fixtures, tests or examples.
- No binary files are committed. CI refuses them.

## Versions and pull requests

- One pull request per version. Work for a version is folded into that
  version's single branch, not stacked as per-feature pull requests.
- The version in a pull request is the latest version published on crates.io
  plus one increment, never more. An unpublished version heading is not a
  baseline; collapse it into the next one rather than bumping again.
- Any code change bumps the crate version, `Cargo.lock` and `CHANGELOG.md` in
  the same change. `CHANGELOG.md` headings are versions only.

## Repository hygiene

- Commits are SSH-signed. Never pass `--no-gpg-sign` and never set
  `commit.gpgsign=false`. If signing fails, leave the change uncommitted.
- No `Co-Authored-By` or other attribution trailers in commit messages.
- Never run destructive git working-tree operations (`checkout --`,
  `restore`, `reset --hard`, `clean`, `stash`) in a shared checkout.
- Branch names use gitflow prefixes: `feature/…`, `bugfix/…`, `hotfix/…`.
  Worktrees go under `.worktrees/`.
- Agents do not push unless told to. The maintainer pushes.
- Do not create, edit or delete Markdown files outside the task at hand.
- `cargo fmt --all`, `cargo clippy --locked --workspace --all-targets
  --all-features -- -D warnings` and `cargo test --locked --all-features` must
  pass before a commit is proposed.
