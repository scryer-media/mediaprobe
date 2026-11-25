//! The H.264 readers on hand-built and hostile parameter sets, and checked
//! against `scuffle-h264` on those and on every H.264 parameter set in the
//! hydrated corpus.

use super::*;
use crate::codec::bits::{
    BitReader, EXP_GOLOMB_ZERO_RUN, TestBitWriter, escape_rbsp, has_terminated_zero_run, read_se,
    read_ue,
};
use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Picture order count syntax of a built SPS.
#[derive(Clone)]
enum Poc {
    Type0(u64),
    Type1 {
        non_ref: i64,
        top_to_bottom: i64,
        cycle: Vec<i64>,
        declared: u64,
    },
    Type2,
}

/// Video signal type of a built SPS: format, full range, description.
type Signal = (u64, bool, Option<(u8, u8, u8)>);

#[derive(Clone, Default)]
struct Vui {
    aspect: Option<(u8, u16, u16)>,
    overscan: Option<bool>,
    signal: Option<Signal>,
    chroma_loc: Option<(u64, u64)>,
    timing: Option<(u32, u32)>,
    /// Writes the VUI fields after the timing information as well.
    tail: bool,
}

/// Every field of an SPS up to the VUI, as written.
#[derive(Clone)]
struct SpsSpec {
    header: u64,
    profile_idc: u64,
    constraint_flags: u64,
    level_idc: u64,
    sps_id: u64,
    chroma_format_idc: u64,
    separate_colour_plane: bool,
    bit_depth_luma_minus8: u64,
    bit_depth_chroma_minus8: u64,
    /// Empty for no scaling matrix; otherwise one entry per list.
    scaling_lists: Vec<Option<Vec<i64>>>,
    log2_max_frame_num_minus4: u64,
    poc: Poc,
    max_num_ref_frames: u64,
    width_mbs_minus1: u64,
    height_map_units_minus1: u64,
    frame_mbs_only: bool,
    crop: Option<[u64; 4]>,
    vui: Option<Vui>,
}

impl SpsSpec {
    /// A 4:2:0 8-bit 320x240 SPS of `profile_idc`, without VUI.
    fn new(profile_idc: u64) -> Self {
        Self {
            header: 0x67,
            profile_idc,
            constraint_flags: 0,
            level_idc: 30,
            sps_id: 0,
            chroma_format_idc: 1,
            separate_colour_plane: false,
            bit_depth_luma_minus8: 0,
            bit_depth_chroma_minus8: 0,
            scaling_lists: Vec::new(),
            log2_max_frame_num_minus4: 0,
            poc: Poc::Type2,
            max_num_ref_frames: 1,
            width_mbs_minus1: 19,
            height_map_units_minus1: 14,
            frame_mbs_only: true,
            crop: None,
            vui: None,
        }
    }

    /// The SPS with every optional VUI field present.
    fn full_vui(mut self) -> Self {
        self.vui = Some(Vui {
            aspect: Some((255, 4, 3)),
            overscan: Some(true),
            signal: Some((5, true, Some((9, 16, 9)))),
            chroma_loc: Some((2, 1)),
            timing: Some((1001, 60000)),
            tail: true,
        });
        self
    }

    fn rbsp(&self) -> Vec<u8> {
        let mut out = TestBitWriter::new();
        out.write_bits(self.header, 8);
        out.write_bits(self.profile_idc, 8);
        out.write_bits(self.constraint_flags, 8);
        out.write_bits(self.level_idc, 8);
        out.write_ue(self.sps_id);
        if has_ext(self.profile_idc) {
            out.write_ue(self.chroma_format_idc);
            if self.chroma_format_idc == 3 {
                out.write_bits(u64::from(self.separate_colour_plane), 1);
            }
            out.write_ue(self.bit_depth_luma_minus8);
            out.write_ue(self.bit_depth_chroma_minus8);
            out.write_bits(0, 1); // qpprime_y_zero_transform_bypass_flag
            write_scaling_lists(&mut out, &self.scaling_lists);
        }
        out.write_ue(self.log2_max_frame_num_minus4);
        match &self.poc {
            Poc::Type0(lsb) => {
                out.write_ue(0);
                out.write_ue(*lsb);
            }
            Poc::Type1 {
                non_ref,
                top_to_bottom,
                cycle,
                declared,
            } => {
                out.write_ue(1);
                out.write_bits(0, 1);
                out.write_se(*non_ref);
                out.write_se(*top_to_bottom);
                out.write_ue(*declared);
                for offset in cycle {
                    out.write_se(*offset);
                }
            }
            Poc::Type2 => out.write_ue(2),
        }
        out.write_ue(self.max_num_ref_frames);
        out.write_bits(0, 1); // gaps_in_frame_num_value_allowed_flag
        out.write_ue(self.width_mbs_minus1);
        out.write_ue(self.height_map_units_minus1);
        out.write_bits(u64::from(self.frame_mbs_only), 1);
        if !self.frame_mbs_only {
            out.write_bits(1, 1); // mb_adaptive_frame_field_flag
        }
        out.write_bits(1, 1); // direct_8x8_inference_flag
        out.write_bits(u64::from(self.crop.is_some()), 1);
        for offset in self.crop.iter().flatten() {
            out.write_ue(*offset);
        }
        out.write_bits(u64::from(self.vui.is_some()), 1);
        if let Some(vui) = &self.vui {
            write_vui(&mut out, vui);
        }
        out.write_bits(1, 1); // rbsp_stop_one_bit
        out.finish()
    }

    /// The SPS as a NAL unit, emulation prevention bytes added.
    fn nal(&self) -> Vec<u8> {
        escape_rbsp(&self.rbsp())
    }
}

fn has_ext(profile_idc: u64) -> bool {
    matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    )
}

fn write_scaling_lists(out: &mut TestBitWriter, lists: &[Option<Vec<i64>>]) {
    out.write_bits(u64::from(!lists.is_empty()), 1);
    for list in lists {
        out.write_bits(u64::from(list.is_some()), 1);
        for delta in list.iter().flatten() {
            out.write_se(*delta);
        }
    }
}

