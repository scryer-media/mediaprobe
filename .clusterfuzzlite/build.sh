#!/bin/bash
# ClusterFuzzLite build: every fuzz target, its seeds and its dictionary.
set -euo pipefail

# The image pins RUSTUP_TOOLCHAIN to a nightly older than the crate's
# rust-version, and that variable outranks `rustup default`.
rustup toolchain install nightly --profile minimal
export RUSTUP_TOOLCHAIN=nightly

# The sanitizer flags arrive in RUSTFLAGS. Debug assertions (-a) keep
# overflow checks on in the optimised build, so a wrapping length
# computation panics.
cd "$SRC/mediaprobe"
cargo fuzz build -O -a

dictionary_for() {
  case "$1" in
    container_mkv|container_webm|ebml_elements) echo ebml.dict ;;
    container_mp4|container_mov|mp4_boxes|dolby_vision) echo mp4.dict ;;
    container_ts|container_m2ts|ts_packets|container_mpeg_ps|container_vob) echo ts.dict ;;
    elementary_*|bitstream|hdr_sei|codec_config) echo codec.dict ;;
    *) echo "" ;;
  esac
}

for target in $(cargo fuzz list); do
  cp "fuzz/target/x86_64-unknown-linux-gnu/release/$target" "$OUT/"
  if [ -d "fuzz/corpus/$target" ] && [ -n "$(ls -A "fuzz/corpus/$target")" ]; then
    # Flat: ClusterFuzzLite unpacks <target>_seed_corpus.zip as the
    # starting corpus for that target.
    (cd "fuzz/corpus/$target" && zip -q -r -j "$OUT/${target}_seed_corpus.zip" .)
    echo "$target: $(find "fuzz/corpus/$target" -type f | wc -l) seed(s)"
  else
    echo "$target: no seeds"
  fi
  dictionary="$(dictionary_for "$target")"
  if [ -n "$dictionary" ]; then
    cp "fuzz/dicts/$dictionary" "$OUT/${target}.dict"
  fi
done
