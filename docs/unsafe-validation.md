# Validating the unsafe code

All of the crate's `unsafe` is in `src/scan.rs`: the byte scanners that
find start codes, box and element IDs and audio sync words, with AVX2
versions on x86_64 and NEON versions on aarch64, chosen at run time, and a
scalar fallback that is the reference. Every `unsafe` block carries a
`// SAFETY:` comment naming the bound it relies on and the check that
established it.

Four lanes check that code, each catching what the others miss. All four
drive the same entry point.

## The entry point

With the `fuzzing` feature the crate exposes `mediaprobe::__fuzzing`. It is
for the test and fuzz harnesses only and carries no stability promise.

- `scanner_differential(data)` reads needles and a start offset from the
  input, runs every scanner the current CPU dispatches to and the scalar
  fallback over the rest, and returns an error naming the first scanner
  whose answer differs.
- `diagnose_source(reader, extension, options)` runs the structural
  diagnosis over any reader, so the fuzz targets reach it without a scratch
  file.

`tests/unsafe_scanners.rs` builds in-memory inputs that put each pattern
across every lane boundary of the 16- and 32-byte vectors, at lengths
around their multiples, and passes them through `scanner_differential`. It
needs no corpus. Under Miri it takes a smaller sweep of the same shapes.

## The lanes

| Lane | Workflow job | What it catches | Corpus |
| --- | --- | --- | --- |
| Miri | `assurance.yml` `miri-*` | Undefined behaviour in the unsafe code itself: out-of-bounds pointer arithmetic, misaligned or uninitialised reads, provenance violations. Runs NEON natively on aarch64, AVX2 on x86_64 (enabled at compile time, since Miri reports no CPU features at run time), and the x86_64 scalar baseline. | none |
| AddressSanitizer | `assurance.yml` `asan-*` | Out-of-bounds and use-after-free reads at native speed, over the full sweep, the scanner unit tests and the tag bounds test, with the standard library rebuilt instrumented. On x86_64 and aarch64. | unit |
| Valgrind | `assurance.yml` `valgrind` | Reads of uninitialised memory, which AddressSanitizer does not see, on the release build. | none |
| SIMD differential | `assurance.yml` `simd-differential-*` | A wrong answer from a vector scanner on an input nobody wrote down: ten minutes of the `simd_differential` fuzz target on each architecture. | none |

The release fuzz campaign (`release-fuzz.yml`) and the fuzz lanes run every
other target under AddressSanitizer too, so the scanners are also exercised
through the parsers on fuzzed media.

Measured on an Apple M-series machine with the pinned nightly, test time
only:

| Miri run | Time | Result |
| --- | ---: | --- |
| aarch64-apple-darwin (NEON) | 104 s (an earlier run: 525 s) | 1 passed |
| x86_64-unknown-linux-gnu with `+avx2`, interpreted on aarch64 | 110 s | 1 passed |
| x86_64-unknown-linux-gnu baseline, interpreted on aarch64 | 130 s | 1 passed |

CI runners were not measured.

## Running them locally

Miri and the sanitizers need the pinned nightly named in `assurance.yml`
(`NIGHTLY`), with the `miri` and `rust-src` components.

```sh
# Miri, on this machine's architecture
cargo +nightly-2026-11-23 miri nextest run --locked --features fuzzing \
  --test unsafe_scanners --no-fail-fast

# Miri on x86_64 with AVX2, from any host
RUSTFLAGS="-C target-feature=+avx2" cargo +nightly-2026-11-23 miri nextest run \
  --locked --features fuzzing --target x86_64-unknown-linux-gnu \
  --test unsafe_scanners --no-fail-fast

# AddressSanitizer (Linux; hydrate the unit corpus first)
RUSTFLAGS=-Zsanitizer=address cargo +nightly-2026-11-23 nextest run --locked \
  -Zbuild-std --target x86_64-unknown-linux-gnu --features fuzzing \
  --test unsafe_scanners --test tag_limits --lib \
  -E 'test(/scan/) | binary(unsafe_scanners) | binary(tag_limits)'

# Valgrind (Linux)
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="valgrind --error-exitcode=1 --track-origins=yes --leak-check=no" \
  cargo nextest run --locked --release --features fuzzing --test unsafe_scanners

# The SIMD differential
cargo +nightly-2026-11-23 fuzz run -O -a simd_differential -- -max_total_time=600
```

If `cargo +nightly-… miri` reports "no such command", put that toolchain's
`bin` directory first on `PATH` and run `cargo miri` without the `+`.

## Changing the unsafe code

- Keep the `// SAFETY:` comment true. It names the bound and the check.
- A new scanner gets a scalar fallback, a branch in `scanner_differential`,
  and its patterns in `tests/unsafe_scanners.rs`.
- A test Miri cannot run is excluded under `cfg(miri)`. The lane is never
  switched off.
