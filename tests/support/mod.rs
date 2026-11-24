//! Shared harness for the corpus tests.
//!
//! This module never calls the parser. It holds the model of
//! `tests/media-fixtures.toml`, the path resolver for the hydrated corpus, the
//! comparison of one analysis against its expectation, the known-gaps
//! ratchet, and the deterministic truncation and bit-flip cases. Everything
//! that touches the parser lives in `shim.rs`, and the selection rules the
//! projection applies (selected video, program audio, known languages) in
//! `details_facts.rs`, which the imported native tests share.

#![allow(dead_code)]

#[cfg(feature = "probe")]
#[path = "details_facts.rs"]
pub mod facts;
#[cfg(feature = "probe")]
pub mod shim;

use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The checked-in expectations, rendered by `cargo xtask test-corpus expectations`.
pub const EXPECTATIONS: &str = "tests/media-fixtures.toml";
/// Parser gaps the corpus exposes today, one entry per fixture.
pub const KNOWN_GAPS: &str = "tests/media-known-gaps.toml";
/// Where `cargo xtask test-corpus hydrate` puts the fixtures.
pub const MEDIA_ROOT: &str = "tests/media";
/// Overrides the media root, for a corpus hydrated somewhere else.
pub const MEDIA_ROOT_ENV: &str = "MEDIAPROBE_MEDIA_ROOT";
/// The corpus profile the expectation tests require (default `unit`).
pub const PROFILE_ENV: &str = "MEDIAPROBE_FIXTURE_PROFILE";
/// Set to `1` to rewrite the known-gaps file from the current results.
pub const BLESS_ENV: &str = "MEDIAPROBE_BLESS_KNOWN_GAPS";

/// The read budget `probe_source` applies by default
/// (`ProbeOptions::DEFAULT_READ_BUDGET_BYTES`).
pub const BYTE_BUDGET: u64 = 64 * 1024 * 1024;
/// The I/O-operation budget `probe_source` applies by default
/// (`ProbeOptions::DEFAULT_IO_LIMIT`).
pub const IO_LIMIT: u64 = 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectationsFile {
    pub fixtures: Vec<Fixture>,
}

/// One `[[fixtures]]` entry. Field meanings are documented where the xtask
/// renders them (`xtask/src/recipes/mod.rs`, `Expect`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub name: String,
    pub generator: String,
    pub profile: String,
    pub container: String,
    pub video_codec: Option<String>,
    pub width: i64,
    pub height: i64,
    pub fps: i64,
    pub audio_codecs: Vec<String>,
    pub audio_channels: Vec<i64>,
    pub audio_languages: Vec<String>,
    pub subtitle_stream_count: i64,
    pub min_duration_seconds: i64,
    pub valid_video: bool,
    pub video_bit_depth: Option<i64>,
    pub hdr_format: Option<String>,
    pub dovi_profile: Option<i64>,
    pub dovi_bl_compat_id: Option<i64>,
    pub chapter_titles: Option<Vec<Option<String>>>,
    #[serde(default)]
    pub warning_codes: Vec<String>,
    pub diagnostic_status: Option<String>,
    #[serde(default)]
    pub may_exhaust_budget: bool,
    #[serde(default)]
    pub payload_scrambled: bool,
    #[serde(default)]
    pub present: Vec<String>,
    #[serde(default)]
    pub absent: Vec<String>,
    #[serde(default)]
    pub checks: BTreeMap<String, toml::Value>,
}