fn write_vui(out: &mut TestBitWriter, vui: &Vui) {
    out.write_bits(u64::from(vui.aspect.is_some()), 1);
    if let Some((idc, width, height)) = vui.aspect {
        out.write_bits(u64::from(idc), 8);
        if idc == 255 {
            out.write_bits(u64::from(width), 16);
            out.write_bits(u64::from(height), 16);
        }
    }
    out.write_bits(u64::from(vui.overscan.is_some()), 1);
    if let Some(appropriate) = vui.overscan {
        out.write_bits(u64::from(appropriate), 1);
    }
    out.write_bits(u64::from(vui.signal.is_some()), 1);
    if let Some((format, full, description)) = vui.signal {
        out.write_bits(format, 3);
        out.write_bits(u64::from(full), 1);
        out.write_bits(u64::from(description.is_some()), 1);
        if let Some((primaries, transfer, matrix)) = description {
            out.write_bits(u64::from(primaries), 8);
            out.write_bits(u64::from(transfer), 8);
            out.write_bits(u64::from(matrix), 8);
        }
    }
    out.write_bits(u64::from(vui.chroma_loc.is_some()), 1);
    if let Some((top, bottom)) = vui.chroma_loc {
        out.write_ue(top);
        out.write_ue(bottom);
    }
    out.write_bits(u64::from(vui.timing.is_some()), 1);
    if let Some((units, scale)) = vui.timing {
        out.write_bits(u64::from(units), 32);
        out.write_bits(u64::from(scale), 32);
        out.write_bits(1, 1); // fixed_frame_rate_flag
    }
    if vui.tail {
        out.write_bits(0, 2); // no NAL or VCL HRD parameters
        out.write_bits(0, 1); // pic_struct_present_flag
        out.write_bits(1, 1); // bitstream_restriction_flag
        out.write_bits(1, 1); // motion_vectors_over_pic_boundaries_flag
        for value in [0, 1, 16, 16, 2, 4] {
            out.write_ue(value);
        }
    }
}

/// An avcC record holding `sps` and `pps`, and an extension when
/// `extension` is given: chroma format, luma and chroma bit depth, and
/// the SPS extension entries.
fn avcc(sps: &[&[u8]], pps: &[&[u8]], extension: Option<(u8, u8, u8, &[&[u8]])>) -> Vec<u8> {
    let profile = sps
        .first()
        .and_then(|nal| nal.get(1))
        .copied()
        .unwrap_or(66);
    let mut out = vec![1, profile, 0, 30, 0xFF, 0xE0 | sps.len() as u8];
    for nal in sps {
        out.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out.push(pps.len() as u8);
    for nal in pps {
        out.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        out.extend_from_slice(nal);
    }
    if let Some((chroma, luma, chroma_depth, entries)) = extension {
        out.extend_from_slice(&[0xFC | chroma, 0xF8 | luma, 0xF8 | chroma_depth]);
        out.push(entries.len() as u8);
        for entry in entries {
            out.extend_from_slice(&(entry.len() as u16).to_be_bytes());
            out.extend_from_slice(entry);
        }
    }
    out
}

const PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];

// ---------------------------------------------------------------------------
// The crate's behaviour before this reader: guards in front of scuffle-h264.
// ---------------------------------------------------------------------------

/// Zero bits in a row that can start no Exp-Golomb code the dependency can
/// read without overflowing: codes shorter than this hold values below
/// 2^48, far from any of its unchecked additions.
const SCUFFLE_SAFE_ZERO_RUN: usize = 48;

/// What the crate did with an SPS before it had its own reader.
enum Before {
    /// A long zero run the old guard refused. The dependency may panic on
    /// such input, so it is not called.
    ZeroRun,
    /// Refused by the old scaling matrix guard.
    Guard,
    /// The dependency's result.
    Parsed(Result<scuffle_h264::Sps, ()>),
}

fn before(nal: &[u8]) -> Before {
    let rbsp = scan::h2645_unescape_rbsp(nal);
    if has_terminated_zero_run(&rbsp, 0, EXP_GOLOMB_ZERO_RUN) {
        return Before::ZeroRun;
    }
    if old_check_scaling_matrices(&rbsp).is_none() {
        return Before::Guard;
    }
    Before::Parsed(scuffle_h264::Sps::parse(Cursor::new(rbsp.as_ref())).map_err(drop))
}

/// The scaling matrix guard the crate ran before the dependency.
fn old_check_scaling_matrices(rbsp: &[u8]) -> Option<()> {
    let mut bits = BitReader::new(rbsp);
    bits.skip_bits(8)?;
    let profile_idc = bits.read_bits(8)?;
    bits.skip_bits(16)?;
    read_ue(&mut bits)?;
    if !has_ext(u64::from(profile_idc)) {
        return Some(());
    }
    let chroma_format_idc = read_ue(&mut bits)?;
    if chroma_format_idc == 3 {
        bits.skip_bits(1)?;
    }
    read_ue(&mut bits)?;
    read_ue(&mut bits)?;
    bits.skip_bits(1)?;
    if bits.read_bit()? == 0 {
        return Some(());
    }
    let lists = if chroma_format_idc == 3 { 12 } else { 8 };
    for list in 0..lists {
        if bits.read_bit()? == 0 {
            continue;
        }
        let mut next_scale = 8;
        for _ in 0..if list < 6 { 16 } else { 64 } {
            let delta_scale = read_se(&mut bits)?;
            if !(-128..=127).contains(&delta_scale) {
                return None;
            }
            next_scale = (next_scale + delta_scale + 256) % 256;
            if next_scale == 0 {
                break;
            }
        }
    }
    Some(())
}

