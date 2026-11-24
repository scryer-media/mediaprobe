# Changelog

All notable changes to this project are documented in this file. Each heading
is a released version.

## 0.1.0 - 2026-11-24

- Adds the parser: `probe`, `probe_path`, `probe_source` and `probe_disc`
  read the container, streams, codecs, HDR and Dolby Vision signalling,
  languages, chapters, attachments and tags of MKV/WebM, MP4/MOV, MPEG-TS and
  M2TS, MPEG program streams, AVI, ASF/WMV, Ogg, FLV, RealMedia, DV and
  elementary streams, and the titles of DVD and Blu-ray images, within a
  byte and I/O budget set by `ProbeOptions`.
- Adds `diagnostics::diagnose_file`, a structural diagnosis of one file.
- The result types serialize with serde. With `default-features = false`
  the crate is the result types alone, without the parser.
- Not yet published to crates.io.

## 0.0.1 - 2026-11-24

- Reserves the crate name. There is no API yet.
