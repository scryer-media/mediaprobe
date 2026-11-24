# mediaprobe

Container and stream metadata for media files, read in pure Rust with no
external tools.

**This release reserves the crate name and has no API.** The parser exists
and is being prepared for publication: it reads the container, the tracks and
their codecs, HDR signalling, languages and chapters from the common video
containers and from disc images, within a fixed read budget. It will be
published here once its public API is settled.

## License

Apache-2.0. See [LICENSE](LICENSE).