/// The picture size the dependency's SPS fields describe, with the crop
/// units of the specification.
fn expected_dimensions(sps: &scuffle_h264::Sps) -> Option<(i32, i32)> {
    let crop = sps.frame_crop_info.as_ref();
    let cropped = |units: u64, scale: u64, crop_unit: u64, low: u64, high: u64| {
        let full = units.checked_add(1)?.checked_mul(16)?.checked_mul(scale)?;
        let lost = low.checked_add(high)?.checked_mul(crop_unit)?;
        i32::try_from(full.checked_sub(lost)?).ok()
    };
    let field_scale = if sps.mb_adaptive_frame_field_flag.is_none() {
        1
    } else {
        2
    };
    let chroma = sps.ext.as_ref().map_or(1, |ext| ext.chroma_format_idc);
    let (unit_x, unit_y) = match chroma {
        1 => (2, 2 * field_scale),
        2 => (2, field_scale),
        _ => (1, field_scale),
    };
    let width = cropped(
        sps.pic_width_in_mbs_minus1,
        1,
        unit_x,
        crop.map_or(0, |c| c.frame_crop_left_offset),
        crop.map_or(0, |c| c.frame_crop_right_offset),
    )?;
    let height = cropped(
        sps.pic_height_in_map_units_minus1,
        field_scale,
        unit_y,
        crop.map_or(0, |c| c.frame_crop_top_offset),
        crop.map_or(0, |c| c.frame_crop_bottom_offset),
    )?;
    Some((width, height))
}

/// Every field this crate exposes equals the dependency's.
fn assert_same_sps(ours: &Sps, theirs: &scuffle_h264::Sps, label: &str) {
    assert_eq!(ours.profile_idc, theirs.profile_idc, "{label}");
    assert_eq!(
        ours.constraint_set1_flag, theirs.constraint_set1_flag,
        "{label}"
    );
    assert_eq!(ours.level_idc, theirs.level_idc, "{label}");
    assert_eq!(
        ours.chroma_format_idc,
        theirs.ext.as_ref().map_or(1, |ext| ext.chroma_format_idc),
        "{label}"
    );
    assert_eq!(
        ours.bit_depth_luma_minus8,
        theirs.ext.as_ref().map(|ext| ext.bit_depth_luma_minus8),
        "{label}"
    );
    assert_eq!(
        u64::from(ours.pic_width_in_mbs_minus1),
        theirs.pic_width_in_mbs_minus1,
        "{label}"
    );
    assert_eq!(
        u64::from(ours.pic_height_in_map_units_minus1),
        theirs.pic_height_in_map_units_minus1,
        "{label}"
    );
    assert_eq!(
        ours.frame_mbs_only_flag,
        theirs.mb_adaptive_frame_field_flag.is_none(),
        "{label}"
    );
    assert_eq!(
        ours.frame_crop
            .map(|c| [c.left, c.right, c.top, c.bottom].map(u64::from)),
        theirs.frame_crop_info.as_ref().map(|c| [
            c.frame_crop_left_offset,
            c.frame_crop_right_offset,
            c.frame_crop_top_offset,
            c.frame_crop_bottom_offset,
        ]),
        "{label}"
    );
    assert_eq!(
        ours.sample_aspect_ratio
            .map(|sar| (sar.aspect_ratio_idc, sar.sar_width, sar.sar_height)),
        theirs.sample_aspect_ratio.as_ref().map(|sar| (
            sar.aspect_ratio_idc.0,
            sar.sar_width,
            sar.sar_height
        )),
        "{label}"
    );
    assert_eq!(
        ours.color
            .map(|c| (c.full_range, c.primaries, c.transfer, c.matrix)),
        theirs.color_config.as_ref().map(|c| (
            c.video_full_range_flag,
            c.color_primaries,
            c.transfer_characteristics,
            c.matrix_coefficients
        )),
        "{label}"
    );
    assert_eq!(
        ours.chroma_sample_loc_type_top_field,
        theirs
            .chroma_sample_loc
            .as_ref()
            .map(|loc| loc.chroma_sample_loc_type_top_field),
        "{label}"
    );
    assert_eq!(
        ours.timing.map(|t| (t.num_units_in_tick, t.time_scale)),
        theirs
            .timing_info
            .as_ref()
            .map(|t| (t.num_units_in_tick, t.time_scale)),
        "{label}"
    );
    assert_eq!(
        ours.frame_rate().map(f64::to_bits),
        theirs.frame_rate().map(f64::to_bits),
        "{label}"
    );
    assert_eq!(sps_dimensions(ours), expected_dimensions(theirs), "{label}");
}

/// How one SPS compared with the crate's earlier behaviour.
#[derive(Debug, Default)]
struct Tally {
    /// Both accepted, every field equal.
    same: usize,
    /// Both refused.
    refused: usize,
    /// Refused now for an Exp-Golomb code wider than 32 bits, accepted
    /// before.
    wide_code: usize,
    /// Held a zero run the old guard refused; the dependency not called.
    zero_run: usize,
}

/// Checks the new reader against the crate's earlier behaviour on one SPS
/// NAL unit and records the outcome.
fn compare_sps(nal: &[u8], label: &str, tally: &mut Tally) {
    let ours = parse_sps(nal);
    match before(nal) {
        Before::ZeroRun => tally.zero_run += 1,
        Before::Guard | Before::Parsed(Err(())) => {
            assert!(ours.is_none(), "{label}: refused before, accepted now");
            tally.refused += 1;
        }
        Before::Parsed(Ok(theirs)) => match ours {
            Some(ours) => {
                assert_same_sps(&ours, &theirs, label);
                tally.same += 1;
            }
            None => {
                let rbsp = scan::h2645_unescape_rbsp(nal);
                assert!(
                    has_terminated_zero_run(&rbsp, 0, 32),
                    "{label}: accepted before, refused now"
                );
                tally.wide_code += 1;
            }
        },
    }
}

/// The dependency's avcC parse, or `None` when the record holds a zero
/// run that could overflow it.
fn scuffle_avcc(data: &[u8]) -> Option<Result<scuffle_h264::AVCDecoderConfigurationRecord, ()>> {
    if has_terminated_zero_run(data, 0, SCUFFLE_SAFE_ZERO_RUN) {
        return None;
    }
    Some(
        scuffle_h264::AVCDecoderConfigurationRecord::parse(&mut Cursor::new(
            bytes::Bytes::copy_from_slice(data),
        ))
        .map_err(drop),
    )
}

