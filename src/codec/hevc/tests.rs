use super::sps::{Timing, Vui, Window};
use super::*;
use crate::codec::bits::{
    BitReader, TestBitWriter, escape_rbsp, has_terminated_zero_run, read_se, read_ue,
};
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Comparison with scuffle-h265
// ---------------------------------------------------------------------------

/// What `scuffle-h265` reads from an SPS, in this crate's terms, or why it
/// refuses the SPS.
///
/// Only call this on an SPS that `scuffle_safe` passes.
fn scuffle_read(nal: &[u8]) -> Result<Sps, String> {
    let sps = scuffle_h265::SpsNALUnit::parse(std::io::Cursor::new(nal))
        .map_err(|error| error.to_string())?;
    let rbsp = &sps.rbsp;
    let profile = &rbsp.profile_tier_level.general_profile;
    let window = &rbsp.conformance_window;
    let narrow = |value: u64| {
        u32::try_from(value).map_err(|_| format!("a value wider than 32 bits: {value}"))
    };
    let vui = match &rbsp.vui_parameters {
        None => None,
        Some(vui) => {
            let (aspect_ratio_idc, sar_width, sar_height) = match vui.aspect_ratio_info {
                scuffle_h265::AspectRatioInfo::Predefined(id) => (id.0, 0, 0),
                scuffle_h265::AspectRatioInfo::ExtendedSar {
                    sar_width,
                    sar_height,
                } => (255, sar_width, sar_height),
            };
            let signal = &vui.video_signal_type;
            Some(Vui {
                aspect_ratio_idc,
                sar_width,
                sar_height,
                video_full_range: signal.video_full_range_flag,
                colour_primaries: signal.colour_primaries,
                transfer_characteristics: signal.transfer_characteristics,
                matrix_coeffs: signal.matrix_coeffs,
                chroma_sample_loc_type_top_field: match &vui.chroma_loc_info {
                    None => None,
                    Some(loc) => Some(narrow(loc.top_field)?),
                },
                field_seq: vui.field_seq_flag,
                timing: vui.vui_timing_info.as_ref().map(|timing| Timing {
                    num_units_in_tick: timing.num_units_in_tick.get(),
                    time_scale: timing.time_scale.get(),
                    num_ticks_poc_diff_one_minus1: timing.num_ticks_poc_diff_one_minus1,
                }),
            })
        }
    };
    Ok(Sps {
        profile_idc: profile.profile_idc,
        progressive_source: profile.progressive_source_flag,
        interlaced_source: profile.interlaced_source_flag,
        level_idc: profile.level_idc.expect("the general level is always read"),
        chroma_format_idc: rbsp.chroma_format_idc,
        bit_depth_luma_minus8: rbsp.bit_depth_luma_minus8,
        width: narrow(rbsp.pic_width_in_luma_samples.get())?,
        height: narrow(rbsp.pic_height_in_luma_samples.get())?,
        conformance_window: Window {
            left: narrow(window.conf_win_left_offset)?,
            right: narrow(window.conf_win_right_offset)?,
            top: narrow(window.conf_win_top_offset)?,
            bottom: narrow(window.conf_win_bottom_offset)?,
        },
        vui,
    })
}

/// [`scuffle_read`], with a refusal as `None`.
fn scuffle_sps(nal: &[u8]) -> Option<Sps> {
    scuffle_read(nal).ok()
}

/// Whether `scuffle-h265` can be given this SPS without reaching one of its
/// known panics: an Exp-Golomb prefix of 64 zero bits, a transform block
/// larger than the coding tree block, a scaling list predicted from before
/// the first matrix or with a coefficient that overflows, and arithmetic
/// on values wider than 32 bits in the PCM block sizes and the default
/// display window. Refuses any Exp-Golomb prefix of 33 or more zero bits
/// after the profile and level, which rules out the last two.
///
/// It follows `scuffle-h265`'s own field layout, including the sub-layer
/// level it reads twice. It does not reach the screen content coding
/// extension, whose palette initializers `scuffle-h265` allocates from a
/// declared count.
fn scuffle_safe(nal: &[u8]) -> bool {
    let Some(payload) = nal.get(2..) else {
        return false;
    };
    let rbsp = crate::scan::h2645_unescape_rbsp(payload);
    scuffle_guard(&rbsp).is_some()
}

fn scuffle_guard(rbsp: &[u8]) -> Option<()> {
    const PROFILE_BITS: usize = 88;
    let mut bits = BitReader::new(rbsp);
    bits.skip_bits(4)?;
    let max_sub_layers_minus1 = bits.read_bits(3)? as usize;
    bits.skip_bits(1)?;
    bits.skip_bits(PROFILE_BITS + 8)?;
    let mut sub_layer_bits = 0;
    for _ in 0..max_sub_layers_minus1 {
        let profile_present = bits.read_bit()? == 1;
        let level_present = bits.read_bit()? == 1;
        if profile_present {
            sub_layer_bits += PROFILE_BITS;
            if level_present {
                sub_layer_bits += 8; // read twice
            }
        }
        if level_present {
            sub_layer_bits += 8;
        }
    }
    if max_sub_layers_minus1 > 0 {
        bits.skip_bits(2 * 8_usize.checked_sub(max_sub_layers_minus1)?)?;
    }
    bits.skip_bits(sub_layer_bits)?;
    if has_terminated_zero_run(rbsp, bits.position(), 33) {
        return None;
    }

    read_ue(&mut bits)?;
    if read_ue(&mut bits)? == 3 {
        bits.skip_bits(1)?;
    }
    read_ue(&mut bits)?;
    read_ue(&mut bits)?;
    if bits.read_bit()? == 1 {
        for _ in 0..4 {
            read_ue(&mut bits)?;
        }
    }
    for _ in 0..3 {
        read_ue(&mut bits)?;
    }
    let sets = if bits.read_bit()? == 1 {
        max_sub_layers_minus1 + 1
    } else {
        1
    };
    for _ in 0..sets * 3 {
        read_ue(&mut bits)?;
    }
    let min_cb_log2 = read_ue(&mut bits)? + 3;
    let ctb_log2 = min_cb_log2 + read_ue(&mut bits)?;
    if read_ue(&mut bits)? + 2 > ctb_log2 {
        return None;
    }
    for _ in 0..3 {
        read_ue(&mut bits)?;
    }
    if bits.read_bit()? == 1 && bits.read_bit()? == 1 {
        // scuffle-h265 steps through the 32x32 matrices by three.
        for size_id in 0..4 {
            let step = if size_id == 3 { 3 } else { 1 };
            let mut matrix_id = 0;
            while matrix_id < 6 {
                if bits.read_bit()? == 0 {
                    if read_ue(&mut bits)? * step > matrix_id {
                        return None;
                    }
                } else {
                    if size_id > 1 {
                        read_se(&mut bits)?;
                    }
                    for _ in 0..(16 << (2 * size_id)).min(64) {
                        read_se(&mut bits)?;
                    }
                }
                matrix_id += step;
            }
        }
    }
    Some(())
}

/// Parses `nal` with both readers, asserts they agree, and returns the
/// result. Both must accept it.
fn agrees(nal: &[u8]) -> Sps {
    let ours = parse_sps(nal).expect("this crate's reader accepts the SPS");
    assert!(scuffle_safe(nal));
    let theirs = scuffle_read(nal).expect("scuffle-h265 accepts the SPS");
    assert_eq!(ours, theirs);
    ours
}