impl Fixture {
    /// The fixture's extension, lowercased: the hint a caller would pass.
    pub fn extension(&self) -> String {
        Path::new(&self.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default()
    }

    /// Disc images go through the path API; everything else can be read
    /// from memory.
    pub fn is_disc_image(&self) -> bool {
        self.container == "iso" || self.name.starts_with("disc/")
    }
}

pub fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

pub fn load_expectations() -> Vec<Fixture> {
    let text = std::fs::read_to_string(repo_path(EXPECTATIONS))
        .unwrap_or_else(|e| panic!("reading {EXPECTATIONS}: {e}"));
    toml::from_str::<ExpectationsFile>(&text)
        .unwrap_or_else(|e| panic!("parsing {EXPECTATIONS}: {e}"))
        .fixtures
}

pub fn media_root() -> PathBuf {
    std::env::var_os(MEDIA_ROOT_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_path(MEDIA_ROOT))
}

/// The profile the expectation tests require: `unit` unless overridden.
/// `all` requires every fixture, `large` the budget fixtures only.
pub fn required_profile() -> String {
    std::env::var(PROFILE_ENV).unwrap_or_else(|_| "unit".to_owned())
}

pub fn profile_selects(profile: &str, fixture: &Fixture) -> bool {
    profile == "all" || fixture.profile == profile
}

/// The fixtures of the required profile, each with its path. A missing file
/// fails the run: these tests only run in lanes that hydrate the corpus, and
/// a silently skipped fixture would be a silently weakened check.
pub fn required_fixtures() -> Vec<(Fixture, PathBuf)> {
    let profile = required_profile();
    let root = media_root();
    let mut missing = Vec::new();
    let mut out = Vec::new();
    for fixture in load_expectations() {
        if !profile_selects(&profile, &fixture) {
            continue;
        }
        let path = root.join(&fixture.name);
        if path.is_file() {
            out.push((fixture, path));
        } else {
            missing.push(fixture.name);
        }
    }
    assert!(
        missing.is_empty(),
        "{} `{profile}` fixtures are missing under {} (first: {}). Run \
         `cargo xtask test-corpus hydrate --profile {profile}`.",
        missing.len(),
        root.display(),
        missing[0],
    );
    assert!(!out.is_empty(), "profile `{profile}` selects no fixtures");
    out
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// What the shim hands back for one fixture: the serialized analysis or the
/// error's text, and the structural diagnosis status when one was produced.
#[derive(Debug, Clone)]
pub struct Observed {
    pub analysis: Result<Value, String>,
    pub diagnostic_status: Option<String>,
}

/// One way an analysis differs from its expectation. `key` is stable across
/// runs and is what the known-gaps ratchet records; `detail` is for people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub key: String,
    pub detail: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.key, self.detail)
    }
}

fn walk<'a>(mut value: &'a Value, parts: &[&str]) -> Option<&'a Value> {
    for part in parts {
        value = match value {
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            Value::Object(map) => map.get(*part)?,
            _ => return None,
        };
        if value.is_null() {
            return None;
        }
    }
    Some(value)
}

fn streams_of<'a>(analysis: &'a Value, kind: &str) -> Vec<&'a Value> {
    analysis
        .pointer("/details/streams")
        .and_then(Value::as_array)
        .map(|streams| {
            streams
                .iter()
                .filter(|s| s.get("kind").and_then(Value::as_str) == Some(kind))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve an expectation path against a serialized analysis.
///
/// `summary.X` and `details.X` walk from the top-level summary and from
/// `details`; `video.X` starts at the first video stream; `audio.N.X` and
/// `subtitle.N.X` at the Nth stream of that kind in `details.streams`;
/// `attachment.N`, `chapter.N`, `program.N` and `caption.N` index the
/// matching `details` collection. JSON `null` resolves to `None`.
pub fn resolve<'a>(analysis: &'a Value, path: &str) -> Option<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let (head, rest) = parts.split_first()?;
    match *head {
        "summary" => walk(analysis, rest),
        "details" => walk(analysis.get("details")?, rest),
        "video" => walk(streams_of(analysis, "video").first()?, rest),
        "audio" | "subtitle" => {
            let (index, rest) = rest.split_first()?;
            let streams = streams_of(analysis, head);
            walk(streams.get(index.parse::<usize>().ok()?)?, rest)
        }
        "attachment" | "chapter" | "program" | "caption" => {
            let collection = match *head {
                "attachment" => "attachments",
                "chapter" => "chapters",
                "program" => "programs",
                _ => "caption_services",
            };
            let (index, rest) = rest.split_first()?;
            let items = analysis.get("details")?.get(collection)?.as_array()?;
            walk(items.get(index.parse::<usize>().ok()?)?, rest)
        }
        _ => None,
    }
}

/// Equality between an observed JSON value and an expected one. Numbers
/// compare by value whatever their JSON representation; an expected float
/// allows a relative error of 1e-6 (TOML and the parser round decimals
/// differently). Arrays and objects compare element by element.
pub fn values_match(got: &Value, want: &Value) -> bool {
    match (got, want) {
        (Value::Number(g), Value::Number(w)) => {
            let (Some(g), Some(w)) = (g.as_f64(), w.as_f64()) else {
                return false;
            };
            if want.is_f64() {
                (g - w).abs() <= 1e-6 * w.abs().max(1.0)
            } else {
                g == w
            }
        }
        (Value::Array(g), Value::Array(w)) => {
            g.len() == w.len() && g.iter().zip(w).all(|(g, w)| values_match(g, w))
        }
        (Value::Object(g), Value::Object(w)) => {
            g.len() == w.len()
                && w.iter()
                    .all(|(k, w)| g.get(k).is_some_and(|g| values_match(g, w)))
        }
        _ => got == want,
    }
}

