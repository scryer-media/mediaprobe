//! The generator toolchain: which programs the recipes run, the major version
//! each must have, and the encoders FFmpeg must offer.
//!
//! Generation is shape-reproducible, not byte-reproducible: two runs of the
//! same recipe produce files with the same codecs, dimensions, rates, layouts,
//! languages, HDR signalling and chapters, which is everything the
//! expectations record. Encoders stamp versions and timing into their output,
//! so the bytes differ. The exact versions used for a published revision are
//! recorded in its provenance document, not here.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use super::{Context as _, Result, fail};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolchainFile {
    pub(crate) schema_version: u32,
    pub(crate) tools: BTreeMap<String, Tool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tool {
    /// The program name looked up on PATH unless the caller overrides it.
    pub(crate) program: String,
    /// The lowest major version whose output the expectations were written
    /// against.
    pub(crate) min_major: u32,
    /// FFmpeg encoders the recipes need. Empty for other tools.
    #[serde(default)]
    pub(crate) encoders: Vec<String>,
    pub(crate) notes: String,
}

impl ToolchainFile {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let file: Self = super::read_json(path)?;
        if file.schema_version != 1 {
            return fail(format!(
                "{}: unsupported schema_version {}",
                path.display(),
                file.schema_version
            ));
        }
        Ok(file)
    }
}

/// What a probe of one installed tool found.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Probed {
    pub(crate) program: String,
    pub(crate) version_line: String,
    pub(crate) major: Option<u32>,
    pub(crate) missing_encoders: Vec<String>,
}

/// Run `<program> -version` (FFmpeg) or `<program> --version` and pull the
/// first version-looking token out of the first line.
pub(crate) fn probe(program: &Path, encoders: &[String]) -> Result<Probed> {
    let is_ffmpeg = program
        .file_stem()
        .is_some_and(|stem| stem.to_string_lossy().starts_with("ffmpeg"));
    let flag = if is_ffmpeg { "-version" } else { "--version" };
    let output = Command::new(program)
        .arg(flag)
        .stdin(Stdio::null())
        .output()
        .context(|| format!("run {} {flag}", program.display()))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    let version_line = text.lines().next().unwrap_or_default().trim().to_owned();
    let major = parse_major(&version_line);
    let mut missing_encoders = Vec::new();
    if !encoders.is_empty() {
        let listing = Command::new(program)
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .output()
            .context(|| format!("run {} -encoders", program.display()))?;
        let listing = [listing.stdout.as_slice(), listing.stderr.as_slice()].concat();
        for encoder in encoders {
            if !encoder_listed(&listing, encoder) {
                missing_encoders.push(encoder.clone());
            }
        }
    }
    Ok(Probed {
        program: program.display().to_string(),
        version_line,
        major,
        missing_encoders,
    })
}

/// FFmpeg prints `ffmpeg version 8.1.1 ...`, distribution builds print
/// `ffmpeg version 7:7.1.2-1ubuntu1 ...` or `n8.0`, speexenc prints
/// `speexenc (Speex encoder) version 1.2.1 ...`.
pub(crate) fn parse_major(line: &str) -> Option<u32> {
    let after = line.split("version").nth(1)?.trim();
    let token = after.split_whitespace().next()?;
    // An epoch (`7:`) is not the version.
    let token = token.rsplit(':').next()?;
    let token = token.trim_start_matches(|c: char| !c.is_ascii_digit());
    let digits: String = token.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

pub(crate) fn encoder_listed(listing: &[u8], encoder: &str) -> bool {
    listing.split(|byte| *byte == b'\n').any(|line| {
        std::str::from_utf8(line)
            .ok()
            .and_then(|line| line.split_whitespace().nth(1))
            == Some(encoder)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_lines_from_the_builds_in_use() {
        assert_eq!(
            parse_major("ffmpeg version 8.1.1 Copyright (c) 2000-2026 the FFmpeg developers"),
            Some(8)
        );
        assert_eq!(
            parse_major("ffmpeg version 7:7.1.2-1ubuntu1 Copyright (c) 2000-2025"),
            Some(7)
        );
        assert_eq!(parse_major("ffmpeg version n8.0 Copyright"), Some(8));
        assert_eq!(
            parse_major("speexenc (Speex encoder) version 1.2.1 (compiled Jan  1 2026)"),
            Some(1)
        );
        assert_eq!(parse_major("no version here"), None);
    }

    #[test]
    fn encoder_names_match_exactly() {
        let listing = b" V....D libx265  libx265 H.265 / HEVC (codec hevc)\n";
        assert!(encoder_listed(listing, "libx265"));
        assert!(!encoder_listed(listing, "x265"));
    }

    #[test]
    fn committed_toolchain_file_parses() {
        let file = ToolchainFile::load(&super::super::repo_path(super::super::TOOLCHAIN)).unwrap();
        assert!(file.tools.contains_key("ffmpeg"));
    }
}
