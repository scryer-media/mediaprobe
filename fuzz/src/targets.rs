//! The bodies of the fuzz targets, shared by `fuzz_targets/` and by the
//! regression test, which replays every literal in `regressions.rs` through
//! the same function its target uses.
//!
//! Every analysis is held to the same invariants: no panic, no more bytes
//! or I/O operations taken from the source than the read budgets allow
//! (counted outside the parser), a report that agrees with those budgets,
//! and an analysis that serialises.

use crate::inputs::{self, Input};
use crate::shim;
use crate::source::{Counting, Sparse};
use mediaprobe::{AnalysisDetails, ProbeDepth, ProbeOptions};
use serde_json::Value;
use std::io::{Cursor, Read, Seek};

/// The read budget `probe_source` applies by default.
pub const BYTE_BUDGET: u64 = ProbeOptions::DEFAULT_READ_BUDGET_BYTES;
/// The I/O-operation budget `probe_source` applies by default.
pub const IO_LIMIT: u64 = ProbeOptions::DEFAULT_IO_LIMIT;
/// Operations an entry point makes before its budget wraps the source: the
/// one seek to the end that learns the length.
pub const ENTRY_OPERATIONS: u64 = 1;
/// What the structural diagnosis may read: its default budget.
pub const DIAGNOSIS_BUDGET: u64 = 8 * 1024 * 1024;

fn check_report(details: &Value, label: &str) {
    if let Some(report) = details.get("report") {
        let read = report
            .get("bytes_read")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let seeks = report.get("seeks").and_then(Value::as_u64).unwrap_or(0);
        assert!(
            read <= BYTE_BUDGET,
            "{label}: report says {read} bytes read"
        );
        assert!(seeks <= IO_LIMIT, "{label}: report says {seeks} seeks");
    }
}

/// Probe `source` at `depth` and hold the result to the invariants.
pub fn analyze_counted_at<S: Read + Seek>(
    source: S,
    extension: &str,
    depth: ProbeDepth,
) -> Option<Value> {
    let mut counting = Counting::new(source);
    let result = shim::analyze(&mut counting, extension, depth);
    assert!(
        counting.bytes <= BYTE_BUDGET,
        "probe took {} bytes from the source",
        counting.bytes
    );
    assert!(
        counting.operations <= IO_LIMIT + ENTRY_OPERATIONS,
        "probe made {} I/O calls",
        counting.operations
    );
    let details = result.ok()?;
    check_report(&details, "probe");
    Some(details)
}

/// [`analyze_counted_at`] at full depth.
pub fn analyze_counted<S: Read + Seek>(source: S, extension: &str) -> Option<Value> {
    analyze_counted_at(source, extension, ProbeDepth::Full)
}

fn diagnose_counted(bytes: &[u8], extension: &str) {
    let mut counting = Counting::new(Cursor::new(bytes));
    let _ = shim::diagnose(&mut counting, extension);
    assert!(
        counting.bytes <= DIAGNOSIS_BUDGET,
        "diagnosis took {} bytes from the source",
        counting.bytes
    );
}

/// Probe and diagnose one in-memory input.
pub fn run(input: &Input) {
    analyze_counted(Cursor::new(&input.bytes[..]), &input.extension);
    diagnose_counted(&input.bytes, &input.extension);
}

/// A container target: the input as a file with a fixed extension.
pub fn container(data: &[u8], extension: &str) {
    run(&Input {
        bytes: data.to_vec(),
        extension: extension.to_owned(),
    });
}

pub fn dispatch(data: &[u8]) {
    run(&inputs::dispatch(data));
}

pub fn codec_config(data: &[u8]) {
    run(&inputs::codec_config(data));
}

pub fn bitstream(data: &[u8]) {
    run(&inputs::bitstream(data));
}

pub fn hdr_sei(data: &[u8]) {
    run(&inputs::hdr_sei(data));
}

pub fn dolby_vision(data: &[u8]) {
    run(&inputs::dolby_vision(data));
}

