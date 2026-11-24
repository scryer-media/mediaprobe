//! The only place the corpus tests call the parser.
//!
//! The expectations compare a flat projection of the result, built here from
//! [`AnalysisDetails`] with the selection rules of `details_facts.rs`, plus
//! the full serialized details under `details`. `mod.rs` and the tests read
//! only that JSON, so a change to the API is absorbed in this file.

use super::Observed;
use super::facts;
use mediaprobe::diagnostics::{DiagnosticsOptions, diagnose_file};
use mediaprobe::{
    AnalysisDetails, ProbeDepth, ProbeOptions, StreamDetail, probe_path, probe_source,
};
use serde_json::{Value, json};
use std::io::Cursor;
use std::path::Path;

/// The options the corpus is probed with: the default budgets at full depth.
pub fn options() -> ProbeOptions {
    ProbeOptions::new().with_depth(ProbeDepth::Full)
}

fn status(diagnosis: Option<Value>) -> Option<String> {
    diagnosis?
        .pointer("/report/status")?
        .as_str()
        .map(str::to_owned)
}

fn audio_entry(stream: &StreamDetail) -> Value {
    json!({
        "codec": stream.codec.as_ref().map(|c| c.as_str().to_owned()),
        "channels": stream.channels,
        "language": facts::known_language(stream),
    })
}

/// The flat view the expectations name with `summary.`, and the top-level
/// fields `compare` reads.
pub fn project(details: &AnalysisDetails) -> Value {
    let audio = facts::audio(details);
    let (width, height) = facts::video_size(details);
    let (dovi_profile, dovi_compat) = facts::dovi(details);
    json!({
        "container_format": details.container_format,
        "video_codec": facts::video_codec(details),
        "video_width": width,
        "video_height": height,
        "video_frame_rate": facts::video_fps(details),
        "video_bit_depth": facts::video_bit_depth(details),
        "video_hdr_format": facts::video(details)
            .and_then(StreamDetail::hdr_format)
            .map(|format| format.to_string()),
        "dovi_profile": dovi_profile,
        "dovi_bl_compat_id": dovi_compat,
        "duration_seconds": details.duration_seconds,
        "audio_streams": audio.iter().map(|s| audio_entry(s)).collect::<Vec<_>>(),
        "audio_profile": facts::primary_audio(details).and_then(|a| a.metadata.profile.clone()),
        "has_multiaudio": audio.len() > 1,
        "num_chapters": facts::chapter_count(details),
        "subtitle_languages": facts::subtitle_languages(details),
        "subtitle_codecs": facts::subtitle_codecs(details),
        "details": serde_json::to_value(details).expect("details serialize"),
    })
}

/// Probe a corpus file the way an application would: through the path API,
/// which also routes disc images to the disc reader.
pub fn observe_file(path: &Path) -> Observed {
    let analysis = probe_path(path, &options())
        .map(|details| project(&details))
        .map_err(|e| e.to_string());
    let diagnosis = diagnose_file(path, DiagnosticsOptions::default())
        .ok()
        .map(|d| serde_json::to_value(d).expect("diagnosis serializes"));
    Observed {
        analysis,
        diagnostic_status: status(diagnosis),
    }
}

/// Probe bytes from memory with an extension hint. Returns the projection or
/// the error text. Panics propagate to the caller, which is the point of the
/// robustness test.
pub fn analyze_bytes(bytes: &[u8], extension: &str) -> Result<Value, String> {
    probe_source(Cursor::new(bytes), extension, &options())
        .map(|details| project(&details))
        .map_err(|e| e.to_string())
}

/// Probe a file through a caller-supplied reader with an extension hint,
/// without loading it: large fixtures run past 4 GiB.
pub fn analyze_file(path: &Path, extension: &str) -> Result<Value, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    probe_source(file, extension, &options())
        .map(|details| project(&details))
        .map_err(|e| e.to_string())
}

/// Run the structural diagnosis over bytes, through a scratch file: the
/// public diagnosis entry point takes a path. What it cannot read it reports
/// inside the diagnosis; an error here is the scratch file's.
pub fn diagnose_bytes(bytes: &[u8], extension: &str) -> Value {
    let dir = std::env::temp_dir().join(format!("mediaprobe-diagnose-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating the scratch directory");
    let path = dir.join(format!("input.{extension}"));
    std::fs::write(&path, bytes).expect("writing the scratch input");
    let diagnosis = diagnose_file(&path, DiagnosticsOptions::default());
    let _ = std::fs::remove_file(&path);
    match diagnosis {
        Ok(d) => serde_json::to_value(d).expect("diagnosis serializes"),
        Err(e) => json!({ "error": e.to_string() }),
    }
}
