# mediaprobe

Container and stream metadata for media files, read in pure Rust with no
external tools.

It reads the container, the streams and their codecs, HDR and Dolby Vision
signalling, languages, chapters and tags from the common video containers and
from DVD and Blu-ray images, within a byte and I/O budget the caller sets.
Version 0.1.0 is not yet published to crates.io; crates.io holds only the
0.0.1 name reservation.

```rust
let details = mediaprobe::probe("movie.mkv")?;
println!("{}", serde_json::to_string_pretty(&details)?);
```

The default `probe` feature carries the parser. With
`default-features = false` the crate is the serde result types alone.

Tests run against a generated fixture corpus that is published separately and
hydrated before the tests run; see [docs/testing.md](docs/testing.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