/// Disc images: the reader only takes a path, so the input goes to a
/// per-process scratch file first.
pub fn disc(data: &[u8]) {
    thread_local! {
        static PATH: std::path::PathBuf = std::env::temp_dir()
            .join(format!("mediaprobe-fuzz-{}.iso", std::process::id()));
    }
    PATH.with(|path| {
        std::fs::write(path, data).expect("writing the scratch disc image");
        if let Ok(analysis) = shim::analyze_path(path) {
            // The disc reader has its own, larger budgets: 128 MiB and 4096
            // operations across navigation and the selected title.
            let read = analysis
                .pointer("/report/bytes_read")
                .and_then(Value::as_u64);
            assert!(
                read.unwrap_or(0) <= 128 * 1024 * 1024,
                "disc read {read:?} bytes"
            );
        }
    });
}

/// SIMD and scalar paths agree: every dispatched scanner on the raw input
/// against its scalar fallback. (Acceleration is chosen once per process
/// from the environment, so a whole-probe comparison is a separate run with
/// `SCRYER_MEDIAINFO_ACCEL=off`, not something one input can toggle.)
pub fn simd_differential(data: &[u8]) {
    if let Err(divergence) = shim::scanner_differential(data) {
        panic!("SIMD and scalar scanners disagree: {divergence}");
    }
}

/// The read budgets hold on a 1 TiB virtual file whose head and tail are the
/// input: nothing scans the zeros between, and no length field sizes an
/// allocation or a loop past the budget.
pub fn budget(data: &[u8]) {
    let input = inputs::dispatch(data);
    let tail = input.bytes[input.bytes.len().saturating_sub(4096)..].to_vec();
    let sparse = Sparse::new(input.bytes.clone(), tail, 1 << 40);
    analyze_counted(sparse, &input.extension);
}

/// `probe_source` end to end at the depth the first byte picks, on the
/// dispatcher's input from the rest.
pub fn probe_depths(data: &[u8]) {
    let (depth, rest) = match data.split_first() {
        Some((first, rest)) => (shim::DEPTHS[usize::from(*first) % shim::DEPTHS.len()], rest),
        None => (ProbeDepth::Full, data),
    };
    let input = inputs::dispatch(rest);
    analyze_counted_at(Cursor::new(&input.bytes[..]), &input.extension, depth);
}

/// Most key and value bytes of tags one probe keeps, across the file and
/// every stream.
pub const TAG_BYTES_LIMIT: usize = 256 * 1024;

/// Key and value bytes of every tag in a serialized analysis, and how many
/// `tags_truncated` warnings its report carries.
pub fn tag_totals(details: &Value) -> (usize, usize) {
    let scope_bytes = |tags: Option<&Value>| {
        tags.and_then(Value::as_object).map_or(0, |map| {
            map.iter()
                .map(|(k, v)| k.len() + v.as_str().map_or(0, str::len))
                .sum::<usize>()
        })
    };
    let mut bytes = scope_bytes(details.get("tags"));
    for stream in details
        .get("streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        bytes += scope_bytes(stream.pointer("/metadata/tags"));
    }
    let warnings = details
        .pointer("/report/warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|w| w.get("code").and_then(Value::as_str) == Some("tags_truncated"))
        .count();
    (bytes, warnings)
}

/// Matroska tag collection on a `Tags` element shaped by the input. Beyond
/// the common invariants, the tags kept stay within [`TAG_BYTES_LIMIT`] and
/// the report warns about truncation at most once.
pub fn mkv_tags(data: &[u8]) {
    let input = inputs::mkv_tags(data);
    if let Some(details) = analyze_counted(Cursor::new(&input.bytes[..]), &input.extension) {
        let (bytes, warnings) = tag_totals(&details);
        assert!(bytes <= TAG_BYTES_LIMIT, "kept {bytes} bytes of tags");
        assert!(warnings <= 1, "{warnings} tags_truncated warnings");
    }
    diagnose_counted(&input.bytes, &input.extension);
}

/// A stored record read back: any JSON that loads as [`AnalysisDetails`]
/// serializes and loads again to the same value.
pub fn model_serde(data: &[u8]) {
    let Ok(details) = serde_json::from_slice::<AnalysisDetails>(data) else {
        return;
    };
    let json = serde_json::to_vec(&details).expect("loaded details serialize");
    let again: AnalysisDetails =
        serde_json::from_slice(&json).expect("serialized details load again");
    assert_eq!(details, again, "details changed in a JSON round trip");
}
