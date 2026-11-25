#!/usr/bin/env bash
# One stage of the release fuzz check for a group of targets, run side by
# side, one per core. Called by .github/workflows/release-fuzz.yml.
#
#   fuzz/scripts/release-fuzz.sh SECONDS CORPUS_DIR OUT_DIR TARGET...
#
# Every target is built under AddressSanitizer with debug assertions, then
# fuzzed for SECONDS in libFuzzer's fork mode, which keeps fuzzing after a
# crash, an out-of-memory or a timeout and writes each reproducer to
# OUT_DIR/artifacts/<target>/. Inputs and the grown corpus live in
# CORPUS_DIR/<target>/. OUT_DIR/summary/<target>.json records executions,
# coverage, new corpus entries and findings.
#
# The script exits 0 when every target fuzzed to its time limit, findings or
# not: the workflow's verdict job fails the run once every target has
# finished. It exits non-zero when the fuzzing could not run (a build
# failure, a missing tool) or a target stopped before its time limit.
#
# Limits, per input: RSS_LIMIT_MB (default 1024) of resident memory,
# MALLOC_LIMIT_MB (default 256) in one allocation, TIMEOUT (default 10)
# seconds, MAX_LEN (default 1048576) bytes.
set -euo pipefail

[ "$#" -ge 4 ] || { echo "usage: $0 SECONDS CORPUS_DIR OUT_DIR TARGET..." >&2; exit 2; }
seconds="$1"
corpus="$2"
out="$3"
shift 3
targets=("$@")

rss_limit="${RSS_LIMIT_MB:-1024}"
malloc_limit="${MALLOC_LIMIT_MB:-256}"
timeout="${TIMEOUT:-10}"
max_len="${MAX_LEN:-1048576}"
case "$seconds" in '' | *[!0-9]*) echo "SECONDS must be a whole number" >&2; exit 2 ;; esac

repo="$(cd "$(dirname "$0")/../.." && pwd)"
dicts="$repo/fuzz/dicts"
mkdir -p "$out/summary" "$out/logs"

dictionary_for() {
  case "$1" in
    container_mkv|container_webm|ebml_elements|mkv_tags) echo ebml.dict ;;
    container_mp4|container_mov|mp4_boxes|dolby_vision) echo mp4.dict ;;
    container_ts|container_m2ts|ts_packets|container_mpeg_ps|container_vob) echo ts.dict ;;
    elementary_*|bitstream|hdr_sei|codec_config) echo codec.dict ;;
    *) echo "" ;;
  esac
}

# One build for the group: the targets share every dependency.
(cd "$repo" && for target in "$@"; do cargo fuzz build -O -a --sanitizer address "$target"; done)

triple="$(rustc -vV | sed -n 's/^host: //p')"
bin_dir="$repo/fuzz/target/$triple/release"

# A small quarantine keeps freed memory from counting against the RSS limit
# for long; leaks are not findings for a parser that returns its result.
export ASAN_OPTIONS="${ASAN_OPTIONS:-quarantine_size_mb=64:detect_leaks=0:allocator_may_return_null=0}"

pids=()
for target in "$@"; do
  mkdir -p "$corpus/$target" "$out/artifacts/$target"
  before="$(find "$corpus/$target" -type f | wc -l | tr -d ' ')"
  echo "$before" > "$out/logs/$target.before"
  args=(
    "-fork=1" "-ignore_crashes=1" "-ignore_ooms=1" "-ignore_timeouts=1"
    "-max_total_time=$seconds" "-rss_limit_mb=$rss_limit" "-malloc_limit_mb=$malloc_limit"
    "-timeout=$timeout" "-max_len=$max_len" "-print_final_stats=1"
    "-artifact_prefix=$out/artifacts/$target/"
  )
  dictionary="$(dictionary_for "$target")"
  if [ -n "$dictionary" ]; then
    args+=("-dict=$dicts/$dictionary")
  fi
  "$bin_dir/$target" "${args[@]}" "$corpus/$target" > "$out/logs/$target.log" 2>&1 &
  pids+=("$!")
done

# In fork mode libFuzzer exits with the status of the last job it ran, so a
# target that fuzzed to its time limit exits non-zero when that job ended in
# an ignored out-of-memory (71), timeout (70) or crash. Such a run logs that
# it reached the limit and then the status it exits with; any other non-zero
# exit means the target could not run or stopped early, and its log says why.
ran_to_time_limit() {
  local log="$1" code="$2"
  grep -qE '^INFO: fuzzed for [0-9]+ seconds, wrapping up soon$' "$log" &&
    [ "$(sed -nE 's/^INFO: exiting: ([0-9]+) time: [0-9]+s$/\1/p' "$log" | tail -n 1)" = "$code" ]
}

status=0
for i in "${!pids[@]}"; do
  code=0
  wait "${pids[$i]}" || code=$?
  if [ "$code" -ne 0 ] && ! ran_to_time_limit "$out/logs/${targets[$i]}.log" "$code"; then
    echo "::warning::${targets[$i]} exited $code before its time limit" >&2
    status=1
  fi
done

for target in "$@"; do
  log="$out/logs/$target.log"
  before="$(cat "$out/logs/$target.before")"
  after="$(find "$corpus/$target" -type f | wc -l | tr -d ' ')"
  findings="$(find "$out/artifacts/$target" -type f | wc -l | tr -d ' ')"
  # The last status line libFuzzer printed: "#N ... cov: C ft: F corp: K ...".
  last="$(grep -E '^#[0-9]+.*cov: ' "$log" | tail -n 1 || true)"
  executions="$(printf '%s' "$last" | sed -nE 's/^#([0-9]+).*/\1/p')"
  coverage="$(printf '%s' "$last" | sed -nE 's/.* cov: ([0-9]+).*/\1/p')"
  features="$(printf '%s' "$last" | sed -nE 's/.* ft: ([0-9]+).*/\1/p')"
  kinds="$(find "$out/artifacts/$target" -type f -exec basename {} \; | sed -E 's/-.*//' | sort | uniq -c | awk '{printf "%s\"%s\": %s", (NR > 1 ? ", " : ""), $2, $1}')"
  cat > "$out/summary/$target.json" <<EOF
{"target": "$target", "seconds": $seconds, "executions": ${executions:-0}, "coverage": ${coverage:-0}, "features": ${features:-0}, "corpus_before": $before, "corpus_after": $after, "new_entries": $((after - before)), "findings": $findings, "finding_kinds": {${kinds}}, "ran": $([ -n "$last" ] && echo true || echo false)}
EOF
  cat "$out/summary/$target.json"
done
exit "$status"
