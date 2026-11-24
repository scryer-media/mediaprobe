//! The only place the fuzz harness names the parser's API: the public probe
//! entry points, and the internals the parser exposes to this harness behind
//! its `fuzzing` feature (`mediaprobe::__fuzzing`; see
//! docs/unsafe-validation.md for that contract).

use mediaprobe::diagnostics::DiagnosticsOptions;
use mediaprobe::{ProbeDepth, ProbeOptions, probe_path, probe_source};
use serde_json::Value;
use std::io::{Read, Seek};
use std::path::Path;

/// Every depth, shallowest first, for the targets that pick one.
pub const DEPTHS: &[ProbeDepth] = ProbeDepth::ALL;

/// The options the targets probe with: the default budgets at `depth`.
pub fn options(depth: ProbeDepth) -> ProbeOptions {
    ProbeOptions::new().with_depth(depth)
}

fn to_json<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("analysis serializes")
}

/// Probe a reader with an extension hint at `depth`: the serialized
/// details, or the error's text.
pub fn analyze<R: Read + Seek>(
    source: R,
    extension: &str,
    depth: ProbeDepth,
) -> Result<Value, String> {
    probe_source(source, extension, &options(depth))
        .map(to_json)
        .map_err(|e| e.to_string())
}

/// Probe a file through the path API (the only way into the disc reader).
pub fn analyze_path(path: &Path) -> Result<Value, String> {
    probe_path(path, &options(ProbeDepth::Full))
        .map(to_json)
        .map_err(|e| e.to_string())
}

/// The structural diagnosis, which samples payloads the probe skips.
pub fn diagnose<R: Read + Seek>(source: R, extension: &str) -> Option<Value> {
    mediaprobe::__fuzzing::diagnose_source(source, extension, DiagnosticsOptions::default())
        .ok()
        .map(to_json)
}

/// Run every dispatched scanner and its scalar fallback over `data` and
/// report the first disagreement.
pub fn scanner_differential(data: &[u8]) -> Result<(), String> {
    mediaprobe::__fuzzing::scanner_differential(data)
}