/// What `is_valid_h264_avcc` returned before, or `None` when the
/// dependency could not be called safely.
fn old_is_valid_avcc(data: &[u8]) -> Option<bool> {
    let Ok(config) = scuffle_avcc(data)? else {
        return Some(false);
    };
    if config.configuration_version != 1 || config.sps.is_empty() || config.pps.is_empty() {
        return Some(false);
    }
    let mut any = false;
    for nal in &config.sps {
        match before(nal) {
            Before::ZeroRun => return None,
            Before::Parsed(Ok(_)) => any = true,
            _ => {}
        }
    }
    Some(any)
}

/// Checks the avcC reader against the dependency on one record. Returns
/// whether the dependency was called.
fn compare_avcc(data: &[u8], label: &str, both_accepted: &mut usize) -> bool {
    let ours = AvcConfig::parse(data);
    let Some(theirs) = scuffle_avcc(data) else {
        return false;
    };
    match (ours, theirs) {
        (Some(ours), Ok(theirs)) => {
            assert_eq!(
                ours.configuration_version, theirs.configuration_version,
                "{label}"
            );
            assert_eq!(
                ours.profile_indication, theirs.profile_indication,
                "{label}"
            );
            assert_eq!(
                ours.sps,
                theirs.sps.iter().map(|b| b.as_ref()).collect::<Vec<_>>(),
                "{label}"
            );
            assert_eq!(usize::from(ours.pps_count), theirs.pps.len(), "{label}");
            assert_eq!(
                ours.bit_depth_luma_minus8,
                theirs
                    .extended_config
                    .as_ref()
                    .map(|ext| ext.bit_depth_luma_minus8),
                "{label}"
            );
            *both_accepted += 1;
        }
        (None, Err(())) => {}
        (ours, theirs) => panic!(
            "{label}: ours {:?}, theirs {:?}",
            ours.is_some(),
            theirs.is_ok()
        ),
    }
    if let Some(valid) = old_is_valid_avcc(data) {
        assert_eq!(is_valid_h264_avcc(data), valid, "{label}");
    }
    true
}

// ---------------------------------------------------------------------------
// Hand-built parameter sets.
// ---------------------------------------------------------------------------

/// A spread of valid SPSs covering every branch the reader takes.
fn hand_built() -> Vec<SpsSpec> {
    let mut specs = Vec::new();
    for profile in [
        66, 77, 88, 100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135,
    ] {
        specs.push(SpsSpec::new(profile));
        specs.push(SpsSpec::new(profile).full_vui());
    }
    let mut constrained = SpsSpec::new(66);
    constrained.constraint_flags = 0xFC;
    specs.push(constrained);
    for profile in [44, 100, 110, 122, 244, 77] {
        let mut spec = SpsSpec::new(profile);
        spec.constraint_flags = 0xFC;
        specs.push(spec);
    }
    for chroma in 0..=3 {
        for separate in [false, true] {
            let mut spec = SpsSpec::new(244);
            spec.chroma_format_idc = chroma;
            spec.separate_colour_plane = separate && chroma == 3;
            spec.bit_depth_luma_minus8 = 6;
            spec.bit_depth_chroma_minus8 = 2;
            let lists = if chroma == 3 { 12 } else { 8 };
            spec.scaling_lists = (0..lists)
                .map(|list| {
                    let size = if list < 6 { 16 } else { 64 };
                    (list % 3 != 1).then(|| {
                        if list % 3 == 0 {
                            vec![1; size]
                        } else {
                            vec![-8]
                        }
                    })
                })
                .collect();
            specs.push(spec);
        }
    }
    let mut boundary = vec![127, -128];
    boundary.resize(16, 0);
    for deltas in [boundary, vec![-8], vec![120, 120, 8]] {
        let mut spec = SpsSpec::new(100);
        spec.scaling_lists = vec![None; 8];
        spec.scaling_lists[0] = Some(deltas);
        specs.push(spec);
    }
    for poc in [
        Poc::Type0(12),
        Poc::Type1 {
            non_ref: -(i64::from(i32::MAX)),
            top_to_bottom: i64::from(i32::MAX),
            cycle: vec![3, -3, i64::from(i32::MAX)],
            declared: 3,
        },
        Poc::Type1 {
            non_ref: 0,
            top_to_bottom: 0,
            cycle: vec![1; 255],
            declared: 255,
        },
    ] {
        let mut spec = SpsSpec::new(77);
        spec.poc = poc;
        spec.log2_max_frame_num_minus4 = 12;
        specs.push(spec);
    }
    let mut interlaced = SpsSpec::new(77).full_vui();
    interlaced.frame_mbs_only = false;
    interlaced.crop = Some([0, 0, 0, 4]);
    specs.push(interlaced);
    let mut cropped = SpsSpec::new(100);
    cropped.width_mbs_minus1 = 119;
    cropped.height_map_units_minus1 = 67;
    cropped.crop = Some([1, 2, 3, 4]);
    specs.push(cropped);
    let mut large = SpsSpec::new(66);
    large.width_mbs_minus1 = u64::from(u32::MAX - 1);
    large.height_map_units_minus1 = u64::from(u32::MAX - 1);
    large.max_num_ref_frames = u64::from(u32::MAX - 1);
    large.crop = Some([u64::from(u32::MAX - 1); 4]);
    specs.push(large);
    for vui in [
        Vui::default(),
        Vui {
            aspect: Some((14, 0, 0)),
            ..Default::default()
        },
        Vui {
            aspect: Some((255, 0, 0)),
            signal: Some((0, false, None)),
            ..Default::default()
        },
        Vui {
            overscan: Some(false),
            signal: Some((7, false, Some((1, 18, 0)))),
            timing: Some((u32::MAX, u32::MAX)),
            ..Default::default()
        },
        Vui {
            chroma_loc: Some((5, 5)),
            timing: Some((1, 1)),
            ..Default::default()
        },
    ] {
        let mut spec = SpsSpec::new(100);
        spec.vui = Some(vui);
        specs.push(spec);
    }
    specs
}