/// What `scuffle-h265` reads from an `hvcC` record, in this crate's terms:
/// the profile, the luma bit depth and the SPS NAL units.
fn scuffle_hvcc(data: &[u8]) -> Option<(u8, u8, Vec<Vec<u8>>)> {
    let config =
        scuffle_h265::HEVCDecoderConfigurationRecord::demux(&mut std::io::Cursor::new(data))
            .ok()?;
    let sps = config
        .arrays
        .iter()
        .filter(|array| array.nal_unit_type == scuffle_h265::NALUnitType::SpsNut)
        .flat_map(|array| array.nalus.iter().map(|nal| nal.to_vec()))
        .collect();
    Some((
        config.general_profile_idc,
        config.bit_depth_luma_minus8,
        sps,
    ))
}

fn our_hvcc(data: &[u8]) -> Option<(u8, u8, Vec<Vec<u8>>)> {
    let config = hvcc::parse(data)?;
    Some((
        config.general_profile_idc,
        config.bit_depth_luma_minus8,
        config.sps_nal_units().map(<[u8]>::to_vec).collect(),
    ))
}

// ---------------------------------------------------------------------------
// SPS builder
// ---------------------------------------------------------------------------

type Tools = Rc<dyn Fn(&mut TestBitWriter)>;

/// A short-term reference picture set as written in the SPS.
#[derive(Clone)]
enum RefSet {
    /// `delta_poc_s0_minus1` and `delta_poc_s1_minus1` values, each picture
    /// used by the current picture.
    Explicit {
        negative: Vec<u64>,
        positive: Vec<u64>,
    },
    /// Predicted from the previous set: the sign and magnitude of
    /// `deltaRps`, and `used_by_curr_pic_flag` and `use_delta_flag` for
    /// each picture of the reference set and the set itself.
    Predicted {
        negative_sign: bool,
        abs_minus1: u64,
        flags: Vec<(bool, bool)>,
    },
}

#[derive(Clone)]
struct VuiBuild {
    aspect: Option<(u8, u16, u16)>,
    overscan: Option<bool>,
    /// `video_full_range_flag` and the colour description.
    signal: Option<(bool, Option<[u8; 3]>)>,
    chroma_loc: Option<(u64, u64)>,
    field_seq: bool,
    frame_field_info: bool,
    display_window: Option<[u64; 4]>,
    /// `vui_num_units_in_tick`, `vui_time_scale` and
    /// `vui_num_ticks_poc_diff_one_minus1`.
    timing: Option<(u32, u32, Option<u64>)>,
    hrd: bool,
    restriction: bool,
}

impl Default for VuiBuild {
    fn default() -> Self {
        Self {
            aspect: Some((1, 0, 0)),
            overscan: None,
            signal: Some((false, Some([9, 16, 9]))),
            chroma_loc: Some((2, 2)),
            field_seq: false,
            frame_field_info: false,
            display_window: None,
            timing: Some((1001, 48_000, Some(0))),
            hrd: false,
            restriction: true,
        }
    }
}

#[derive(Clone)]
struct Build {
    layer_id: u8,
    nesting: bool,
    /// `sub_layer_profile_present_flag` and `sub_layer_level_present_flag`
    /// for each sub-layer below the highest; their count is
    /// `sps_max_sub_layers_minus1`.
    sub_layers: Vec<(bool, bool)>,
    profile_idc: u8,
    compatibility: u32,
    progressive: bool,
    interlaced: bool,
    frame_only: bool,
    level_idc: u8,
    chroma_format_idc: u64,
    separate_planes: bool,
    width: u64,
    height: u64,
    window: Option<[u64; 4]>,
    depth_luma_minus8: u64,
    depth_chroma_minus8: u64,
    poc_lsb_minus4: u64,
    /// Whether every sub-layer has its own ordering information, and the
    /// entries written.
    ordering_all: bool,
    ordering: Vec<[u64; 3]>,
    /// The coding and transform block sizes and depths.
    blocks: [u64; 6],
    /// Writes `scaling_list_enabled_flag` through the PCM parameters.
    tools: Tools,
    /// The set count written, when it is not the number of sets.
    ref_set_count: Option<u64>,
    ref_sets: Vec<RefSet>,
    /// The count written and `lt_ref_pic_poc_lsb_sps` with its flag.
    long_term: Option<(u64, Vec<u64>)>,
    vui: Option<VuiBuild>,
    extension: bool,
}

impl Default for Build {
    fn default() -> Self {
        Self {
            layer_id: 0,
            nesting: true,
            sub_layers: Vec::new(),
            profile_idc: 2,
            compatibility: 0x2000_0000,
            progressive: true,
            interlaced: false,
            frame_only: true,
            level_idc: 153,
            chroma_format_idc: 1,
            separate_planes: false,
            width: 3840,
            height: 2160,
            window: None,
            depth_luma_minus8: 2,
            depth_chroma_minus8: 2,
            poc_lsb_minus4: 4,
            ordering_all: false,
            ordering: vec![[4, 2, 0]],
            blocks: [0, 3, 0, 3, 2, 2],
            tools: Rc::new(|out| out.write_bits(0b0110, 4)),
            ref_set_count: None,
            ref_sets: vec![RefSet::Explicit {
                negative: vec![0, 1],
                positive: vec![0],
            }],
            long_term: None,
            vui: Some(VuiBuild::default()),
            extension: false,
        }
    }
}

impl Build {
    fn rbsp(&self) -> Vec<u8> {
        let mut out = TestBitWriter::new();
        out.write_bits(0, 4); // sps_video_parameter_set_id
        out.write_bits(self.sub_layers.len() as u64, 3);
        out.write_bits(u64::from(self.nesting), 1);

        out.write_bits(0, 3); // profile space and tier
        out.write_bits(u64::from(self.profile_idc), 5);
        out.write_bits(u64::from(self.compatibility), 32);
        out.write_bits(u64::from(self.progressive), 1);
        out.write_bits(u64::from(self.interlaced), 1);
        out.write_bits(0, 1);
        out.write_bits(u64::from(self.frame_only), 1);
        out.write_bits(0, 44);
        out.write_bits(u64::from(self.level_idc), 8);
        for (profile, level) in &self.sub_layers {
            out.write_bits(u64::from(*profile), 1);
            out.write_bits(u64::from(*level), 1);
        }
        if !self.sub_layers.is_empty() {
            out.write_bits(0, 2 * (8 - self.sub_layers.len()));
        }
        for (profile, level) in &self.sub_layers {
            if *profile {
                // Main profile, all flags set where a flag is allowed.
                out.write_bits(0b000_00001, 8);
                out.write_bits(0x6000_0000, 32);
                out.write_bits(0b1001, 4);
                out.write_bits(0, 44);
            }
            if *level {
                out.write_bits(120, 8);
            }
        }

        out.write_ue(0); // sps_seq_parameter_set_id
        out.write_ue(self.chroma_format_idc);
        if self.chroma_format_idc == 3 {
            out.write_bits(u64::from(self.separate_planes), 1);
        }
        out.write_ue(self.width);
        out.write_ue(self.height);
        match self.window {
            None => out.write_bits(0, 1),
            Some(offsets) => {
                out.write_bits(1, 1);
                for offset in offsets {
                    out.write_ue(offset);
                }
            }
        }
        out.write_ue(self.depth_luma_minus8);
        out.write_ue(self.depth_chroma_minus8);
        out.write_ue(self.poc_lsb_minus4);
        out.write_bits(u64::from(self.ordering_all), 1);
        for entry in &self.ordering {
            for value in entry {
                out.write_ue(*value);
            }
        }
        for value in self.blocks {
            out.write_ue(value);
        }
        (self.tools)(&mut out);

        out.write_ue(self.ref_set_count.unwrap_or(self.ref_sets.len() as u64));
        for (index, set) in self.ref_sets.iter().enumerate() {
            match set {
                RefSet::Explicit { negative, positive } => {
                    if index > 0 {
                        out.write_bits(0, 1);
                    }
                    out.write_ue(negative.len() as u64);
                    out.write_ue(positive.len() as u64);
                    for delta in negative.iter().chain(positive) {
                        out.write_ue(*delta);
                        out.write_bits(1, 1);
                    }
                }
                RefSet::Predicted {
                    negative_sign,
                    abs_minus1,
                    flags,
                } => {
                    out.write_bits(1, 1);
                    out.write_bits(u64::from(*negative_sign), 1);
                    out.write_ue(*abs_minus1);
                    for (used, use_delta) in flags {
                        out.write_bits(u64::from(*used), 1);
                        if !used {
                            out.write_bits(u64::from(*use_delta), 1);
                        }
                    }
                }
            }
        }
        match &self.long_term {
            None => out.write_bits(0, 1),
            Some((count, pictures)) => {
                out.write_bits(1, 1);
                out.write_ue(*count);
                for lsb in pictures {
                    out.write_bits(*lsb, self.poc_lsb_minus4 as usize + 4);
                    out.write_bits(1, 1);
                }
            }
        }
        out.write_bits(0b11, 2); // temporal MVP, strong intra smoothing

        match &self.vui {
            None => out.write_bits(0, 1),
            Some(vui) => {
                out.write_bits(1, 1);
                self.write_vui(vui, &mut out);
            }
        }
        if self.extension {
            // Only the range extension, all flags set.
            out.write_bits(1, 1);
            out.write_bits(0b1000_0000, 8);
            out.write_bits(0x1FF, 9);
        } else {
            out.write_bits(0, 1);
        }
        out.write_bits(1, 1); // rbsp_stop_one_bit
        out.finish()
    }

