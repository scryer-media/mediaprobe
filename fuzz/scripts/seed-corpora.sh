#!/usr/bin/env bash
# Seed every fuzz target's corpus from the hydrated fixture corpus.
#
#   cargo xtask test-corpus hydrate --profile fuzz-seeds
#   fuzz/scripts/seed-corpora.sh [MEDIA_ROOT] [CORPUS_DIR]
#
# MEDIA_ROOT defaults to tests/media, CORPUS_DIR to fuzz/corpus. Each
# container target gets the fixtures of its extensions; the dispatcher,
# SIMD-differential and budget targets get every fixture behind the selector
# byte that names its extension (see inputs::EXTENSIONS); the wrapping
# targets get generated seeds. Existing corpus entries are kept: libFuzzer
# names entries by content hash, so re-seeding never duplicates one.
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
media="${1:-$repo/tests/media}"
corpus="${2:-$repo/fuzz/corpus}"
[ -d "$media" ] || { echo "no corpus at $media; run cargo xtask test-corpus hydrate --profile fuzz-seeds" >&2; exit 1; }

# The order of inputs::EXTENSIONS; a seed's first byte is its index here.
extensions=(mkv mka mks mk3d webm mp4 m4v m4a mov qt 3gp 3g2 ismv f4v avi divx ts m2ts mts tp trp m2t
  mpg mpeg vob evo m2v m1v mpv h264 264 avc h265 265 hevc ogg ogv oga ogm flv wmv asf rm rmvb dv ivf
  obu iso img bin "" MKV)

target_for() {
  case "$1" in
    mkv|mka|mks|mk3d) echo "container_mkv ebml_elements" ;;
    webm) echo "container_webm ebml_elements" ;;
    mp4|m4v|m4a|3gp|3g2|ismv|f4v) echo "container_mp4 mp4_boxes" ;;
    mov|qt) echo "container_mov mp4_boxes" ;;
    avi|divx) echo "container_avi" ;;
    ts|tp|trp|m2t) echo "container_ts ts_packets" ;;
    m2ts|mts) echo "container_m2ts ts_packets" ;;
    mpg|mpeg) echo "container_mpeg_ps" ;;
    vob|evo) echo "container_vob" ;;
    ogg|ogv|oga|ogm|opus) echo "container_ogg" ;;
    flv) echo "container_flv" ;;
    wmv|asf|wma) echo "container_asf" ;;
    rm|rmvb|ra) echo "container_realmedia" ;;
    dv|dif) echo "container_dv" ;;
    h264|264|avc) echo "elementary_h264" ;;
    h265|265|hevc) echo "elementary_hevc" ;;
    m2v|m1v|mpv) echo "elementary_mpegvideo" ;;
    iso|img|bin) echo "disc_image disc_navigation" ;;
    *) echo "" ;;
  esac
}

index_of() {
  local i
  for i in "${!extensions[@]}"; do
    if [ "${extensions[$i]}" = "$1" ]; then echo "$i"; return; fi
  done
  echo ""
}

count=0
# The large fixtures are left out, as the fuzz-seeds profile leaves them
# out: libFuzzer caps input length far below them.
while IFS= read -r -d '' file; do
  name="${file#"$media"/}"
  ext="${file##*.}"
  ext="$(printf '%s' "$ext" | tr '[:upper:]' '[:lower:]')"
  key="$(printf '%s' "$name" | tr '/' '_')"
  for target in $(target_for "$ext"); do
    mkdir -p "$corpus/$target"
    cp "$file" "$corpus/$target/$key"
    count=$((count + 1))
  done
  idx="$(index_of "$ext")"
  # Disc images are megabytes of video; the in-memory targets get the rest.
  if [ -n "$idx" ] && [ "$(wc -c < "$file")" -le 4194304 ]; then
    for target in dispatch_any_extension simd_differential budget_invariants; do
      mkdir -p "$corpus/$target"
      { printf '%b' "\\0$(printf '%03o' "$idx")"; cat "$file"; } > "$corpus/$target/$key"
      count=$((count + 1))
    done
  fi
done < <(find "$media" -type f ! -name '.*' ! -path "$media/large/*" -print0)

(cd "$repo/fuzz" && cargo run --locked --quiet --example write_seeds -- "$corpus")
echo "seeded $count corpus entries from $media"