#[test]
fn hand_built_spss_read_as_scuffle_reads_them() {
    let mut tally = Tally::default();
    for (index, spec) in hand_built().iter().enumerate() {
        let nal = spec.nal();
        assert!(parse_sps(&nal).is_some(), "hand-built {index} parses");
        compare_sps(&nal, &format!("hand-built {index}"), &mut tally);
        for len in 0..nal.len() {
            compare_sps(
                &nal[..len],
                &format!("hand-built {index} cut at {len}"),
                &mut tally,
            );
        }
    }
    assert_eq!(tally.wide_code + tally.zero_run, 0, "{tally:?}");
    assert!(tally.same > 60, "{tally:?}");
}

#[test]
fn a_full_sps_reads_every_field() {
    let mut spec = SpsSpec::new(110).full_vui();
    spec.bit_depth_luma_minus8 = 2;
    spec.constraint_flags = 0x40;
    spec.crop = Some([0, 0, 0, 4]);
    spec.frame_mbs_only = false;
    let sps = parse_sps(&spec.nal()).expect("parses");
    assert_eq!(
        sps,
        Sps {
            profile_idc: 110,
            constraint_set1_flag: false,
            level_idc: 30,
            chroma_format_idc: 1,
            bit_depth_luma_minus8: Some(2),
            pic_width_in_mbs_minus1: 19,
            pic_height_in_map_units_minus1: 14,
            frame_mbs_only_flag: false,
            frame_crop: Some(sps::FrameCrop {
                left: 0,
                right: 0,
                top: 0,
                bottom: 4
            }),
            sample_aspect_ratio: Some(sps::SampleAspectRatio {
                aspect_ratio_idc: 255,
                sar_width: 4,
                sar_height: 3
            }),
            color: Some(sps::ColorConfig {
                full_range: true,
                primaries: 9,
                transfer: 16,
                matrix: 9
            }),
            chroma_sample_loc_type_top_field: Some(2),
            timing: Some(sps::Timing {
                num_units_in_tick: std::num::NonZeroU32::new(1001).expect("nonzero"),
                time_scale: std::num::NonZeroU32::new(60000).expect("nonzero"),
            }),
        }
    );
    assert_eq!(sps_dimensions(&sps), Some((320, 464)));
    assert_eq!(sps.frame_rate(), Some(60000.0 / 2002.0));
    let mut baseline = SpsSpec::new(66);
    baseline.constraint_flags = 0x40;
    let sps = parse_sps(&baseline.nal()).expect("parses");
    assert!(sps.constraint_set1_flag);
    assert_eq!(
        (sps.chroma_format_idc, sps.bit_depth_luma_minus8),
        (1, None)
    );
}

#[test]
fn an_h264_sps_with_scaling_matrices_parses_as_before() {
    let mut boundary = vec![127, -128];
    boundary.resize(16, 0);
    // A list that ends early when its scale reaches zero.
    for deltas in [boundary, vec![-8]] {
        let mut spec = SpsSpec::new(100);
        spec.width_mbs_minus1 = 3;
        spec.height_map_units_minus1 = 2;
        spec.scaling_lists = vec![None; 8];
        spec.scaling_lists[0] = Some(deltas);
        let sps = parse_sps(&spec.nal()).expect("SPS parses");
        assert_eq!(sps_dimensions(&sps), Some((64, 48)));
        let mut tally = Tally::default();
        compare_sps(&spec.nal(), "scaling", &mut tally);
        assert_eq!(tally.same, 1);
    }
}

#[test]
fn h264_profile_mapping() {
    assert_eq!(map_h264_profile(66).as_deref(), Some("Baseline"));
    assert_eq!(map_h264_profile(77).as_deref(), Some("Main"));
    assert_eq!(map_h264_profile(100).as_deref(), Some("High"));
    assert_eq!(map_h264_profile(110).as_deref(), Some("High 10"));
    assert_eq!(map_h264_profile(0), None);
}

// ---------------------------------------------------------------------------
// Hostile parameter sets.
// ---------------------------------------------------------------------------

/// A baseline SPS whose first Exp-Golomb value has a 64-bit zero prefix.
const SPS_WITH_LONG_ZERO_PREFIX: [u8; 22] = [
    0x67, 0x42, 0x00, 0x0A, 0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF,
];

#[test]
fn an_sps_with_an_overlong_exp_golomb_prefix_is_refused() {
    assert!(parse_sps(&SPS_WITH_LONG_ZERO_PREFIX).is_none());
    // Every prefix from 32 to 100 zero bits, with and without value bits.
    for zeros in 32..=100 {
        for value in [0_u64, u64::MAX] {
            let mut out = TestBitWriter::new();
            out.write_bits(0x6742000A, 32);
            for _ in 0..zeros {
                out.write_bits(0, 1);
            }
            out.write_bits(1, 1);
            for _ in 0..zeros {
                out.write_bits(value & 1, 1);
            }
            out.write_bits(0xFF, 8);
            let rbsp = out.finish();
            assert!(Sps::parse(&rbsp).is_none(), "{zeros} zeros");
        }
    }
}

/// Sets one Exp-Golomb field of `spec` to `value`, by index.
fn with_ue_field(mut spec: SpsSpec, field: usize, value: u64) -> SpsSpec {
    match field {
        0 => spec.max_num_ref_frames = value,
        1 => spec.width_mbs_minus1 = value,
        2 => spec.height_map_units_minus1 = value,
        3..=6 => {
            let mut crop = spec.crop.unwrap_or([0; 4]);
            crop[field - 3] = value;
            spec.crop = Some(crop);
        }
        _ => unreachable!(),
    }
    spec
}

#[test]
fn values_past_32_bits_are_refused_and_values_at_the_limit_kept() {
    let limit = u64::from(u32::MAX - 1);
    for field in 0..=6 {
        let spec = with_ue_field(SpsSpec::new(77).full_vui(), field, limit);
        let sps = parse_sps(&spec.nal()).expect("value at the limit parses");
        let mut tally = Tally::default();
        compare_sps(&spec.nal(), &format!("field {field} at limit"), &mut tally);
        assert_eq!(tally.same, 1, "field {field}");
        if field > 0 {
            assert_eq!(sps_dimensions(&sps), None, "field {field}");
        }
        for past in [limit + 1, 1 << 40] {
            let spec = with_ue_field(SpsSpec::new(77).full_vui(), field, past);
            assert!(parse_sps(&spec.nal()).is_none(), "field {field} = {past}");
            let mut tally = Tally::default();
            compare_sps(&spec.nal(), &format!("field {field} past"), &mut tally);
            assert_eq!(tally.wide_code, 1, "field {field}: {tally:?}");
        }
    }
}

