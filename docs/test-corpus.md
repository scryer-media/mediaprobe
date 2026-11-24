# The fixture corpus

The tests read real media files: 714 fixtures across every container and
codec the parser classifies. None of them is committed. They are generated
from recipes, published once to a public bucket by content address, and
downloaded (hydrated) into `tests/media/` by `cargo xtask test-corpus`.

## The pieces

| File | What it is |
| --- | --- |
| `xtask/src/recipes/` | One recipe per fixture: FFmpeg and speexenc invocations, Rust passes over their output, Rust builders, fetches of pinned files, and large-file synthesis. Also the expectations each fixture must meet. |
| `tests/media-fixtures.toml` | The expectations, rendered from the recipes. Never edited by hand. |
| `tests/media-known-gaps.toml` | Mismatches the parser is known to have, per fixture. A ratchet: a new mismatch fails, and a fixed one must be removed. |
| `test-corpus/sources.json` | The ledger: every fixture path, its generator, profile, size and BLAKE3, plus the shapes the corpus cannot carry and why. |
| `test-corpus/profiles.json` | Named selections of the ledger (`unit`, `large`, `fuzz-seeds`, `all`). |
| `test-corpus/toolchain.json` | The FFmpeg and speexenc versions and encoders the corpus is generated with. |
| `test-corpus/lock.json` | The published revision the tests read: base URL, manifest digest, and the Sigstore identity that signed it. |

Rendered files are checked: `cargo xtask test-corpus expectations --check`,
`ledger --check` and `verify` fail when a recipe changed and its rendering
did not. CI runs all three in `corpus-config`.

## Hydrating

```sh
cargo xtask test-corpus hydrate                     # the unit profile
cargo xtask test-corpus hydrate --profile large     # the large fixtures
cargo xtask test-corpus hydrate --profile all --verify-signature
```

Hydrate reads the pinned manifest, fetches every file of the selected
profiles by its BLAKE3 address, checks size and digest before a file lands,
and skips files already present with the right digest. With
`--verify-signature` it also checks the manifest's Sigstore bundle against
the identity in the lock.

The parser's unit tests embed fixtures with `include_bytes!`, so the test
targets of the default feature set do not compile until the unit profile is
hydrated. `--no-default-features` needs no corpus.

## Profiles

| Profile | Files | Bytes | Used by |
| --- | ---: | ---: | --- |
| `unit` | 701 | 54,740,971 | the corpus tests, robustness, clippy and test lanes |
| `large` | 4 published + 9 synthesised | 317,561,211 published; 13,035,995,108 synthesised (apparent) | `corpus-large` in assurance.yml |
| `fuzz-seeds` | 701 | 54,740,971 | seeding the fuzz corpora |
| `all` | all of the above | | the publish workflow's read-back |

## Generators

| Generator | Recipe | What |
| --- | --- | --- |
| `matrix` | `matrix.rs` | The container × codec matrix and the SIMD-scanner inputs. |
| `supplemental` | `supplemental.rs` | AV1, HDR10, HLG, HDR10+, chapters, program streams, 8K. |
| `dolby_vision` | `supplemental.rs` | HEVC with a Dolby Vision configuration record inserted. |
| `builders` | `builders.rs` | Shapes no FFmpeg muxer writes (OGM). |
| `large` | `large.rs` | Budget fixtures of 50–90 MiB, published. |
| `coverage` | `coverage.rs` | One fixture per classifying branch of the parser, with injected bitstream units where no encoder writes the signal. |
| `disc` | `disc.rs` | DVD-Video ISO 9660 and Blu-ray / UHD Blu-ray UDF images. |
| `imported` | `imported.rs` | 69 files taken byte for byte from the repository the parser came from, at a pinned commit, because its tests assert facts of those exact bytes. |
| `synthesized` | `synth.rs` | Files past 1, 2 and 4 GiB built from an imported seed plus format padding, written at hydrate time and never published. |

Shapes the corpus cannot carry, because nothing it may run can write them
(a VVC encoder, Atmos, DTS-HD MA, PGS subtitles and so on), are listed in
the ledger as `not_reproducible` with the reason. There are 33.

## Large fixtures

The parser reads within a byte budget (64 MiB by default, 128 MiB for disc
images) and an I/O-operation budget (1024, or 4096 for disc images). The
large fixtures check that it stays inside both on files far bigger than the
budgets, reports the right streams, and that its cost does not grow with the
file's length.

The four published ones are real FFmpeg encodes. The nine synthesised ones
are written from a small imported seed by `xtask/src/recipes/synth.rs`:
deterministic, so the recipe pins each file's size and BLAKE3, and sparse
where the format allows (an MP4 `mdat`, an EBML `Void`, an AVI `JUNK` chunk
are holes; null transport-stream packets are data, since each starts with a
sync byte).