fn opt(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or(Value::Null)
}

fn toml_to_json(value: &toml::Value) -> Value {
    serde_json::to_value(value).expect("TOML values always convert to JSON")
}

fn push(out: &mut Vec<Mismatch>, key: &str, detail: String) {
    out.push(Mismatch {
        key: key.to_owned(),
        detail,
    });
}

fn check(out: &mut Vec<Mismatch>, key: &str, got: Value, want: Value) {
    if !values_match(&got, &want) {
        push(out, key, format!("got {got} want {want}"));
    }
}

/// Compare one observation with its fixture's expectation.
///
/// The rules, in order:
/// - `container = "none"`: the parser must refuse the input or report no
///   container; nothing else is checked.
/// - Any error is a single `error` mismatch.
/// - `may_exhaust_budget` with `details.report.budget_exhausted` set: the
///   budget fixtures only promise a bounded, panic-free read, which the budget
///   test asserts; no field is compared.
/// - `payload_scrambled`: stream fields that need payload bytes (an absent
///   video codec, languages, subtitle count, duration, validity) are not
///   compared.
/// - Zero `width`, `height`, `fps` and per-stream `audio_channels` mean "not
///   asserted"; fps compares the rounded frame rate.
/// - Audio comes from the summary's `audio_streams`, which follow the
///   selected program. A stream language of `und` or none is dropped before
///   the language list is compared.
/// - `warning_codes` must be a subset of the report's warning codes.
/// - `present`, `absent` and `checks` resolve paths with [`resolve`].
pub fn compare(fixture: &Fixture, observed: &Observed) -> Vec<Mismatch> {
    let mut out = Vec::new();

    if fixture.container == "none" {
        if let Ok(analysis) = &observed.analysis {
            let container = opt(analysis.get("container_format"));
            if !container.is_null() {
                push(&mut out, "container", format!("got {container} want none"));
            }
        }
        return out;
    }
    let analysis = match &observed.analysis {
        Ok(analysis) => analysis,
        Err(error) => {
            push(&mut out, "error", error.clone());
            return out;
        }
    };
    let exhausted = analysis
        .pointer("/details/report/budget_exhausted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if fixture.may_exhaust_budget && exhausted {
        return out;
    }
    let scrambled = fixture.payload_scrambled;
    let string = |s: &Option<String>| s.clone().map(Value::String).unwrap_or(Value::Null);

    check(
        &mut out,
        "container",
        opt(analysis.get("container_format")),
        Value::String(fixture.container.clone()),
    );
    if !(scrambled && fixture.video_codec.is_none()) {
        check(
            &mut out,
            "video_codec",
            opt(analysis.get("video_codec")),
            string(&fixture.video_codec),
        );
    }
    if fixture.width > 0 {
        check(
            &mut out,
            "width",
            opt(analysis.get("video_width")),
            fixture.width.into(),
        );
    }
    if fixture.height > 0 {
        check(
            &mut out,
            "height",
            opt(analysis.get("video_height")),
            fixture.height.into(),
        );
    }
    if fixture.fps > 0 {
        // The summary carries the rate as a decimal string ("23.976").
        let fps = analysis
            .get("video_frame_rate")
            .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok()))
            .map(|f| Value::from(f.round() as i64))
            .unwrap_or(Value::Null);
        check(&mut out, "fps", fps, fixture.fps.into());
    }

    let audio: Vec<&Value> = analysis
        .get("audio_streams")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let codecs: Vec<Value> = audio.iter().map(|s| opt(s.get("codec"))).collect();
    check(
        &mut out,
        "audio_codecs",
        Value::Array(codecs),
        fixture
            .audio_codecs
            .iter()
            .cloned()
            .map(Value::String)
            .collect(),
    );
    if audio.len() == fixture.audio_codecs.len() {
        for (i, (stream, channels)) in audio.iter().zip(&fixture.audio_channels).enumerate() {
            if *channels > 0 {
                check(
                    &mut out,
                    &format!("audio.{i}.channels"),
                    opt(stream.get("channels")),
                    (*channels).into(),
                );
            }
        }
    }
    if !scrambled {
        let languages: Vec<Value> = audio
            .iter()
            .filter_map(|s| s.get("language").and_then(Value::as_str))
            .filter(|l| *l != "und")
            .map(|l| Value::String(l.to_owned()))
            .collect();
        check(
            &mut out,
            "audio_languages",
            Value::Array(languages),
            fixture
                .audio_languages
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        );
        check(
            &mut out,
            "subtitle_stream_count",
            streams_of(analysis, "subtitle").len().into(),
            fixture.subtitle_stream_count.into(),
        );
        let duration = analysis.get("duration_seconds").and_then(Value::as_f64);
        // Whole seconds, rounded, as the parser's own tests compare them: a
        // one-second fixture reports 0.999 in containers that time the last
        // frame's start.
        if fixture.min_duration_seconds > 0
            && !duration.is_some_and(|d| d.round() >= fixture.min_duration_seconds as f64)
        {
            push(
                &mut out,
                "min_duration_seconds",
                format!(
                    "got {duration:?} want >= {} rounded",
                    fixture.min_duration_seconds
                ),
            );
        }
        let valid = analysis.get("video_codec").is_some_and(|c| !c.is_null())
            && duration.is_some_and(|d| d > 0.0);
        check(
            &mut out,
            "valid_video",
            valid.into(),
            fixture.valid_video.into(),
        );
    }

    if let Some(depth) = fixture.video_bit_depth {
        check(
            &mut out,
            "video_bit_depth",
            opt(analysis.get("video_bit_depth")),
            depth.into(),
        );
    }
    if fixture.hdr_format.is_some() {
        check(
            &mut out,
            "hdr_format",
            opt(analysis.get("video_hdr_format")),
            string(&fixture.hdr_format),
        );
    }
    if let Some(profile) = fixture.dovi_profile {
        check(
            &mut out,
            "dovi_profile",
            opt(analysis.get("dovi_profile")),
            profile.into(),
        );
    }
    if let Some(id) = fixture.dovi_bl_compat_id {
        check(
            &mut out,
            "dovi_bl_compat_id",
            opt(analysis.get("dovi_bl_compat_id")),
            id.into(),
        );
    }
    if let Some(titles) = &fixture.chapter_titles {
        let got: Vec<Value> = analysis
            .pointer("/details/chapters")
            .and_then(Value::as_array)
            .map(|c| c.iter().map(|c| opt(c.get("title"))).collect())
            .unwrap_or_default();
        check(
            &mut out,
            "chapter_titles",
            Value::Array(got),
            titles.iter().map(string).collect(),
        );
    }
    if !fixture.warning_codes.is_empty() {
        let codes: BTreeSet<&str> = analysis
            .pointer("/details/report/warnings")
            .and_then(Value::as_array)
            .map(|w| w.iter().filter_map(|w| w.get("code")?.as_str()).collect())
            .unwrap_or_default();
        for code in &fixture.warning_codes {
            if !codes.contains(code.as_str()) {
                push(
                    &mut out,
                    &format!("warning:{code}"),
                    format!("absent; have {codes:?}"),
                );
            }
        }
    }
    if let Some(status) = &fixture.diagnostic_status {
        check(
            &mut out,
            "diagnostic_status",
            string(&observed.diagnostic_status),
            Value::String(status.clone()),
        );
    }
    for path in &fixture.present {
        if resolve(analysis, path).is_none() {
            push(&mut out, &format!("present:{path}"), "missing".to_owned());
        }
    }
    for path in &fixture.absent {
        if let Some(value) = resolve(analysis, path) {
            push(&mut out, &format!("absent:{path}"), format!("got {value}"));
        }
    }
    for (path, want) in &fixture.checks {
        check(
            &mut out,
            path,
            opt(resolve(analysis, path)),
            toml_to_json(want),
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Known gaps
// ---------------------------------------------------------------------------

/// Fixture name to the sorted mismatch keys it is known to produce.
pub type KnownGaps = BTreeMap<String, BTreeSet<String>>;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct KnownGapsFile {
    #[serde(default)]
    gaps: BTreeMap<String, Vec<String>>,
}

pub fn load_known_gaps() -> KnownGaps {
    let path = repo_path(KNOWN_GAPS);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return KnownGaps::new(),
        Err(e) => panic!("reading {KNOWN_GAPS}: {e}"),
    };
    let file: KnownGapsFile =
        toml::from_str(&text).unwrap_or_else(|e| panic!("parsing {KNOWN_GAPS}: {e}"));
    file.gaps
        .into_iter()
        .map(|(name, keys)| {
            let set: BTreeSet<String> = keys.iter().cloned().collect();
            assert_eq!(set.len(), keys.len(), "{KNOWN_GAPS}: {name} repeats a key");
            assert!(!set.is_empty(), "{KNOWN_GAPS}: {name} lists no keys");
            (name, set)
        })
        .collect()
}

pub fn render_known_gaps(gaps: &KnownGaps) -> String {
    let mut out = String::from(
        "# Parser gaps the fixture corpus exposes, as the mismatch keys each fixture\n\
         # produces today. tests/media_fixtures.rs fails when a fixture gains a key\n\
         # not listed here, and when a listed key starts passing, so this list only\n\
         # shrinks. Regenerate after a parser fix with\n\
         # `MEDIAPROBE_BLESS_KNOWN_GAPS=1 cargo nextest run --test media_fixtures`\n\
         # and review the diff: an entry may only disappear or lose keys.\n\n[gaps]\n",
    );
    for (name, keys) in gaps {
        let keys: Vec<String> = keys.iter().map(|k| toml_string(k)).collect();
        out.push_str(&format!("{} = [{}]\n", toml_string(name), keys.join(", ")));
    }
    out
}

fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

/// Compare the observed mismatch keys with the ratchet. Returns one line per
/// fixture whose keys differ, in either direction.
pub fn ratchet(
    observed: &KnownGaps,
    known: &KnownGaps,
    checked: &BTreeSet<String>,
    all_fixtures: &BTreeSet<String>,
) -> Vec<String> {
    let empty = BTreeSet::new();
    let mut out = Vec::new();
    for name in checked {
        let got = observed.get(name).unwrap_or(&empty);
        let want = known.get(name).unwrap_or(&empty);
        let new: Vec<_> = got.difference(want).collect();
        let fixed: Vec<_> = want.difference(got).collect();
        if !new.is_empty() {
            out.push(format!("{name}: new mismatches {new:?}"));
        }
        if !fixed.is_empty() {
            out.push(format!(
                "{name}: now passes {fixed:?}; remove from {KNOWN_GAPS}"
            ));
        }
    }
    for name in known.keys() {
        if !all_fixtures.contains(name) {
            out.push(format!("{name}: listed in {KNOWN_GAPS} but not a fixture"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Truncation and bit-flip cases
// ---------------------------------------------------------------------------

/// Percentages of the input kept by the proportional truncations.
pub const TRUNCATE_PERCENT: [u64; 12] = [1, 2, 3, 5, 8, 13, 21, 34, 55, 77, 90, 99];
/// Absolute lengths kept by the head truncations.
pub const TRUNCATE_BYTES: [usize; 8] = [1, 4, 8, 16, 32, 64, 128, 512];
/// Number of bit-flip cases per input.
pub const FLIP_CASES: u64 = 16;
/// Seed of the bit-flip generator.
pub const FLIP_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
/// Even-indexed flip cases stay inside the head, where headers live.
pub const FLIP_HEAD: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub label: String,
    pub bytes: Vec<u8>,
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Every truncated and bit-flipped variant of `input`, deterministically.
///
/// Truncations keep `k%` of the input for each `k` in [`TRUNCATE_PERCENT`]
/// and the first `n` bytes for each `n` in [`TRUNCATE_BYTES`] shorter than
/// the input. Flip case `i` flips `1 + (i % 4) * 8` bits chosen by one
/// xorshift64 stream seeded with [`FLIP_SEED`]; even cases choose bits within
/// the first [`FLIP_HEAD`] bytes, odd cases anywhere.
pub fn mutation_cases(input: &[u8]) -> Vec<Case> {
    let mut out = Vec::new();
    let len = input.len();
    for k in TRUNCATE_PERCENT {
        let keep = (len as u64 * k / 100) as usize;
        out.push(Case {
            label: format!("truncate {k}% ({keep} bytes)"),
            bytes: input[..keep].to_vec(),
        });
    }
    for n in TRUNCATE_BYTES {
        if n < len {
            out.push(Case {
                label: format!("truncate to {n} bytes"),
                bytes: input[..n].to_vec(),
            });
        }
    }
    if len == 0 {
        return out;
    }
    let mut rng = XorShift(FLIP_SEED);
    for i in 0..FLIP_CASES {
        let span = if i % 2 == 0 { len.min(FLIP_HEAD) } else { len };
        let flips = 1 + (i % 4) * 8;
        let mut bytes = input.to_vec();
        let mut bits = Vec::new();
        for _ in 0..flips {
            let bit = rng.next() % (span as u64 * 8);
            bytes[(bit / 8) as usize] ^= 1 << (bit % 8);
            bits.push(bit);
        }
        out.push(Case {
            label: format!("flip case {i}: bits {bits:?}"),
            bytes,
        });
    }
    out
}
