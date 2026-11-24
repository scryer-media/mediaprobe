//! The generated test corpus: a provenance ledger, a lock that pins one
//! published and signed manifest, and BLAKE3 content addressing for every
//! byte that crosses the network.
//!
//! The trust chain runs one way. `test-corpus/lock.json` is reviewed and
//! committed; it names the manifest by BLAKE3. The manifest names every
//! fixture by BLAKE3. A hydrate therefore accepts only bytes whose digest a
//! reviewed commit already fixed, whatever the CDN serves. The Sigstore
//! bundle beside the manifest proves the manifest came out of the protected
//! publish workflow on `main`; `verify-signature` checks it against the exact
//! identity the lock names.

pub(crate) mod commands;
pub(crate) mod http;
pub(crate) mod ledger;
pub(crate) mod lock;
pub(crate) mod manifest;
pub(crate) mod profiles;
pub(crate) mod sigstore;
pub(crate) mod sigv4;
pub(crate) mod store;
pub(crate) mod toolchain;

use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// The key prefix inside the corpus bucket. The bucket is dedicated to this
/// corpus; the prefix only keeps the key layout of the other scryer-media corpora.
pub(crate) const KEY_PREFIX: &str = "test-corpus";

/// The committed corpus configuration.
pub(crate) const SOURCES: &str = "test-corpus/sources.json";
pub(crate) const PROFILES: &str = "test-corpus/profiles.json";
pub(crate) const LOCK: &str = "test-corpus/lock.json";
pub(crate) const TOOLCHAIN: &str = "test-corpus/toolchain.json";
/// The committed, human-readable expectations every fixture is checked against.
pub(crate) const EXPECTATIONS: &str = "tests/media-fixtures.toml";
/// Where fixtures live once generated or hydrated. Ignored by Git.
pub(crate) const MEDIA_ROOT: &str = "tests/media";

#[derive(Debug)]
pub(crate) struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub(crate) type Result<T> = std::result::Result<T, Error>;

pub(crate) fn error(message: impl Into<String>) -> Error {
    Error(message.into())
}

pub(crate) fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(error(message))
}

pub(crate) trait Context<T> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T>;
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|source| error(format!("{}: {source}", what())))
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

pub(crate) fn blake3_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub(crate) fn is_blake3_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// BLAKE3 and length of a file, streamed.
pub(crate) fn digest_file(path: &Path) -> Result<(String, u64)> {
    let mut file = std::fs::File::open(path).context(|| format!("open {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

/// The repository root: xtask lives one directory below it.
pub(crate) fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(manifest_dir)
}

pub(crate) fn repo_path(relative: &str) -> PathBuf {
    repo_root().join(relative)
}

/// A ledger path: relative, forward slashes, no `.` or `..`, no empty
/// components, under the media root. Anything else could escape the tree a
/// hydrate writes into.
pub(crate) fn valid_repo_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && path.starts_with(&format!("{MEDIA_ROOT}/"))
}

/// Write through a sibling temporary file and rename, so a reader never sees
/// a half-written file and an interrupted write leaves the old one intact.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context(|| format!("create {}", parent.display()))?;
    }
    let temporary = temporary_sibling(path);
    std::fs::write(&temporary, bytes).context(|| format!("write {}", temporary.display()))?;
    rename_into_place(&temporary, path)
}

pub(crate) fn temporary_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.partial-{}", std::process::id()))
}

pub(crate) fn rename_into_place(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to).context(|| format!("rename {} -> {}", from.display(), to.display()))
}

/// Civil date and time from Unix seconds (UTC), for timestamps SigV4 and the
/// provenance document need. Howard Hinnant's days-from-civil inverse.
pub(crate) fn utc_parts(unix_seconds: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = unix_seconds / 86_400;
    let rem = unix_seconds % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year as u64,
        month as u64,
        day as u64,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
    )
}

pub(crate) fn utc_now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub(crate) fn utc_now_rfc3339() -> String {
    let (year, month, day, hour, minute, second) = utc_parts(utc_now_seconds());
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Serialize as canonical JSON: object keys sorted (serde_json's default map
/// is a BTreeMap), two-space indent, trailing newline. Two runs over the same
/// inputs produce the same bytes and so the same BLAKE3.
pub(crate) fn canonical_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value).context(|| "serialize".to_owned())?;
    let mut bytes = serde_json::to_vec_pretty(&value).context(|| "serialize".to_owned())?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path).context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).context(|| format!("parse {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_parts_round_trips_known_instants() {
        assert_eq!(utc_parts(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(utc_parts(1_440_938_160), (2015, 8, 30, 12, 36, 0));
        assert_eq!(utc_parts(951_782_400), (2000, 2, 29, 0, 0, 0));
    }

    #[test]
    fn ledger_paths_cannot_escape_the_media_root() {
        assert!(valid_repo_relative("tests/media/a.mkv"));
        assert!(valid_repo_relative("tests/media/large/a.ts"));
        for bad in [
            "",
            "/tests/media/a.mkv",
            "tests/media/../Cargo.toml",
            "tests/media//a.mkv",
            "tests/media/./a.mkv",
            "tests\\media\\a.mkv",
            "src/lib.rs",
            "C:/tests/media/a",
        ] {
            assert!(!valid_repo_relative(bad), "{bad}");
        }
    }

    #[test]
    fn canonical_json_sorts_keys_and_is_stable() {
        #[derive(serde::Serialize)]
        struct Shape {
            zebra: u8,
            apple: u8,
        }
        let bytes = canonical_json(&Shape { zebra: 1, apple: 2 }).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\n  \"apple\": 2,\n  \"zebra\": 1\n}\n"
        );
    }

    #[test]
    fn hex_and_digest_shapes() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
        let digest = blake3_bytes(b"");
        assert!(is_blake3_hex(&digest));
        assert_eq!(
            digest,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert!(!is_blake3_hex(&digest.to_uppercase()));
    }
}