#[test]
fn ranged_fields_are_checked() {
    let cases: Vec<(&str, SpsSpec)> = vec![
        ("sps id", {
            let mut spec = SpsSpec::new(66);
            spec.sps_id = 32;
            spec
        }),
        ("chroma format", {
            let mut spec = SpsSpec::new(100);
            spec.chroma_format_idc = 4;
            spec
        }),
        ("luma depth", {
            let mut spec = SpsSpec::new(100);
            spec.bit_depth_luma_minus8 = 7;
            spec
        }),
        ("chroma depth", {
            let mut spec = SpsSpec::new(100);
            spec.bit_depth_chroma_minus8 = 7;
            spec
        }),
        ("frame num", {
            let mut spec = SpsSpec::new(66);
            spec.log2_max_frame_num_minus4 = 13;
            spec
        }),
        ("poc lsb", {
            let mut spec = SpsSpec::new(66);
            spec.poc = Poc::Type0(13);
            spec
        }),
        ("poc cycle", {
            let mut spec = SpsSpec::new(66);
            spec.poc = Poc::Type1 {
                non_ref: 0,
                top_to_bottom: 0,
                cycle: vec![0; 256],
                declared: 256,
            };
            spec
        }),
        ("poc cycle past the data", {
            let mut spec = SpsSpec::new(66);
            spec.poc = Poc::Type1 {
                non_ref: 0,
                top_to_bottom: 0,
                cycle: Vec::new(),
                declared: 255,
            };
            spec
        }),
        ("chroma location 4:2:2", {
            let mut spec = SpsSpec::new(122).full_vui();
            spec.chroma_format_idc = 2;
            spec
        }),
        ("chroma location top", {
            let mut spec = SpsSpec::new(66);
            spec.vui = Some(Vui {
                chroma_loc: Some((6, 0)),
                ..Default::default()
            });
            spec
        }),
        ("chroma location bottom", {
            let mut spec = SpsSpec::new(66);
            spec.vui = Some(Vui {
                chroma_loc: Some((0, 6)),
                ..Default::default()
            });
            spec
        }),
        ("zero tick", {
            let mut spec = SpsSpec::new(66);
            spec.vui = Some(Vui {
                timing: Some((0, 1)),
                ..Default::default()
            });
            spec
        }),
        ("zero scale", {
            let mut spec = SpsSpec::new(66);
            spec.vui = Some(Vui {
                timing: Some((1, 0)),
                ..Default::default()
            });
            spec
        }),
        ("forbidden bit", {
            let mut spec = SpsSpec::new(66);
            spec.header = 0xE7;
            spec
        }),
        ("not an SPS", {
            let mut spec = SpsSpec::new(66);
            spec.header = 0x68;
            spec
        }),
    ];
    for (label, spec) in cases {
        assert!(parse_sps(&spec.nal()).is_none(), "{label}");
        let mut tally = Tally::default();
        compare_sps(&spec.nal(), label, &mut tally);
        assert_eq!(tally.refused, 1, "{label}");
    }
}

#[test]
fn scaling_deltas_outside_a_byte_are_refused() {
    // The largest has a 63-bit zero prefix and overflowed the dependency.
    for delta in [128, -129, 1 << 31, -(1 << 31), i64::MAX, i64::MIN + 1] {
        for list in [0, 7] {
            let mut spec = SpsSpec::new(100);
            spec.scaling_lists = vec![None; 8];
            spec.scaling_lists[list] = Some(vec![delta]);
            assert!(parse_sps(&spec.nal()).is_none(), "{delta} in list {list}");
        }
    }
}

#[test]
fn picture_size_arithmetic_is_checked() {
    let size = |width: u64, height: u64, frame_mbs_only: bool, crop: Option<[u64; 4]>| {
        let mut spec = SpsSpec::new(77);
        spec.width_mbs_minus1 = width;
        spec.height_map_units_minus1 = height;
        spec.frame_mbs_only = frame_mbs_only;
        spec.crop = crop;
        let mut tally = Tally::default();
        compare_sps(&spec.nal(), "size", &mut tally);
        assert_eq!(tally.same, 1);
        sps_dimensions(&parse_sps(&spec.nal()).expect("parses"))
    };
    let widest = u64::from(i32::MAX as u32 / 16) - 1;
    assert_eq!(size(widest, 0, true, None), Some((i32::MAX - 15, 16)));
    assert_eq!(size(widest + 1, 0, true, None), None);
    // Fields double the height.
    assert_eq!(
        size(0, widest / 2 - 1, false, None),
        Some((16, i32::MAX - 31))
    );
    assert_eq!(size(0, widest / 2, false, None), None);
    // A crop of the whole picture leaves nothing; one more is too much.
    assert_eq!(size(0, 0, true, Some([4, 4, 0, 0])), Some((0, 16)));
    assert_eq!(size(0, 0, true, Some([4, 5, 0, 0])), None);
    assert_eq!(size(0, 0, false, Some([0, 0, 4, 4])), Some((16, 0)));
    assert_eq!(size(0, 0, false, Some([0, 0, 4, 5])), None);
    let most = u64::from(u32::MAX - 1);
    assert_eq!(size(most, most, false, Some([most; 4])), None);
    assert_eq!(size(most, most, false, None), None);
}