    fn write_vui(&self, vui: &VuiBuild, out: &mut TestBitWriter) {
        match vui.aspect {
            None => out.write_bits(0, 1),
            Some((idc, width, height)) => {
                out.write_bits(1, 1);
                out.write_bits(u64::from(idc), 8);
                if idc == 255 {
                    out.write_bits(u64::from(width), 16);
                    out.write_bits(u64::from(height), 16);
                }
            }
        }
        match vui.overscan {
            None => out.write_bits(0, 1),
            Some(appropriate) => out.write_bits(0b10 | u64::from(appropriate), 2),
        }
        match vui.signal {
            None => out.write_bits(0, 1),
            Some((full_range, colour)) => {
                out.write_bits(1, 1);
                out.write_bits(5, 3); // video_format
                out.write_bits(u64::from(full_range), 1);
                match colour {
                    None => out.write_bits(0, 1),
                    Some(values) => {
                        out.write_bits(1, 1);
                        for value in values {
                            out.write_bits(u64::from(value), 8);
                        }
                    }
                }
            }
        }
        match vui.chroma_loc {
            None => out.write_bits(0, 1),
            Some((top, bottom)) => {
                out.write_bits(1, 1);
                out.write_ue(top);
                out.write_ue(bottom);
            }
        }
        out.write_bits(0, 1); // neutral_chroma_indication_flag
        out.write_bits(u64::from(vui.field_seq), 1);
        out.write_bits(u64::from(vui.frame_field_info), 1);
        match vui.display_window {
            None => out.write_bits(0, 1),
            Some(offsets) => {
                out.write_bits(1, 1);
                for offset in offsets {
                    out.write_ue(offset);
                }
            }
        }
        match vui.timing {
            None => out.write_bits(0, 1),
            Some((units, scale, ticks)) => {
                out.write_bits(1, 1);
                out.write_bits(u64::from(units), 32);
                out.write_bits(u64::from(scale), 32);
                match ticks {
                    None => out.write_bits(0, 1),
                    Some(ticks) => {
                        out.write_bits(1, 1);
                        out.write_ue(ticks);
                    }
                }
                out.write_bits(u64::from(vui.hrd), 1);
                if vui.hrd {
                    // No NAL or VCL HRD; a fixed picture rate per sub-layer.
                    out.write_bits(0b00, 2);
                    for _ in 0..=self.sub_layers.len() {
                        out.write_bits(1, 1); // fixed_pic_rate_general_flag
                        out.write_ue(0); // elemental_duration_in_tc_minus1
                        out.write_ue(0); // cpb_cnt_minus1
                    }
                }
            }
        }
        if vui.restriction {
            out.write_bits(1, 1);
            out.write_bits(0b011, 3);
            for value in [0, 2, 1, 15, 15] {
                out.write_ue(value);
            }
        } else {
            out.write_bits(0, 1);
        }
    }

    fn nal(&self) -> Vec<u8> {
        let mut nal = vec![
            (33 << 1) | (self.layer_id >> 5),
            ((self.layer_id & 0x1F) << 3) | 1,
        ];
        nal.extend(escape_rbsp(&self.rbsp()));
        nal
    }
}

