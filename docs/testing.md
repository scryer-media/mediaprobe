# Testing mediaprobe

The crate has one test harness, run with `cargo nextest`. Doctests run
with `cargo test --doc`, which nextest cannot run. Every Cargo command
takes `--locked`.

## What runs where

| Workflow | When | Jobs |
| --- | --- | --- |
| `ci.yml` | every pull request and push to `main` | `hygiene`, `fmt`, `clippy`, `docs`, `test` (Linux, macOS and Windows on x86_64 and aarch64, plus a scalar-only Linux lane), `no-default-features`, `msrv`, `package`, `corpus-config`, `fuzz-harness`, and `ci-complete` over them all |
| `assurance.yml` | every pull request and push to `main` | `miri-*`, `asan-*`, `valgrind`, `simd-differential-*`, `fuzz-short`, `corpus-large`, and `assurance-complete` |
| `security.yml` | pull requests, pushes to `main`, weekly | `cargo-audit` on the crate's, the fuzz crate's and xtask's lockfiles; `zizmor` on the workflows |
| `codeql.yml` | pull requests, pushes to `main`, weekly | CodeQL |
| `cflite-pr.yml` | pull requests touching the parser or the fuzz crate (switched off) | ClusterFuzzLite, ten minutes, the targets the change reaches |
| `cflite-batch.yml` | nightly (switched off) | ClusterFuzzLite's long batch run |
| `fuzz-extended.yml` | by hand; weekly (switched off) | each target for 30 minutes in its own job |
| `release-fuzz.yml` | by hand, before a release | every target under AddressSanitizer for up to 12 hours |
| `test-corpus-publish.yml` | by hand, on `main` | generates, signs and publishes the fixture corpus |

`docs/unsafe-validation.md` covers the Miri, AddressSanitizer, Valgrind and
SIMD-differential lanes. `docs/test-corpus.md` covers the fixtures.

## Running the tests

Most tests read the fixture corpus, so hydrate it first:

```sh
cargo xtask test-corpus hydrate
cargo nextest run --locked --workspace --all-features --no-fail-fast
cargo test --doc --locked --workspace --all-features --no-fail-fast
cargo nextest run --locked --no-default-features --no-fail-fast
```

The parser's unit tests embed fixtures, so the test targets of the default
feature set do not compile without the unit corpus. The
`--no-default-features` build (the data model and serde only) needs none.

| Test | What it checks |
| --- | --- |
| unit tests under `src/` | each container and codec parser, the scanners, the model's serialisation |
| `tests/media_fixtures.rs` | every fixture against its expectations in `tests/media-fixtures.toml`, with `tests/media-known-gaps.toml` as the ratchet |
| `tests/budget.rs` | the read and I/O budgets on large files, and that probe cost does not grow with file length |
| `tests/robustness.rs` | truncated and bit-flipped copies of every small fixture: no panic |
| `tests/tag_limits.rs` | a Matroska tag flood keeps a bounded set of tags and a bounded heap |
| `tests/unsafe_scanners.rs` | the accelerated scanners against the scalar path (needs `--features fuzzing`) |
| `tests/model_json.rs`, `tests/warning_codes.rs`, `tests/native_tests.rs`, `tests/harness_selftest.rs` | the JSON shape, the warning codes, the native-only paths, the harness itself |
| `tests/ffprobe_parity.rs` | compares with FFprobe; ignored unless run by hand |

Environment variables the harness reads:

| Variable | Effect |
| --- | --- |
| `MEDIAPROBE_FIXTURE_PROFILE` | `unit` (default), `large` or `all`: which fixtures the corpus tests read |
| `MEDIAPROBE_MEDIA_ROOT` | where the fixtures are, if not `tests/media` |
| `MEDIAPROBE_BLESS_KNOWN_GAPS` | `1` rewrites `tests/media-known-gaps.toml` from this run; review the diff |
| `MEDIAPROBE_PARITY_FIXTURES`, `MEDIAPROBE_PARITY_REPORT` | the FFprobe parity run's input and report |

Five tests are ignored on purpose: two FFprobe references and the parity
harness, which need FFprobe, and two microbenchmarks, which mean something
only in a release build (`--release --run-ignored only --no-capture`).

## Fuzzing

`fuzz/` holds 32 cargo-fuzz targets: one per container and codec parser,
the probe and diagnosis entry points at each depth, the scanner
differential, and structure-aware targets whose mutators edit MP4 boxes,
EBML elements, transport-stream packets and parameter sets rather than raw
bytes. `fuzz/dicts/` has a dictionary per format.

```sh
cargo xtask test-corpus hydrate --profile fuzz-seeds
fuzz/scripts/seed-corpora.sh               # copies fixtures into fuzz/corpus/<target>
fuzz/scripts/smoke.sh 20000 60             # every target, briefly, as CI does
FUZZ_TARGETS=container_mkv fuzz/scripts/smoke.sh -1 1800
cargo +nightly fuzz run container_mkv      # by hand
```

A crash found by any lane is fixed and recorded in
`fuzz/src/regressions.rs` as a byte literal with the target it reproduces
on. `fuzz/tests/regressions.rs` replays every one through the target's own
entry point; the `fuzz-harness` job in `ci.yml` runs it, so a fixed crash
stays fixed.

### The release campaign

`release-fuzz.yml` is dispatched by hand before a release. Inputs: `hours`
(1 to 12, default 12), `targets` (space-separated; all 32 when empty), and
`seed-run` (an earlier run whose final corpus seeds this one). A plan job splits the targets into groups of four, one per core;
each group is one job per stage, two stages, and stage 2 starts from the
corpus stage 1 grew. Every target runs under AddressSanitizer with debug
assertions. A verdict job fails the run only after every target has
finished, if any produced a crash, a sanitizer report, a timeout or an
out-of-memory, and the reproducers are uploaded.

Limits per input: 1024 MiB resident, 256 MiB in one allocation, 10 s, 1 MiB
long. Over the seeded corpus under that build the highest peak was 288 MiB
resident (`dispatch_any_extension`) and no input took a second.

Cost, estimated from the timeouts, not measured: with all 32 targets, 8
jobs per stage and 2 stages of up to 350 minutes each, at most about 5,600
runner-minutes and about 12 hours of wall-clock time for a 12-hour run.

## Switching on

Some lanes need the fixture corpus published first. Before that, the jobs
that read it fail: `clippy` and `test` in `ci.yml` (the unit tests embed
fixtures), and `asan`, `fuzz-short` and `corpus-large` in `assurance.yml`.

1. Add the repository variables and secrets listed in
   `docs/test-corpus.md`, "Publishing".
2. Run `test-corpus-publish.yml` on `main` and merge a pull request with the
   `test-corpus/lock.json` and `test-corpus/sources.json` it produces.
3. Uncomment the `pull_request` trigger in `cflite-pr.yml`, and the
   `schedule` triggers in `cflite-batch.yml` and `fuzz-extended.yml`.
4. Mark `ci-complete` and `assurance-complete` as required checks in the
   branch rules for `main`.

## Before a pull request

The open-pr skill in `.agents/skills/open-pr/SKILL.md` lists the commands
CI runs. In short: fmt, both clippy invocations, docs with `-D warnings`,
nextest and doctests for both feature sets, the corpus checks, and the fuzz
crate's tests.