#[test]
fn every_truncation_of_a_valid_sps_is_refused_or_complete() {
    for spec in [SpsSpec::new(100).full_vui(), SpsSpec::new(66).full_vui(), {
        let mut spec = SpsSpec::new(244).full_vui();
        spec.poc = Poc::Type1 {
            non_ref: 5,
            top_to_bottom: -5,
            cycle: vec![2, -2],
            declared: 2,
        };
        spec.scaling_lists = (0..8)
            .map(|list| Some(vec![1; if list < 6 { 16 } else { 64 }]))
            .collect();
        spec
    }] {
        let nal = spec.nal();
        let full = parse_sps(&nal).expect("parses");
        let mut complete = false;
        for len in 0..=nal.len() {
            match parse_sps(&nal[..len]) {
                Some(sps) => {
                    assert_eq!(sps, full, "cut at {len}");
                    complete = true;
                }
                None => assert!(!complete, "cut at {len} refused after a shorter cut parsed"),
            }
        }
        assert!(complete);
    }
}

#[test]
fn every_truncation_of_a_valid_avcc_is_refused_or_complete() {
    let high = SpsSpec::new(100).full_vui().nal();
    let mut ext = TestBitWriter::new();
    ext.write_ue(1); // chroma_format_idc
    ext.write_ue(2); // bit_depth_luma_minus8
    ext.write_ue(2); // bit_depth_chroma_minus8
    ext.write_bits(0b01, 2); // no bypass; scaling matrix present
    ext.write_bits(1, 1); // the first list present
    ext.write_se(-8);
    ext.write_bits(0, 7); // the other lists absent
    let ext = ext.finish();
    let records = [
        avcc(&[&high], &[PPS], Some((1, 2, 2, &[&ext]))),
        avcc(&[&high, &high], &[PPS, PPS], Some((3, 0, 0, &[]))),
        avcc(&[&high], &[PPS], None),
        avcc(&[&SpsSpec::new(66).nal()], &[PPS], None),
    ];
    let mut both = 0;
    for (index, record) in records.iter().enumerate() {
        let full = AvcConfig::parse(record).expect("parses");
        assert!(is_valid_h264_avcc(record));
        for len in 0..=record.len() {
            let label = format!("record {index} cut at {len}");
            assert!(compare_avcc(&record[..len], &label, &mut both), "{label}");
            if let Some(cut) = AvcConfig::parse(&record[..len]) {
                // A cut inside the extension drops the extension.
                assert_eq!(cut.sps, full.sps, "{label}");
            }
        }
    }
    assert!(both >= records.len());
    let config = AvcConfig::parse(&records[0]).expect("parses");
    assert_eq!(config.bit_depth_luma_minus8, Some(2));
    assert_eq!(config.pps_count, 1);
}

#[test]
fn absurd_avcc_counts_are_refused() {
    let sps = SpsSpec::new(100).nal();
    let base = avcc(&[&sps], &[PPS], Some((1, 0, 0, &[])));
    let pps_count_at = 6 + 2 + sps.len();

    // 31 SPSs declared, one present.
    let mut record = base.clone();
    record[5] = 0xFF;
    assert!(AvcConfig::parse(&record).is_none());
    // An SPS longer than the record.
    let mut record = base.clone();
    record[6..8].copy_from_slice(&[0xFF, 0xFF]);
    assert!(AvcConfig::parse(&record).is_none());
    // 255 PPSs declared, one present.
    let mut record = base.clone();
    record[pps_count_at] = 0xFF;
    assert!(AvcConfig::parse(&record).is_none());
    // A PPS longer than the record.
    let mut record = base.clone();
    record[pps_count_at + 1..pps_count_at + 3].copy_from_slice(&[0xFF, 0xFF]);
    assert!(AvcConfig::parse(&record).is_none());
    // 255 SPS extensions declared, none present.
    let mut record = base.clone();
    *record.last_mut().expect("count") = 0xFF;
    assert!(AvcConfig::parse(&record).is_none());
    // An extension entry longer than the record.
    let mut record = base.clone();
    *record.last_mut().expect("count") = 1;
    record.extend_from_slice(&[0xFF, 0xFF, 0x80]);
    assert!(AvcConfig::parse(&record).is_none());
    // An extension entry with an overlong prefix or a delta out of range.
    let mut wide = vec![0_u8; 9];
    wide.push(0x80);
    let mut delta = TestBitWriter::new();
    delta.write_ue(1);
    delta.write_ue(0);
    delta.write_ue(0);
    delta.write_bits(0b011, 3);
    delta.write_se(200);
    for entry in [wide, delta.finish()] {
        let record = avcc(&[&sps], &[PPS], Some((1, 0, 0, &[&entry])));
        assert!(AvcConfig::parse(&record).is_none());
    }
    // No SPS, or no PPS: parses, but is not a usable record.
    for record in [
        avcc(&[], &[PPS], None),
        avcc(&[&sps], &[], Some((1, 0, 0, &[]))),
    ] {
        assert!(AvcConfig::parse(&record).is_some());
        assert!(!is_valid_h264_avcc(&record));
    }
    let mut both = 0;
    assert!(compare_avcc(&base, "base", &mut both));
    assert_eq!(both, 1);
}

// ---------------------------------------------------------------------------
// The hydrated corpus.
// ---------------------------------------------------------------------------