| Fixture | Apparent size | Seed | Shape | Probe result |
| --- | ---: | --- | --- | --- |
| `mp4_co64_over_4g.mp4` | 4,296,050,916 | `simd_dense_h264_aac.mp4` | 64-bit `mdat` over 4 GiB, every chunk offset in `co64`, `moov` at the end | streams and 8 s; 18,703 bytes, 21 seeks |
| `mp4_co64_100m.mp4` | 104,892,644 | same | the same at 100 MiB | identical cost |
| `mp4_moov_first_300m.mp4` | 314,607,844 | same | `moov` before a 300 MiB `mdat` | streams and 8 s; 18,703 bytes, 21 seeks |
| `mkv_seekhead_cues_tags_over_4g.mkv` | 4,296,052,667 | `h264_aac.mkv` | eight-byte EBML sizes; Cues and Tags after a 4 GiB `Void`, reached only through the SeekHead | streams and 2.05 s; 878,841 bytes, 156 seeks |
| `mkv_seekhead_cues_tags_100m.mkv` | 104,894,395 | same | the same at 100 MiB | identical cost |
| `mkv_no_seekhead_no_cues_300m.mkv` | 314,706,010 | same | unknown-size Segment, no SeekHead, no Cues, 300 clusters each behind a 1 MiB `Void` | streams and 7.32 s; 3,024,923 bytes, 193 seeks |
| `ts_pts_pcr_wrap_300m.ts` | 315,024,268 | `simd_dense_h264_aac.ts` | PTS and PCR wrap the 33-bit counter 4 s in; 300 MiB of null packets; a second copy 20 s later | streams and 28 s; 7,692,724 bytes, 90 seeks |
| `avi_idx1_over_1g.avi` | 1,075,620,214 | `matrix_avi_002.avi` | AVI 1.0, a 1 GiB `JUNK` before `movi`, `idx1` only | streams and 1.37 s; 205,996 bytes, 94 seeks |
| `avi_opendml_over_2g.avi` | 2,214,146,150 | same | OpenDML: `RIFF AVI` and `RIFF AVIX`, `indx` super indexes, `ix##` chunks, `odml`/`dmlh` | streams; duration 1.37 s of 2.75 s (a known gap) |

Every synthesised fixture was checked with FFmpeg: it decodes them all, and
its durations are the ones in the table except where marked. No fixture
exhausts the read budget. The cost columns are from the probe's report
(`bytes_read`, `seeks`); `tests/budget.rs` asserts the 100 MiB and over-4
GiB twins cost exactly the same.

Measured on an Apple M-series machine with APFS: synthesis takes about
0.1 s for a 100–300 MiB file and 2 s for a 4 GiB one, most of it the BLAKE3
check; the probe takes milliseconds with a peak resident size of 3–5 MiB.
On APFS a 4 GiB file with one hole takes 52 KiB of disk, but holes of a
few MiB are allocated, so the linear Matroska file takes its full 300 MiB.
Linux runners were not measured.

Disc images past the 128 MiB disc budget are not built: `disc.rs` authors
an image in memory with each file's data inline, so a 1–2 GiB image would
need hole-aware extents in its ISO 9660 and UDF writers first.

## Publishing

`test-corpus-publish.yml` is the only workflow with write access to the
bucket. It runs on `main`, by hand, after a `guard` job that holds no
secrets checks the branch and a typed confirmation. It generates the whole
corpus with the pinned toolchain, verifies it, builds and signs the
manifest keylessly with the workflow's OIDC identity, uploads each file
create-only under its BLAKE3 address, reads the revision back through the
public URL, and uploads the new `lock.json` and `sources.json` as an
artifact. Pinning them is a reviewed pull request; the workflow never
commits.

The repository needs these before the first publish:

| Kind | Name | Value |
| --- | --- | --- |
| variable | `R2_BUCKET_NAME` | the bucket |
| variable | `R2_PUBLIC_URL` | its public base URL, the one in `lock.json` |
| secret | `R2_CORPUS_ACCESS_KEY` | an R2 API token's access key, write access to that bucket only |
| secret | `R2_CORPUS_ACCESS_SECRET` | its secret |
| secret | `R2_CORPUS_ENDPOINT` | the account's S3 endpoint |

## Changing a fixture

1. Change its recipe (or add one) under `xtask/src/recipes/`.
2. `cargo xtask test-corpus generate --fixture NAME`, then run the corpus
   tests against it.
3. `cargo xtask test-corpus expectations` and `cargo xtask test-corpus ledger`.
4. A synthesised fixture: put the size and BLAKE3 `generate` printed into
   its pin in `synth.rs`.
5. If the parser disagrees with the new expectation for a reason that is
   the parser's, record it with
   `MEDIAPROBE_BLESS_KNOWN_GAPS=1 cargo nextest run --test media_fixtures`
   and review the diff.
6. The new revision is published by the maintainer through
   `test-corpus-publish.yml`.

Fixture names are invented. No real titles, release names or file names.
