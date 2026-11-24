//! Internals for the fuzz harness and the unsafe-code lanes. Not part of the
//! API: present only with the `fuzzing` feature, and free to change in any
//! release.
//!
//! [`scanner_differential`] runs each dispatched byte scanner, which uses
//! AVX2 on x86_64 and NEON on aarch64 when the CPU has them, and compares its
//! answer with the portable scalar scanner the dispatcher falls back to. On a
//! CPU without the extension, or with acceleration switched off by the
//! environment, both sides run the scalar code and the comparison is
//! trivially equal; the out-of-bounds and uninitialised-read checks of Miri,
//! AddressSanitizer and Valgrind still apply to every call.

use crate::scan::{self, scalar};

/// The parameters one input chooses for every scanner: a start offset,
/// needles, EBML ids and MP4 box names, all read from its first bytes so a
/// fuzzer can steer them.
struct Params {
    start: usize,
    needle: u8,
    needles: [u8; 3],
    second_mask: u8,
    second_value: u8,
    ids: [u32; 4],
    names: [[u8; 4]; 3],
}

impl Params {
    fn from(data: &[u8]) -> Self {
        let p = |i: usize| data.get(i).copied().unwrap_or(0);
        Self {
            // Past the end as well, which every scanner must refuse.
            start: usize::from(p(0)) % (data.len() + 2),
            needle: p(1),
            needles: [p(1), p(2), p(3)],
            second_mask: p(2),
            second_value: p(3),
            ids: [0x1F43_B675, 0xA3, 0x1654_AE6B, u32::from(p(2))],
            names: [*b"moov", *b"mdat", [p(3), p(4), p(5), p(6)]],
        }
    }
}

fn same<T: PartialEq + std::fmt::Debug>(
    name: &str,
    accelerated: T,
    scalar: T,
) -> Result<(), String> {
    if accelerated == scalar {
        Ok(())
    } else {
        Err(format!(
            "{name}: dispatched {accelerated:?}, scalar {scalar:?}"
        ))
    }
}

/// Runs every dispatched scanner over `data` and compares each answer with
/// its scalar fallback's. Returns the first difference.
///
/// # Errors
///
/// A description of the first scanner whose answers differ.
pub fn scanner_differential(data: &[u8]) -> Result<(), String> {
    let p = Params::from(data);
    let start = p.start;
    let tail = data.get(start..);

    same(
        "find_byte_from",
        scan::find_byte_from(data, p.needle, start),
        tail.and_then(|t| scalar::find_byte_from(t, p.needle, 0).map(|o| start + o)),
    )?;
    same(
        "find_any_byte_from",
        scan::find_any_byte_from(data, &p.needles, start),
        tail.and_then(|t| scalar::find_any_byte_from(t, &p.needles, 0).map(|o| start + o)),
    )?;
    same(
        "find_pair_from",
        scan::find_pair_from(data, start, p.needle, p.second_mask, p.second_value),
        tail.and_then(|t| {
            scalar::find_pair_from(t, 0, p.needle, p.second_mask, p.second_value).map(|o| start + o)
        }),
    )?;
    same(
        "find_mpeg_start_code",
        scan::find_mpeg_start_code(data, p.needle),
        scalar::find_mpeg_start_code_from(data, p.needle, 0),
    )?;
    same(
        "find_annexb_start_code",
        scan::find_annexb_start_code(data, start),
        scalar::find_annexb_start_code_from(data, start),
    )?;
    same(
        "find_audio_sync_candidate",
        scan::find_audio_sync_candidate(data, start),
        tail.and_then(|t| {
            scalar::find_audio_sync_candidate_from(t, 0).map(|c| scan::AudioSyncCandidate {
                offset: start + c.offset,
                kind: c.kind,
            })
        }),
    )?;
    for ids in [&p.ids[..], &[][..]] {
        same(
            "find_ebml_candidate",
            scan::find_ebml_candidate(data, start, ids),
            tail.filter(|_| !ids.is_empty()).and_then(|t| {
                scalar::find_ebml_candidate_from(t, 0, ids).map(|c| scan::EbmlCandidate {
                    offset: start + c.offset,
                    id: c.id,
                })
            }),
        )?;
    }
    same(
        "find_hevc_sei_nal_header_candidate",
        scan::find_hevc_sei_nal_header_candidate(data, start),
        tail.and_then(|t| {
            scalar::find_hevc_sei_nal_header_candidate_from(t, 0).map(|c| {
                scan::LengthPrefixedNalCandidate {
                    offset: start + c.offset,
                    nal_type: c.nal_type,
                }
            })
        }),
    )?;
    same(
        "find_hdr10plus_itu_t35_candidate",
        scan::find_hdr10plus_itu_t35_candidate(data, start),
        tail.and_then(|t| scalar::find_hdr10plus_itu_t35_candidate_from(t, 0).map(|o| start + o)),
    )?;
    for names in [&p.names[..], &[][..]] {
        same(
            "find_mp4_box_name_candidate",
            scan::find_mp4_box_name_candidate(data, start, names),
            if names.is_empty() || data.len() < 8 {
                None
            } else {
                scalar::find_mp4_box_name_candidate_from(data, start, names)
            },
        )?;
    }
    let mut dispatched = [0_u64; 100];
    let mut fallback = [0_u64; 100];
    scan::accumulate_avi_idx1_stream_sizes(data, &mut dispatched);
    scalar::accumulate_avi_idx1_stream_sizes(data, &mut fallback);
    same("accumulate_avi_idx1_stream_sizes", dispatched, fallback)?;
    for (size, sync) in [
        (188_usize, 0x47_u8),
        (192, 0x47),
        (204, 0x47),
        (188, p.needle),
    ] {
        for sync_offset in [0, size - 188, size - 1, size] {
            same(
                "score_ts_packet_layout",
                scan::score_ts_packet_layout(data, size, sync_offset, sync),
                if sync_offset >= size || data.len() < size {
                    0
                } else {
                    scalar::score_ts_packet_layout(data, size, sync_offset, sync)
                },
            )?;
        }
    }
    same(
        "find_h2645_emulation_prevention_byte",
        scan::find_h2645_emulation_prevention_byte(data, start),
        if data.len() < 3 || start >= data.len() {
            None
        } else {
            scalar::find_h2645_emulation_prevention_byte_from(data, start)
        },
    )?;
    same(
        "find_dts_sync",
        scan::find_dts_sync(data, start),
        tail.and_then(|t| scalar::find_dts_sync_from(t, 0).map(|o| start + o)),
    )?;
    Ok(())
}

/// Runs the structural diagnosis over any reader, as
/// [`diagnose_file`](crate::diagnostics::diagnose_file) does over a file and
/// [`probe_source`](crate::probe_source) does for the probe: the length is
/// learned with one seek to the end, then the diagnosis reads within its own
/// budget. The fuzz targets call it once per input, without a scratch file.
pub fn diagnose_source<R: std::io::Read + std::io::Seek>(
    mut source: R,
    extension: &str,
    options: crate::diagnostics::DiagnosticsOptions,
) -> std::io::Result<crate::StructuralDiagnostics> {
    let length = source.seek(std::io::SeekFrom::End(0))?;
    Ok(crate::diagnostics::diagnose_source(
        &mut crate::source::SizedReader::new(&mut source, length),
        extension,
        options,
    ))
}