fn media_root() -> PathBuf {
    std::env::var_os("MEDIAPROBE_MEDIA_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media"))
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|err| panic!("{}: {err}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The bytes up to the next start code, at most 64 KiB.
fn until_start_code(data: &[u8]) -> &[u8] {
    let data = &data[..data.len().min(64 * 1024)];
    let end = data
        .windows(3)
        .position(|window| window[0] == 0 && window[1] == 0 && window[2] <= 1)
        .unwrap_or(data.len());
    &data[..end]
}

/// Every H.264 SPS NAL unit and avcC record found in `data`: Annex B SPSs,
/// MP4 `avcC` boxes, Matroska `CodecPrivate` elements and FLV AVC sequence
/// headers. Candidates that are not H.264 at all are harmless: both
/// readers see the same bytes.
fn parameter_sets_in(data: &[u8], sps: &mut BTreeSet<Vec<u8>>, avcc: &mut BTreeSet<Vec<u8>>) {
    for at in 0..data.len().saturating_sub(4) {
        let window = &data[at..at + 4];
        if window[..3] == [0, 0, 1] && window[3] & 0x9F == 7 {
            sps.insert(until_start_code(&data[at + 3..]).to_vec());
        }
        if window == b"avcC" && at >= 4 {
            let size = u32::from_be_bytes([data[at - 4], data[at - 3], data[at - 2], data[at - 1]]);
            if let Some(record) = (size as usize)
                .checked_sub(8)
                .and_then(|len| data.get(at + 4..at + 4 + len))
            {
                avcc.insert(record.to_vec());
            }
        }
        if window[..2] == [0x63, 0xA2] {
            // An EBML size of one to four bytes.
            let first = window[2];
            let width = first.leading_zeros() as usize + 1;
            if (1..=4).contains(&width)
                && let Some(size) = data.get(at + 2..at + 2 + width)
            {
                let len = size[1..]
                    .iter()
                    .fold(usize::from(first & (0xFF >> width)), |acc, &byte| {
                        (acc << 8) | usize::from(byte)
                    });
                if let Some(record) = data.get(at + 2 + width..at + 2 + width + len)
                    && record.first() == Some(&1)
                {
                    avcc.insert(record.to_vec());
                }
            }
        }
        // An FLV video tag whose payload is an AVC sequence header.
        if data[at] == 9 && at + 16 < data.len() && data[at + 11..at + 16] == [0x17, 0, 0, 0, 0] {
            let size = usize::from(data[at + 1]) << 16
                | usize::from(data[at + 2]) << 8
                | usize::from(data[at + 3]);
            if let Some(record) = size
                .checked_sub(5)
                .and_then(|len| data.get(at + 16..at + 16 + len))
            {
                avcc.insert(record.to_vec());
            }
        }
    }
    for record in avcc.iter() {
        if let Some(config) = AvcConfig::parse(record) {
            sps.extend(config.sps.iter().map(|nal| nal.to_vec()));
        }
    }
}

#[test]
fn corpus_parameter_sets_read_as_scuffle_reads_them() {
    let mut files = Vec::new();
    files_under(&media_root(), &mut files);
    let mut sps = BTreeSet::new();
    let mut avcc = BTreeSet::new();
    for file in &files {
        let data = std::fs::read(file).unwrap_or_else(|err| panic!("{}: {err}", file.display()));
        parameter_sets_in(&data, &mut sps, &mut avcc);
    }

    let mut tally = Tally::default();
    for (index, nal) in sps.iter().enumerate() {
        compare_sps(nal, &format!("corpus SPS {index}"), &mut tally);
    }
    let accepted = tally.same;
    // No corpus SPS differs in any way.
    assert_eq!(tally.wide_code + tally.zero_run, 0, "{tally:?}");

    // Every cut and every single flipped bit of each accepted SPS.
    let mut mutated = Tally::default();
    for (index, nal) in sps.iter().enumerate() {
        if parse_sps(nal).is_none() {
            continue;
        }
        for len in 0..nal.len() {
            compare_sps(
                &nal[..len],
                &format!("corpus SPS {index} cut"),
                &mut mutated,
            );
        }
        let mut flipped = nal.clone();
        for bit in 0..nal.len() * 8 {
            flipped[bit / 8] ^= 0x80 >> (bit % 8);
            compare_sps(
                &flipped,
                &format!("corpus SPS {index} bit {bit}"),
                &mut mutated,
            );
            flipped[bit / 8] ^= 0x80 >> (bit % 8);
        }
    }

    let mut records = 0;
    let mut both = 0;
    for (index, record) in avcc.iter().enumerate() {
        if compare_avcc(record, &format!("corpus avcC {index}"), &mut both) {
            records += 1;
        }
        for len in 0..record.len() {
            compare_avcc(
                &record[..len],
                &format!("corpus avcC {index} cut"),
                &mut both,
            );
        }
    }
    let usable = avcc
        .iter()
        .filter(|record| is_valid_h264_avcc(record))
        .count();
    eprintln!(
        "corpus: {} files, {} SPS candidates ({accepted} accepted by both), \
         {} avcC candidates ({records} compared, {usable} usable); \
         mutations {mutated:?}; avcC accepted by both incl. cuts: {both}",
        files.len(),
        sps.len(),
        avcc.len()
    );
    assert!(accepted >= 20, "{tally:?}");
    assert!(usable >= 20, "{usable}");
}

#[test]
fn crop_units_follow_chroma_format_and_field_coding() {
    // A 1920x1088 coded picture cropped to 1080 lines.
    let size = |profile: u64, chroma: u64, frame_mbs_only: bool, bottom: u64| {
        let mut spec = SpsSpec::new(profile);
        spec.chroma_format_idc = chroma;
        spec.width_mbs_minus1 = 119;
        spec.height_map_units_minus1 = if frame_mbs_only { 67 } else { 33 };
        spec.frame_mbs_only = frame_mbs_only;
        spec.crop = Some([0, 0, 0, bottom]);
        sps_dimensions(&parse_sps(&spec.nal()).expect("parses"))
    };
    // 4:2:0: two luma rows per unit in a frame, four when coding fields.
    assert_eq!(size(77, 1, true, 4), Some((1920, 1080)));
    assert_eq!(size(77, 1, false, 2), Some((1920, 1080)));
    // 4:2:2, 4:4:4 and monochrome: one luma row per unit, two for fields.
    for chroma in [2, 3, 0] {
        assert_eq!(size(122, chroma, true, 8), Some((1920, 1080)), "{chroma}");
        assert_eq!(size(122, chroma, false, 4), Some((1920, 1080)), "{chroma}");
    }

    // Horizontally, 4:4:4 and monochrome count single luma columns.
    let width = |chroma: u64| {
        let mut spec = SpsSpec::new(244);
        spec.chroma_format_idc = chroma;
        spec.width_mbs_minus1 = 119;
        spec.height_map_units_minus1 = 67;
        spec.crop = Some([2, 2, 0, 0]);
        sps_dimensions(&parse_sps(&spec.nal()).expect("parses")).map(|size| size.0)
    };
    assert_eq!(width(1), Some(1912));
    assert_eq!(width(2), Some(1912));
    assert_eq!(width(3), Some(1916));
    assert_eq!(width(0), Some(1916));
}
