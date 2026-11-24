#!/usr/bin/env bash
# A short, seeded run of every fuzz target: the fuzz lane on pull requests.
#
#   fuzz/scripts/smoke.sh [RUNS] [MAX_SECONDS]
#
# Each target runs RUNS executions (default 20000) with a fixed seed, capped
# at MAX_SECONDS (default 60), from fuzz/corpus/<target> (seeded by
# seed-corpora.sh; a target with no seeds starts empty). Every target runs
# even after one fails; crashes land in fuzz/artifacts/<target>/ and the
# script exits non-zero naming them. FUZZ_TARGETS (space-separated) limits
# the run to those targets; fuzz-extended.yml runs one per job.
# FUZZ_TOOLCHAIN picks the nightly (default: nightly). RUSTFLAGS is passed
# through.
set -euo pipefail

runs="${1:-20000}"
seconds="${2:-60}"
toolchain="${FUZZ_TOOLCHAIN:-nightly}"
cd "$(dirname "$0")/../.."

cargo "+$toolchain" fuzz build --debug-assertions
failed=()
for target in ${FUZZ_TARGETS:-$(cargo "+$toolchain" fuzz list)}; do
  mkdir -p "fuzz/corpus/$target" "fuzz/artifacts/$target"
  echo "::group::$target"
  if ! cargo "+$toolchain" fuzz run --debug-assertions "$target" "fuzz/corpus/$target" -- \
      -seed=1 -runs="$runs" -max_total_time="$seconds" -max_len=1048576 \
      -timeout=10 -rss_limit_mb=4096 -print_final_stats=1 \
      -artifact_prefix="fuzz/artifacts/$target/"; then
    failed+=("$target")
  fi
  echo "::endgroup::"
done

if [ "${#failed[@]}" -ne 0 ]; then
  echo "fuzz targets failed: ${failed[*]}" >&2
  exit 1
fi
echo "every fuzz target ran clean"