/// An `hvcC` record with these arrays of NAL units.
fn hvcc_record(profile_idc: u8, luma_minus8: u8, arrays: &[(u8, Vec<Vec<u8>>)]) -> Vec<u8> {
    let mut out = vec![
        1,
        profile_idc,
        0x60,
        0,
        0,
        0,
        0x90,
        0,
        0,
        0,
        0,
        0,
        153,
        0xF0,
        0,
        0xFC,
        0xFD,
        0xF8 | luma_minus8,
        0xFA,
        0,
        0,
        0x0F,
        arrays.len() as u8,
    ];
    for (nal_type, nals) in arrays {
        out.push(0x80 | nal_type);
        out.extend((nals.len() as u16).to_be_bytes());
        for nal in nals {
            out.extend((nal.len() as u16).to_be_bytes());
            out.extend(nal);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Splicing an encoder's SPS
// ---------------------------------------------------------------------------

/// A Main profile SPS: 16x16 coding blocks and coding tree blocks, and a
/// 64x64 minimum transform block.
const SPS: [u8; 40] = [
    0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03,
    0x00, 0x1E, 0xA0, 0x20, 0x81, 0x05, 0x96, 0x56, 0xD4, 0xA4, 0xC2, 0xF0, 0x16, 0x80, 0x80, 0x00,
    0x00, 0x03, 0x00, 0x80, 0x00, 0x00, 0x0C, 0x04,
];

fn sps_rbsp() -> Vec<u8> {
    crate::scan::h2645_unescape_rbsp(&SPS[2..]).into_owned()
}

/// Bit offsets of fields in the RBSP of `SPS`.
const CONFORMANCE_WINDOW_FLAG_BIT: usize = 134;
const MIN_TRANSFORM_BIT: usize = 158;
const SCALING_LIST_ENABLED_BIT: usize = 170;
const PCM_ENABLED_BIT: usize = 173;

fn copy_bits(data: &[u8], from: usize, to: usize, out: &mut TestBitWriter) {
    let mut bits = BitReader::new(data);
    bits.skip_bits(from).unwrap();
    for _ in from..to {
        out.write_bits(u64::from(bits.read_bit().unwrap()), 1);
    }
}

/// `SPS` with a 4x4 minimum transform block, so that its block sizes
/// are valid, the conformance window `window`, and the fields from
/// `scaling_list_enabled_flag` through `pcm_enabled_flag` and the PCM
/// parameters written by `tools`.
fn sps_with(window: Option<[u64; 4]>, tools: impl FnOnce(&mut TestBitWriter)) -> Vec<u8> {
    let rbsp = sps_rbsp();
    let mut fields = BitReader::new(&rbsp);
    fields.skip_bits(CONFORMANCE_WINDOW_FLAG_BIT).unwrap();
    assert_eq!(fields.read_bit(), Some(0));
    fields
        .skip_bits(MIN_TRANSFORM_BIT - CONFORMANCE_WINDOW_FLAG_BIT - 1)
        .unwrap();
    assert_eq!(read_ue(&mut fields), Some(4));
    let min_transform_end = fields.position();
    for _ in 0..3 {
        read_ue(&mut fields).unwrap();
    }
    assert_eq!(fields.position(), SCALING_LIST_ENABLED_BIT);
    assert_eq!(fields.read_bits(4), Some(0));
    let stop_bit = rbsp.len() * 8 - 1 - rbsp.last().unwrap().trailing_zeros() as usize;

    let mut out = TestBitWriter::new();
    copy_bits(&rbsp, 0, CONFORMANCE_WINDOW_FLAG_BIT, &mut out);
    match window {
        None => out.write_bits(0, 1),
        Some(offsets) => {
            out.write_bits(1, 1);
            for offset in offsets {
                out.write_ue(offset);
            }
        }
    }
    copy_bits(
        &rbsp,
        CONFORMANCE_WINDOW_FLAG_BIT + 1,
        MIN_TRANSFORM_BIT,
        &mut out,
    );
    out.write_ue(0); // log2_min_luma_transform_block_size_minus2
    copy_bits(&rbsp, min_transform_end, SCALING_LIST_ENABLED_BIT, &mut out);
    tools(&mut out);
    copy_bits(&rbsp, PCM_ENABLED_BIT + 1, stop_bit, &mut out);
    out.write_bits(1, 1); // rbsp_stop_one_bit
    let mut nal = SPS[..2].to_vec();
    nal.extend(escape_rbsp(&out.finish()));
    nal
}

/// No scaling lists, AMP, SAO or PCM.
fn no_tools(out: &mut TestBitWriter) {
    out.write_bits(0, 4);
}

/// Scaling list data, each matrix written by `matrix(size_id, matrix_id,
/// out)`, and no AMP, SAO or PCM.
fn scaling_list_tools(
    matrix: impl Fn(usize, u64, &mut TestBitWriter),
) -> impl Fn(&mut TestBitWriter) {
    move |out| {
        out.write_bits(0b11, 2); // enabled and present in the SPS
        for size_id in 0..4 {
            let step = if size_id == 3 { 3 } else { 1 };
            for matrix_id in (0..6).step_by(step) {
                matrix(size_id, matrix_id, out);
            }
        }
        out.write_bits(0, 3);
    }
}

fn scaling_lists(matrix: impl Fn(usize, u64, &mut TestBitWriter)) -> Vec<u8> {
    sps_with(None, scaling_list_tools(matrix))
}

/// A matrix predicted from the one `delta` matrices back, or from the
/// default when `delta` is zero.
fn predicted(out: &mut TestBitWriter, delta: u64) {
    out.write_bits(0, 1);
    out.write_ue(delta);
}

/// An explicit matrix with this DC coefficient, for the sizes that have
/// one, and every delta coefficient `delta`.
fn explicit(out: &mut TestBitWriter, size_id: usize, dc_minus8: i64, delta: i64) {
    out.write_bits(1, 1);
    if size_id > 1 {
        out.write_se(dc_minus8);
    }
    for _ in 0..(16 << (2 * size_id)).min(64) {
        out.write_se(delta);
    }
}

// ---------------------------------------------------------------------------
// Encoder-shaped SPS
// ---------------------------------------------------------------------------

#[test]
fn an_sps_whose_transform_block_exceeds_its_coding_tree_block_is_refused() {
    assert!(parse_sps(&SPS).is_none());
    assert_eq!(extract_h265_info(&SPS).bit_depth, None);
}

#[test]
fn an_hevc_sps_with_or_without_scaling_lists_parses_as_before() {
    let plain = agrees(&sps_with(None, no_tools));
    assert_eq!((plain.width, plain.height), (64, 64));

    // Enabled, but with the default lists rather than lists in the SPS.
    agrees(&sps_with(None, |out| {
        out.write_bits(0b10, 2);
        out.write_bits(0, 3);
    }));

    // The first matrix of each size explicit with boundary coefficients,
    // odd ones predicted from it, even ones from the default.
    agrees(&scaling_lists(|size_id, matrix_id, out| {
        let step = if size_id == 3 { 3 } else { 1 };
        match matrix_id {
            0 => {
                let dc = if size_id == 2 { -7 } else { 247 };
                let delta = if size_id % 2 == 0 { -128 } else { 127 };
                explicit(out, size_id, dc, delta);
            }
            id if id % 2 == 1 => predicted(out, id / step),
            _ => predicted(out, 0),
        }
    }));
}

#[test]
fn an_hevc_scaling_list_predicted_from_before_the_first_matrix_is_refused() {
    // One matrix too far back at each size.
    for (size_id, target) in [(0, 2), (1, 0), (2, 5), (3, 3)] {
        let step = if size_id == 3 { 3 } else { 1 };
        let nal = scaling_lists(|size, matrix_id, out| {
            let delta = if (size, matrix_id) == (size_id, target) {
                target / step + 1
            } else {
                0
            };
            predicted(out, delta);
        });
        assert!(parse_sps(&nal).is_none(), "{size_id}");
    }
}

#[test]
fn an_hevc_scaling_list_coefficient_out_of_range_is_refused() {
    for (size_id, dc, delta) in [(2, -8, 0), (3, 248, 0), (2, 8, -129), (0, 8, 128)] {
        let nal = scaling_lists(|size, matrix_id, out| {
            if (size, matrix_id) == (size_id, 0) {
                explicit(out, size, dc, delta);
            } else {
                predicted(out, 0);
            }
        });
        assert!(parse_sps(&nal).is_none(), "{size_id}");
    }
}

#[test]
fn hevc_pcm_block_sizes_are_bounded() {
    let pcm = |min_minus3: u64, diff: u64| {
        sps_with(None, move |out| {
            out.write_bits(0b0001, 4); // scaling lists, AMP and SAO off; PCM on
            out.write_bits(0x77, 8); // 8-bit PCM samples
            out.write_ue(min_minus3);
            out.write_ue(diff);
            out.write_bits(0, 1); // pcm_loop_filter_disabled_flag
        })
    };
    // The coding blocks are all 16x16, so PCM blocks must be too.
    agrees(&pcm(1, 0));
    for (min, diff) in [
        (0, 0),
        (1, 1),
        (1 << 40, 0),
        (1, 1 << 40),
        (1, u64::from(u32::MAX)),
    ] {
        assert!(parse_sps(&pcm(min, diff)).is_none(), "{min} {diff}");
    }
}

#[test]
fn an_hevc_conformance_window_wider_than_the_picture_has_no_dimensions() {
    let cropped = agrees(&sps_with(Some([2, 2, 4, 0]), no_tools));
    assert_eq!(sps_dimensions(&cropped), Some((56, 56)));
    let too_wide = agrees(&sps_with(Some([40, 0, 0, 0]), no_tools));
    assert_eq!(sps_dimensions(&too_wide), None);
    let too_tall = agrees(&sps_with(Some([0, 0, 0, 40]), no_tools));
    assert_eq!(sps_dimensions(&too_tall), None);
}

// ---------------------------------------------------------------------------
// Built SPS: agreement with scuffle-h265
// ---------------------------------------------------------------------------

#[test]
fn a_built_sps_reads_every_field_the_crate_uses() {
    let sps = agrees(&Build::default().nal());
    assert_eq!(
        sps,
        Sps {
            profile_idc: 2,
            progressive_source: true,
            interlaced_source: false,
            level_idc: 153,
            chroma_format_idc: 1,
            bit_depth_luma_minus8: 2,
            width: 3840,
            height: 2160,
            conformance_window: Window::default(),
            vui: Some(Vui {
                aspect_ratio_idc: 1,
                sar_width: 0,
                sar_height: 0,
                video_full_range: false,
                colour_primaries: 9,
                transfer_characteristics: 16,
                matrix_coeffs: 9,
                chroma_sample_loc_type_top_field: Some(2),
                field_seq: false,
                timing: Some(Timing {
                    num_units_in_tick: 1001,
                    time_scale: 48_000,
                    num_ticks_poc_diff_one_minus1: Some(0),
                }),
            }),
        }
    );
    let info = extract_h265_info(&Build::default().nal());
    assert_eq!(info.profile.as_deref(), Some("Main 10"));
    assert_eq!(info.bit_depth, Some(10));
    assert_eq!(info.color_transfer, Some(16));
}

#[test]
fn built_sps_variants_agree_with_scuffle() {
    let base = Build::default();
    let mut variants = vec![base.clone()];
    let mut vary = |change: &dyn Fn(&mut Build)| {
        let mut build = base.clone();
        change(&mut build);
        variants.push(build);
    };
    vary(&|b| b.vui = None);
    vary(&|b| {
        b.vui = Some(VuiBuild {
            aspect: None,
            overscan: Some(true),
            signal: None,
            chroma_loc: None,
            timing: None,
            restriction: false,
            ..VuiBuild::default()
        })
    });
    vary(&|b| {
        let vui = b.vui.as_mut().unwrap();
        vui.aspect = Some((255, 64, 45));
        vui.signal = Some((true, None));
        vui.timing = Some((1, 25, None));
        vui.hrd = true;
    });
    vary(&|b| {
        let vui = b.vui.as_mut().unwrap();
        vui.field_seq = true;
        vui.frame_field_info = true;
        b.frame_only = false;
    });
    vary(&|b| {
        b.progressive = false;
        b.interlaced = true;
        b.frame_only = false;
    });
    vary(&|b| b.window = Some([0, 0, 0, 4]));
    vary(&|b| b.vui.as_mut().unwrap().display_window = Some([8, 8, 4, 4]));
    vary(&|b| {
        b.chroma_format_idc = 0;
        b.vui.as_mut().unwrap().chroma_loc = None;
    });
    vary(&|b| {
        b.chroma_format_idc = 2;
        b.vui.as_mut().unwrap().chroma_loc = None;
    });
    vary(&|b| {
        b.chroma_format_idc = 3;
        b.vui.as_mut().unwrap().chroma_loc = None;
        b.vui.as_mut().unwrap().signal = Some((true, Some([1, 13, 0])));
    });
    vary(&|b| {
        b.depth_chroma_minus8 = 3;
        b.chroma_format_idc = 3;
        b.vui.as_mut().unwrap().chroma_loc = None;
        b.vui.as_mut().unwrap().signal = Some((false, Some([1, 1, 8])));
    });
    vary(&|b| {
        b.sub_layers = vec![(false, false), (false, true)];
        b.nesting = false;
        b.ordering_all = true;
        b.ordering = vec![[2, 0, 0], [3, 1, 0], [4, 2, 5]];
    });
    vary(&|b| b.layer_id = 1);
    vary(&|b| b.long_term = Some((3, vec![0, 255, 17])));
    vary(&|b| b.extension = true);
    vary(&|b| b.ref_sets.clear());
    vary(&|b| {
        b.tools = Rc::new(scaling_list_tools(|_, _, out| predicted(out, 0)));
    });
    vary(&|b| {
        b.tools = Rc::new(|out| {
            out.write_bits(0b0011, 4);
            out.write_bits(0x99, 8);
            out.write_ue(0);
            out.write_ue(2);
            out.write_bits(1, 1);
        })
    });
    for (index, build) in variants.iter().enumerate() {
        let nal = build.nal();
        let ours = parse_sps(&nal).unwrap_or_else(|| panic!("variant {index} refused"));
        assert_eq!(Some(ours), scuffle_sps(&nal), "variant {index}");
    }
}

#[test]
fn sixty_four_short_term_sets_with_inter_prediction_agree_with_scuffle() {
    let mut sets = vec![RefSet::Explicit {
        negative: vec![0, 1, 3],
        positive: vec![0, 2],
    }];
    // A predicted set holds one flag pair per picture of the set before it,
    // plus one; drop every third picture so the sets stay small.
    let mut previous = Some(5_usize);
    let mut build = Build {
        ordering: vec![[16, 4, 0]],
        ..Build::default()
    };
    for index in 1..64_usize {
        let count = previous.expect("the reference set's size is known") + 1;
        let flags: Vec<(bool, bool)> = (0..count)
            .map(|j| {
                if (index + j) % 3 == 0 {
                    (false, false)
                } else {
                    (j % 2 == 0, true)
                }
            })
            .collect();
        sets.push(RefSet::Predicted {
            negative_sign: index % 2 == 0,
            abs_minus1: (index % 5) as u64,
            flags,
        });
        build.ref_sets = sets.clone();
        previous = Some(last_set_size(&build));
    }
    assert_eq!(build.ref_sets.len(), 64);
    // The VUI after the sets is read in place.
    let sps = agrees(&build.nal());
    assert_eq!(sps.vui, parse_sps(&Build::default().nal()).unwrap().vui);

    // A 65th set is more than an SPS may hold.
    build.ref_set_count = Some(65);
    assert!(parse_sps(&build.nal()).is_none());
}

/// The picture count `scuffle-h265` derives for the last set of `build`,
/// after checking that both readers agree on it.
fn last_set_size(build: &Build) -> usize {
    let nal = build.nal();
    agrees(&nal);
    let sps = scuffle_h265::SpsNALUnit::parse(std::io::Cursor::new(&nal[..])).expect("parses");
    let sets = &sps.rbsp.short_term_ref_pic_sets;
    *sets.num_delta_pocs.last().unwrap() as usize
}

#[test]
fn sub_layers_with_both_profile_and_level_are_read_as_the_spec_lays_them_out() {
    // Six sub-layers below the highest, each with a profile and a level:
    // scuffle-h265 reads each such level twice, once with the profile and
    // once on its own, so everything after it is out of place there.
    let build = Build {
        sub_layers: vec![(true, true); 6],
        nesting: false,
        ..Build::default()
    };
    let nal = build.nal();
    let ours = parse_sps(&nal).expect("parses");
    assert_eq!(ours, parse_sps(&Build::default().nal()).unwrap());
    assert_ne!(
        scuffle_sps(&nal),
        Some(ours),
        "scuffle reads the levels twice"
    );

    // A sub-layer with only one of them is the same in both.
    for flags in [(true, false), (false, true), (false, false)] {
        agrees(
            &Build {
                sub_layers: vec![flags; 6],
                nesting: false,
                ..Build::default()
            }
            .nal(),
        );
    }
    // Seven sub-layers below the highest is more than an SPS may hold.
    assert!(
        parse_sps(
            &Build {
                sub_layers: vec![(false, false); 7],
                ..Build::default()
            }
            .nal()
        )
        .is_none()
    );
}

#[test]
fn a_predicted_set_with_more_than_sixteen_following_pictures_is_refused() {
    // Sixteen pictures after the current one, then a set predicted from it
    // one later that keeps them all and adds its own: seventeen. scuffle
    // bounds only the pictures before the current one (it re-checks
    // num_negative_pics where it means num_positive_pics) and accepts it.
    let build = Build {
        ordering: vec![[16, 4, 0]],
        ref_sets: vec![
            RefSet::Explicit {
                negative: vec![],
                positive: vec![0; 16],
            },
            RefSet::Predicted {
                negative_sign: false,
                abs_minus1: 0,
                flags: vec![(true, true); 17],
            },
        ],
        ..Build::default()
    };
    let nal = build.nal();
    assert!(parse_sps(&nal).is_none());
    let theirs = scuffle_h265::SpsNALUnit::parse(std::io::Cursor::new(&nal[..])).expect("parses");
    assert_eq!(theirs.rbsp.short_term_ref_pic_sets.num_positive_pics[1], 17);

    // Seventeen before it is refused by both.
    let build = Build {
        ordering: vec![[16, 4, 0]],
        ref_sets: vec![
            RefSet::Explicit {
                negative: vec![0; 16],
                positive: vec![],
            },
            RefSet::Predicted {
                negative_sign: true,
                abs_minus1: 0,
                flags: vec![(true, true); 17],
            },
        ],
        ..Build::default()
    };
    let nal = build.nal();
    assert!(parse_sps(&nal).is_none());
    assert!(scuffle_sps(&nal).is_none());
}

#[test]
fn chroma_location_outside_4_2_0_is_kept_as_the_spec_allows() {
    // H.265 E.3.1 says chroma_loc_info_present_flag "should" be 0 unless
    // ChromaArrayType is 1; scuffle refuses the SPS.
    let build = Build {
        chroma_format_idc: 2,
        ..Build::default()
    };
    let ours = parse_sps(&build.nal()).expect("parses");
    assert_eq!(ours.vui.unwrap().chroma_sample_loc_type_top_field, Some(2));
    assert!(scuffle_sps(&build.nal()).is_none());
}

#[test]
fn identity_matrix_needs_4_4_4() {
    let identity = |separate_planes| Build {
        chroma_format_idc: 3,
        separate_planes,
        vui: Some(VuiBuild {
            chroma_loc: None,
            signal: Some((false, Some([1, 13, 0]))),
            ..VuiBuild::default()
        }),
        ..Build::default()
    };
    agrees(&identity(false).nal());
    // The check is on chroma_format_idc, so separate colour planes do not
    // change it.
    agrees(&identity(true).nal());

    // Identity with 4:2:0, and YCgCo with chroma two bits deeper, are
    // refused by both.
    for build in [
        Build {
            vui: Some(VuiBuild {
                signal: Some((false, Some([1, 13, 0]))),
                ..VuiBuild::default()
            }),
            ..Build::default()
        },
        Build {
            depth_chroma_minus8: 4,
            vui: Some(VuiBuild {
                signal: Some((false, Some([1, 13, 8]))),
                ..VuiBuild::default()
            }),
            ..Build::default()
        },
    ] {
        assert!(parse_sps(&build.nal()).is_none());
        assert!(scuffle_sps(&build.nal()).is_none());
    }
}

#[test]
fn vui_constraints_scuffle_enforces_are_kept() {
    let refused = [
        // Field coding in a frame-only stream.
        Build {
            vui: Some(VuiBuild {
                field_seq: true,
                frame_field_info: true,
                ..VuiBuild::default()
            }),
            ..Build::default()
        },
        // Field coding without frame-field information.
        Build {
            frame_only: false,
            vui: Some(VuiBuild {
                field_seq: true,
                ..VuiBuild::default()
            }),
            ..Build::default()
        },
        // Progressive and interlaced without frame-field information.
        Build {
            interlaced: true,
            ..Build::default()
        },
        // A zero tick or time scale.
        Build {
            vui: Some(VuiBuild {
                timing: Some((0, 25, None)),
                ..VuiBuild::default()
            }),
            ..Build::default()
        },
        Build {
            vui: Some(VuiBuild {
                timing: Some((1, 0, None)),
                ..VuiBuild::default()
            }),
            ..Build::default()
        },
        // A default display window that leaves nothing.
        Build {
            vui: Some(VuiBuild {
                display_window: Some([960, 960, 0, 0]),
                ..VuiBuild::default()
            }),
            ..Build::default()
        },
        Build {
            window: Some([0, 0, 500, 0]),
            vui: Some(VuiBuild {
                display_window: Some([0, 0, 0, 580]),
                ..VuiBuild::default()
            }),
            ..Build::default()
        },
        // Level 255, a zero width, a reserved chroma format, 17-bit samples.
        Build {
            level_idc: 255,
            ..Build::default()
        },
        Build {
            width: 0,
            ..Build::default()
        },
        Build {
            chroma_format_idc: 4,
            ..Build::default()
        },
        Build {
            depth_luma_minus8: 9,
            ..Build::default()
        },
        // DPB sizes that shrink in a higher sub-layer.
        Build {
            sub_layers: vec![(false, false)],
            nesting: false,
            ordering_all: true,
            ordering: vec![[4, 2, 0], [3, 2, 0]],
            ..Build::default()
        },
        // A single sub-layer without temporal ID nesting.
        Build {
            nesting: false,
            ..Build::default()
        },
        // More pictures in a set than the DPB holds.
        Build {
            ordering: vec![[1, 0, 0]],
            ..Build::default()
        },
        // 33 long-term pictures.
        Build {
            long_term: Some((33, vec![0; 33])),
            ..Build::default()
        },
    ];
    for (index, build) in refused.iter().enumerate() {
        let nal = build.nal();
        assert!(parse_sps(&nal).is_none(), "case {index}");
        // None of these reaches a known scuffle panic.
        assert!(scuffle_sps(&nal).is_none(), "case {index}");
    }
}

// ---------------------------------------------------------------------------
// Hostile SPS
// ---------------------------------------------------------------------------

#[test]
fn exp_golomb_values_are_bounded_to_32_bits() {
    assert_eq!(read_ue(&mut BitReader::new(&[0b1000_0000])), Some(0));
    assert_eq!(read_ue(&mut BitReader::new(&[0b0110_0000])), Some(2));
    assert_eq!(read_ue(&mut BitReader::new(&[0b0001_1110])), Some(14));
    assert_eq!(read_ue(&mut BitReader::new(&[0; 16])), None);
    assert_eq!(
        read_ue(&mut BitReader::new(&[0, 0, 0, 0, 0x80, 0, 0, 0, 0])),
        None
    );
}

/// A change to a built SPS.
type Edit = Box<dyn Fn(&mut Build)>;

#[test]
fn an_overlong_exp_golomb_prefix_in_any_field_is_refused() {
    // 2^32 - 1 is the first value whose code needs 32 leading zeros.
    let wide = u64::from(u32::MAX);
    let cases: Vec<Edit> = vec![
        Box::new(move |b| b.width = wide),
        Box::new(move |b| b.height = 1 << 40),
        Box::new(move |b| b.window = Some([0, wide, 0, 0])),
        Box::new(move |b| b.depth_luma_minus8 = wide),
        Box::new(move |b| b.ordering = vec![[4, 2, wide]]),
        Box::new(move |b| b.blocks[1] = wide),
        Box::new(move |b| b.blocks[3] = 1 << 50),
        Box::new(move |b| b.ref_set_count = Some(wide)),
        Box::new(move |b| {
            b.ref_sets = vec![RefSet::Explicit {
                negative: vec![wide],
                positive: vec![],
            }]
        }),
        Box::new(move |b| b.long_term = Some((wide, vec![]))),
        Box::new(move |b| b.vui.as_mut().unwrap().chroma_loc = Some((wide, 0))),
        Box::new(move |b| b.vui.as_mut().unwrap().chroma_loc = Some((0, 1 << 60))),
        Box::new(move |b| b.vui.as_mut().unwrap().display_window = Some([0, 0, wide, 0])),
        Box::new(move |b| b.vui.as_mut().unwrap().timing = Some((1, 1, Some(wide)))),
    ];
    for (index, change) in cases.iter().enumerate() {
        let mut build = Build::default();
        change(&mut build);
        assert!(parse_sps(&build.nal()).is_none(), "case {index}");
    }
    // A run of zeros with no one bit after it ends the data.
    let mut nal = Build::default().nal();
    nal.truncate(30);
    nal.extend([0; 64]);
    assert!(parse_sps(&nal).is_none());
}

#[test]
fn values_at_the_32_bit_limit_do_not_overflow() {
    let largest = u64::from(u32::MAX) - 1;
    // The largest picture, the widest conformance window.
    let huge = Build {
        width: largest,
        height: largest,
        window: Some([largest; 4]),
        vui: Some(VuiBuild {
            timing: Some((u32::MAX, u32::MAX, Some(largest))),
            ..VuiBuild::default()
        }),
        ..Build::default()
    };
    let sps = parse_sps(&huge.nal()).expect("parses");
    assert_eq!(sps.width, u32::MAX - 1);
    assert_eq!(sps_dimensions(&sps), None);
    let timing = sps.vui.as_ref().unwrap().timing.unwrap();
    assert_eq!(timing.num_ticks_poc_diff_one_minus1, Some(u32::MAX - 1));

    // A picture too wide for an i32, uncropped.
    let wide = Build {
        width: largest,
        ..Build::default()
    };
    let sps = agrees(&wide.nal());
    assert_eq!(sps_dimensions(&sps), None);
    // A picture that fits exactly once cropped.
    let cropped = Build {
        width: u64::from(i32::MAX as u32) + 2,
        window: Some([1, 0, 0, 0]),
        ..Build::default()
    };
    let sps = agrees(&cropped.nal());
    assert_eq!(sps_dimensions(&sps), Some((i32::MAX, 2160)));

    // A default display window at the limit on top of a conformance window
    // at the limit: refused, without overflow.
    let display = Build {
        width: largest,
        window: Some([largest, largest, 0, 0]),
        vui: Some(VuiBuild {
            display_window: Some([largest, largest, largest, largest]),
            ..VuiBuild::default()
        }),
        ..Build::default()
    };
    assert!(parse_sps(&display.nal()).is_none());

    // The derived frame rate from the largest tick values.
    let mut track = crate::raw::RawTrack {
        codec_name: Some(crate::Codec::Hevc),
        ..Default::default()
    };
    let mut report = crate::ProbeReport::default();
    crate::codec::parameter_sets::enrich(&mut track, &huge.nal(), &mut report);
    assert_eq!(track.metadata.bit_depth, Some(10));
}

#[test]
fn the_largest_long_term_and_poc_fields_are_stepped_over() {
    let build = Build {
        poc_lsb_minus4: 12,
        long_term: Some((32, (0..32).map(|i| i * 2000).collect())),
        ..Build::default()
    };
    agrees(&build.nal());
}

#[test]
fn every_truncation_of_an_sps_is_refused_or_reads_the_same() {
    let full_build = Build {
        sub_layers: vec![(true, false), (false, true)],
        nesting: false,
        window: Some([0, 0, 0, 4]),
        long_term: Some((2, vec![1, 2])),
        vui: Some(VuiBuild {
            aspect: Some((255, 4, 3)),
            display_window: Some([2, 2, 2, 2]),
            ..VuiBuild::default()
        }),
        ..Build::default()
    };
    let nal = full_build.nal();
    let full = agrees(&nal);
    let mut accepted = 0;
    for len in 0..nal.len() {
        match parse_sps(&nal[..len]) {
            None => {}
            Some(sps) => {
                assert_eq!(sps, full, "length {len}");
                accepted += 1;
            }
        }
    }
    // Only the bitstream restriction, the extension flag and the trailing
    // bits are past the timing information.
    assert!((1..=5).contains(&accepted), "{accepted}");
}

#[test]
fn random_bytes_after_an_sps_header_never_panic() {
    // A deterministic stream of bytes behind a valid header and profile.
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let base = Build::default().nal();
    for round in 0..20_000 {
        let mut nal = base[..(round % 20).max(2)].to_vec();
        for _ in 0..(round % 97) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            nal.push(state as u8);
        }
        let _ = parse_sps(&nal);
        let _ = extract_h265_info(&nal);
    }
}

#[test]
fn a_non_sps_nal_header_is_refused() {
    let nal = Build::default().nal();
    let mut forbidden = nal.clone();
    forbidden[0] |= 0x80;
    let mut pps = nal.clone();
    pps[0] = 34 << 1;
    let mut temporal = nal.clone();
    temporal[1] = (temporal[1] & !7) | 2;
    let mut zero_temporal = nal.clone();
    zero_temporal[1] &= !7;
    for bad in [forbidden, pps, temporal, zero_temporal] {
        assert!(parse_sps(&bad).is_none());
        assert!(scuffle_sps(&bad).is_none());
    }
    assert!(parse_sps(&[]).is_none());
    assert!(parse_sps(&[0x42]).is_none());
}

// ---------------------------------------------------------------------------
// hvcC
// ---------------------------------------------------------------------------

#[test]
fn an_hvcc_record_reads_as_scuffle_reads_it() {
    let sps = Build::default().nal();
    let other = Build {
        width: 1280,
        height: 720,
        ..Build::default()
    }
    .nal();
    let record = hvcc_record(
        2,
        2,
        &[
            (32, vec![vec![0x40, 0x01, 0x0C]]),
            (33, vec![sps.clone(), other.clone()]),
            (34, vec![vec![0x44, 0x01, 0xC1], vec![]]),
            (39, vec![]),
            (33, vec![sps.clone()]),
        ],
    );
    let ours = our_hvcc(&record).expect("parses");
    assert_eq!(ours, (2, 2, vec![sps.clone(), other, sps]));
    assert_eq!(Some(ours), scuffle_hvcc(&record));
    let info = extract_h265_info(&record);
    assert_eq!(info.profile.as_deref(), Some("Main 10"));
    assert_eq!(info.color_transfer, Some(16));

    // Trailing bytes are ignored by both.
    let mut padded = record.clone();
    padded.extend([0xFF; 9]);
    assert_eq!(our_hvcc(&padded), scuffle_hvcc(&padded));
    assert!(our_hvcc(&padded).is_some());

    // Every truncation is refused by both.
    for len in 0..record.len() {
        assert_eq!(our_hvcc(&record[..len]), None, "length {len}");
        assert_eq!(scuffle_hvcc(&record[..len]), None, "length {len}");
    }
}

#[test]
fn a_malformed_hvcc_record_is_refused() {
    let sps = Build::default().nal();
    let good = hvcc_record(1, 0, &[(33, vec![sps.clone()])]);
    let mut cases = Vec::new();
    // Version 0, and the reserved two-byte NAL length size.
    let mut version = good.clone();
    version[0] = 0;
    cases.push(version);
    let mut length_size = good.clone();
    length_size[21] = 0xFE;
    cases.push(length_size);
    // A NAL unit type a record may not hold.
    cases.push(hvcc_record(1, 0, &[(1, vec![sps.clone()])]));
    // 255 arrays declared, one present.
    let mut arrays = good.clone();
    arrays[22] = 255;
    cases.push(arrays);
    // 65535 NAL units declared, one present.
    let mut nal_count = good.clone();
    nal_count[24] = 0xFF;
    nal_count[25] = 0xFF;
    cases.push(nal_count);
    // A NAL unit longer than the record.
    let mut nal_length = good.clone();
    nal_length[26] = 0xFF;
    nal_length[27] = 0xFF;
    cases.push(nal_length);
    for (index, record) in cases.iter().enumerate() {
        assert_eq!(our_hvcc(record), None, "case {index}");
        assert_eq!(scuffle_hvcc(record), None, "case {index}");
        assert_eq!(extract_h265_info(record).profile, None, "case {index}");
    }

    // The most arrays a record can declare, each with every NAL unit it
    // declares present, are read without allocating.
    let mut empty = hvcc_record(1, 0, &[]);
    empty[22] = 255;
    for _ in 0..255 {
        empty.extend([0x80 | 34, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00]);
    }
    assert_eq!(our_hvcc(&empty), scuffle_hvcc(&empty));
    assert_eq!(our_hvcc(&empty).map(|record| record.2.len()), Some(0));
}

#[test]
fn hevc_nal_length_size_extraction() {
    // Byte 21 = 0xFF -> lengthSizeMinusOne = 3, length size = 4
    let mut hvcc = vec![0u8; 23];
    hvcc[21] = 0xFF;
    assert_eq!(hevc_nal_length_size(&hvcc), 4);

    // Byte 21 = 0xFC -> lengthSizeMinusOne = 0, length size = 1
    hvcc[21] = 0xFC;
    assert_eq!(hevc_nal_length_size(&hvcc), 1);

    // Too short -> default 4
    assert_eq!(hevc_nal_length_size(&[0u8; 10]), 4);
}

#[test]
fn h265_profile_mapping() {
    assert_eq!(map_h265_profile(1).as_deref(), Some("Main"));
    assert_eq!(map_h265_profile(2).as_deref(), Some("Main 10"));
    assert_eq!(map_h265_profile(3).as_deref(), Some("Main Still Picture"));
    assert_eq!(map_h265_profile(0), None);
}

// ---------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------

/// Every HEVC SPS and `hvcC` record found in the files under `root`: SPS NAL
/// units after an Annex B start code or a two-byte length (as in an `hvcC`
/// array; a four-byte length ends in one), and records after an `hvcC` box
/// header or a Matroska `CodecPrivate` element ID.
fn corpus_parameter_sets(
    root: &std::path::Path,
) -> (
    std::collections::BTreeSet<Vec<u8>>,
    std::collections::BTreeSet<Vec<u8>>,
) {
    let mut sps = std::collections::BTreeSet::new();
    let mut records = std::collections::BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .expect("reading the corpus")
            .map(|entry| entry.expect("a corpus entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let data = std::fs::read(&path).expect("reading a fixture");
            collect_parameter_sets(&data, &mut sps, &mut records);
        }
    }
    (sps, records)
}

fn collect_parameter_sets(
    data: &[u8],
    sps: &mut std::collections::BTreeSet<Vec<u8>>,
    records: &mut std::collections::BTreeSet<Vec<u8>>,
) {
    let is_sps_header = |at: usize| {
        data.get(at..at + 2)
            .is_some_and(|header| header[0] & 0x7E == 33 << 1 && header[1] & 0x07 == 1)
    };
    for at in 0..data.len().saturating_sub(5) {
        // Annex B: up to the next start code, at most 4 KiB.
        if data[at..at + 3] == [0, 0, 1] && is_sps_header(at + 3) {
            let start = at + 3;
            let limit = data.len().min(start + 4096);
            let end = (start + 2..limit)
                .find(|&i| {
                    data.get(i..i + 3)
                        .is_some_and(|w| w[0] == 0 && w[1] == 0 && w[2] <= 1)
                })
                .unwrap_or(limit);
            sps.insert(data[start..end].to_vec());
        }
        // A two-byte length that ends inside the file.
        if is_sps_header(at + 2) {
            let length = usize::from(u16::from_be_bytes([data[at], data[at + 1]]));
            if (4..=4096).contains(&length)
                && let Some(nal) = data.get(at + 2..at + 2 + length)
            {
                sps.insert(nal.to_vec());
            }
        }
        // An hvcC box: its size, then the type.
        if data[at..at + 4] == *b"hvcC" && at >= 4 {
            let size = u32::from_be_bytes(data[at - 4..at].try_into().unwrap()) as usize;
            if let Some(record) = size
                .checked_sub(8)
                .and_then(|len| data.get(at + 4..at + 4 + len))
            {
                records.insert(record.to_vec());
            }
        }
        // A Matroska CodecPrivate holding a version 1 record: the element
        // ID, a one- or two-byte size, then the record.
        if data[at..at + 2] == [0x63, 0xA2] {
            let (size, header) = match data[at + 2] {
                byte if byte & 0x80 != 0 => (usize::from(byte & 0x7F), 3),
                byte if byte & 0x40 != 0 => (
                    (usize::from(byte & 0x3F) << 8) | usize::from(data[at + 3]),
                    4,
                ),
                _ => continue,
            };
            if let Some(record) = data.get(at + header..at + header + size)
                && record.first() == Some(&1)
            {
                records.insert(record.to_vec());
            }
        }
    }
}

#[test]
fn corpus_parameter_sets_read_as_scuffle_reads_them() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media");
    let (mut sps_set, records) = corpus_parameter_sets(&root);

    let mut records_compared = 0;
    let mut records_with_sps = 0;
    for record in &records {
        let ours = our_hvcc(record);
        assert_eq!(ours, scuffle_hvcc(record), "hvcC {record:02X?}");
        if let Some((_, _, nals)) = ours {
            records_compared += 1;
            records_with_sps += usize::from(!nals.is_empty());
            sps_set.extend(nals);
        }
    }

    let mut compared = 0;
    let mut refused_by_both = 0;
    let mut kept_from_scuffle = 0;
    let mut sub_layer_level_read_twice = Vec::new();
    let mut unexplained = Vec::new();
    for nal in &sps_set {
        let ours = parse_sps(nal);
        if !scuffle_safe(nal) {
            if ours.is_none() {
                kept_from_scuffle += 1;
            } else if reads_a_sub_layer_level_twice(nal) {
                sub_layer_level_read_twice.push(format!("{ours:?} vs a panic"));
            } else {
                unexplained.push(format!("{nal:02X?}: {ours:?} vs a panic"));
            }
            continue;
        }
        match (ours, scuffle_read(nal)) {
            (Some(ours), Ok(theirs)) if ours == theirs => compared += 1,
            (None, Err(_)) => refused_by_both += 1,
            (ours, theirs) if reads_a_sub_layer_level_twice(nal) => {
                sub_layer_level_read_twice.push(format!("{ours:?} vs {theirs:?}"));
            }
            (ours, theirs) => unexplained.push(format!("{nal:02X?}: {ours:?} vs {theirs:?}")),
        }
    }
    eprintln!(
        "corpus: {} hvcC records ({records_compared} valid, {records_with_sps} with an SPS); \
         {} SPS candidates: {compared} read identically by both readers, {refused_by_both} \
         refused by both, {kept_from_scuffle} refused here and not given to scuffle-h265, {} \
         read differently because scuffle-h265 reads a sub-layer level twice",
        records.len(),
        sps_set.len(),
        sub_layer_level_read_twice.len(),
    );
    for difference in &sub_layer_level_read_twice {
        eprintln!("  sub-layer level read twice: {difference}");
    }
    assert!(records_with_sps > 0, "the corpus holds hvcC records");
    assert!(compared > 0, "the corpus holds HEVC SPSs");
    assert!(
        unexplained.is_empty(),
        "readers disagree:\n{}",
        unexplained.join("\n")
    );
}

/// Whether a sub-layer of the SPS signals both a profile and a level,
/// whose level `scuffle-h265` reads twice.
fn reads_a_sub_layer_level_twice(nal: &[u8]) -> bool {
    let rbsp = crate::scan::h2645_unescape_rbsp(&nal[2..]);
    let mut bits = BitReader::new(&rbsp);
    let Some(max_sub_layers_minus1) = bits.skip_bits(4).and_then(|()| bits.read_bits(3)) else {
        return false;
    };
    if bits.skip_bits(97).is_none() {
        return false;
    }
    (0..max_sub_layers_minus1).any(|_| bits.read_bits(2) == Some(0b11))
}
