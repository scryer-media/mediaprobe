//! The accelerated scanners (AVX2 on x86_64, NEON on aarch64) against the
//! scalar path, on in-memory inputs built to sit on every vector boundary.
//!
//! This is the test the Miri, AddressSanitizer and Valgrind lanes run: each
//! input goes through `mediaprobe::__fuzzing::scanner_differential`, which
//! runs every dispatched scanner and compares its answer with the scalar
//! fallback's. Under those tools any out-of-bounds or uninitialised read in
//! the unsafe code fails the run; natively it is a differential test. Miri
//! interprets slowly, so it gets a smaller sweep of the same shapes.
use mediaprobe::__fuzzing::scanner_differential;

/// Byte patterns the scanners look for, each placed across lane boundaries.
const PATTERNS: &[&[u8]] = &[
    &[0x00, 0x00, 0x01, 0xB3],             // MPEG sequence header
    &[0x00, 0x00, 0x00, 0x01, 0x67],       // Annex B four-byte start, H.264 SPS
    &[0x00, 0x00, 0x03, 0x01],             // emulation prevention
    &[0x00, 0x00, 0x01, 0x4E, 0x01],       // HEVC prefix SEI header
    &[0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04], // HDR10+ ITU-T T.35 prefix
    &[0x1A, 0x45, 0xDF, 0xA3],             // EBML header
    &[0x1F, 0x43, 0xB6, 0x75],             // Matroska cluster
    b"moov",
    b"mdat",
    &[0x7F, 0xFE, 0x80, 0x01], // DTS core
    &[0x0B, 0x77],             // AC-3
    &[0xFF, 0xF1],             // ADTS
    &[0x56, 0xE0],             // LATM
    &[0xFF, 0xFB],             // MPEG audio layer III
    b"00dc",                   // AVI idx1 stream prefix
];

/// Lengths around the 16- and 32-byte vector widths and their multiples.
#[cfg(not(miri))]
const LENGTHS: &[usize] = &[
    0, 1, 2, 3, 4, 7, 8, 15, 16, 17, 31, 32, 33, 47, 48, 63, 64, 65, 95, 96, 127, 128, 129, 191,
    192, 255, 256, 257, 383, 384, 600,
];
#[cfg(miri)]
const LENGTHS: &[usize] = &[0, 1, 3, 15, 16, 17, 31, 32, 33, 64, 65, 129];

/// The differential reads its needles and start offset from the input's
/// first bytes; these headers pick needles that occur in the patterns.
const HEADERS: &[[u8; 7]] = &[
    [0x00, 0x00, 0x01, 0xB3, 0x7F, 0xFE, 0x80],
    [0x03, 0x47, 0x0B, 0x6D, 0x6F, 0x6F, 0x76],
    [0x11, 0xFF, 0xF0, 0xF0, 0x6D, 0x64, 0x61],
];

fn inputs() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for &len in LENGTHS {
        for header in HEADERS {
            // The bare header and zero fill.
            let mut base = vec![0u8; len];
            let n = len.min(header.len());
            base[..n].copy_from_slice(&header[..n]);
            out.push(base.clone());
            for pattern in PATTERNS {
                if pattern.len() > len {
                    continue;
                }
                // At the start, straddling each lane boundary, and at the end.
                let mut offsets = vec![0, len - pattern.len()];
                for lane in [16usize, 32] {
                    let mut at = lane;
                    while at < len {
                        offsets.extend([at - 1, at.saturating_sub(pattern.len() / 2)]);
                        at += lane;
                    }
                }
                offsets.sort_unstable();
                offsets.dedup();
                #[cfg(miri)]
                offsets.truncate(3);
                for at in offsets {
                    if at + pattern.len() > len {
                        continue;
                    }
                    let mut data = base.clone();
                    data[at..at + pattern.len()].copy_from_slice(pattern);
                    out.push(data);
                }
            }
            // A buffer of matches: every position a candidate.
            if len > 0 {
                out.push(PATTERNS.concat().into_iter().cycle().take(len).collect());
            }
        }
    }
    // Transport stream packet layouts at each packet size.
    for size in [188usize, 192, 204] {
        let mut data = vec![0xFFu8; size * 3 + 5];
        for k in 0..3 {
            data[5 + k * size + size - 188] = 0x47;
        }
        out.push(data);
    }
    out
}

#[test]
fn accelerated_and_scalar_scanners_agree() {
    let inputs = inputs();
    let mut divergent = Vec::new();
    for data in &inputs {
        if let Err(divergence) = scanner_differential(data) {
            divergent.push(format!(
                "{} bytes {:02X?}: {divergence}",
                data.len(),
                &data[..data.len().min(16)]
            ));
        }
    }
    assert!(
        divergent.is_empty(),
        "{} of {} inputs diverge:\n{}",
        divergent.len(),
        inputs.len(),
        divergent.join("\n")
    );
}
