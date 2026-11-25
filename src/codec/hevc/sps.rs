//! HEVC sequence parameter set reader (Rec. ITU-T H.265 7.3.2.2), as far as
//! the VUI timing information.
//!
//! Every field up to there is read so that the VUI is reached bit-aligned,
//! but only the fields the crate uses are kept. The HRD parameters, the
//! bitstream restriction, the SPS extensions and the trailing bits are not
//! read. Every value is bounded before it is used in arithmetic, a loop or
//! an index, and nothing is allocated.

use crate::codec::bits::{BitReader, read_se, read_ue};

/// `nal_unit_type` of an SPS.
const SPS_NUT: u8 = 33;
/// Bits in one `profile_tier_level` profile entry, without its level.
const PROFILE_BITS: usize = 88;
/// Most short-term reference picture sets an SPS may hold.
const MAX_SHORT_TERM_SETS: u64 = 64;
/// Most pictures on either side of a short-term reference picture set.
const MAX_SET_PICTURES: usize = 16;
/// Most long-term reference pictures an SPS may hold.
const MAX_LONG_TERM_PICTURES: u64 = 32;
/// Largest `delta_poc_s0_minus1`, `delta_poc_s1_minus1` and
/// `abs_delta_rps_minus1`.
const MAX_DELTA_POC_MINUS1: u64 = (1 << 15) - 1;

/// The fields of an HEVC SPS the crate reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sps {
    /// `general_profile_idc`.
    pub(crate) profile_idc: u8,
    /// `general_progressive_source_flag`.
    pub(crate) progressive_source: bool,
    /// `general_interlaced_source_flag`.
    pub(crate) interlaced_source: bool,
    /// `general_level_idc`.
    pub(crate) level_idc: u8,
    pub(crate) chroma_format_idc: u8,
    pub(crate) bit_depth_luma_minus8: u8,
    /// `pic_width_in_luma_samples`, never zero.
    pub(crate) width: u32,
    /// `pic_height_in_luma_samples`, never zero.
    pub(crate) height: u32,
    /// The conformance window offsets, zero when there is none.
    pub(crate) conformance_window: Window,
    pub(crate) vui: Option<Vui>,
}

/// Window offsets in chroma sample units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Window {
    pub(crate) left: u32,
    pub(crate) right: u32,
    pub(crate) top: u32,
    pub(crate) bottom: u32,
}

/// The VUI fields the crate reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Vui {
    /// `aspect_ratio_idc`, 0 (unspecified) when absent.
    pub(crate) aspect_ratio_idc: u8,
    /// `sar_width` and `sar_height`, zero unless `aspect_ratio_idc` is 255.
    pub(crate) sar_width: u16,
    pub(crate) sar_height: u16,
    /// Whether `video_signal_type_present_flag` was set.
    pub(crate) video_full_range: bool,
    /// `colour_primaries`, `transfer_characteristics` and `matrix_coeffs`,
    /// 2 (unspecified) when absent.
    pub(crate) colour_primaries: u8,
    pub(crate) transfer_characteristics: u8,
    pub(crate) matrix_coeffs: u8,
    /// `chroma_sample_loc_type_top_field`, when present.
    pub(crate) chroma_sample_loc_type_top_field: Option<u32>,
    pub(crate) field_seq: bool,
    pub(crate) timing: Option<Timing>,
}

/// VUI timing information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timing {
    /// Never zero.
    pub(crate) num_units_in_tick: u32,
    /// Never zero.
    pub(crate) time_scale: u32,
    /// `vui_num_ticks_poc_diff_one_minus1`, when POC is proportional to
    /// timing.
    pub(crate) num_ticks_poc_diff_one_minus1: Option<u32>,
}

impl Sps {
    /// `SubWidthC` (Table 6-1).
    pub(crate) fn sub_width_c(&self) -> u32 {
        sub_width_c(self.chroma_format_idc)
    }

    /// `SubHeightC` (Table 6-1).
    pub(crate) fn sub_height_c(&self) -> u32 {
        sub_height_c(self.chroma_format_idc)
    }
}

fn sub_width_c(chroma_format_idc: u8) -> u32 {
    if matches!(chroma_format_idc, 1 | 2) {
        2
    } else {
        1
    }
}

fn sub_height_c(chroma_format_idc: u8) -> u32 {
    if chroma_format_idc == 1 { 2 } else { 1 }
}

/// Parses an SPS NAL unit, header included. Returns `None` when it is not
/// an SPS, is truncated before the last field read, or holds a value the
/// specification does not allow in a field that is read.
pub(crate) fn parse(nal: &[u8]) -> Option<Sps> {
    let [first, second, ref payload @ ..] = *nal else {
        return None;
    };
    // forbidden_zero_bit, nal_unit_type, nuh_layer_id, nuh_temporal_id_plus1
    if first & 0x80 != 0 || (first >> 1) & 0x3F != SPS_NUT {
        return None;
    }
    let layer_id = ((first & 1) << 5) | (second >> 3);
    if second & 0x07 != 1 {
        return None; // an SPS has TemporalId 0
    }
    let rbsp = crate::scan::h2645_unescape_rbsp(payload);
    parse_rbsp(&rbsp, layer_id)
}

/// Reads an unsigned Exp-Golomb value no greater than `max`.
fn ue_at_most(bits: &mut BitReader<'_>, max: u64) -> Option<u64> {
    read_ue(bits).filter(|value| *value <= max)
}

/// Reads an unsigned Exp-Golomb value that fits in 32 bits.
fn ue_u32(bits: &mut BitReader<'_>) -> Option<u32> {
    u32::try_from(read_ue(bits)?).ok()
}

/// Reads one bit as a flag.
fn flag(bits: &mut BitReader<'_>) -> Option<bool> {
    Some(bits.read_bit()? == 1)
}

/// Reads `count` bits, at most 8, as a byte.
fn byte(bits: &mut BitReader<'_>, count: usize) -> Option<u8> {
    u8::try_from(bits.read_bits(count)?).ok()
}

fn parse_rbsp(rbsp: &[u8], layer_id: u8) -> Option<Sps> {
    let mut bits = BitReader::new(rbsp);

    bits.skip_bits(4)?; // sps_video_parameter_set_id
    let max_sub_layers_minus1 = byte(&mut bits, 3)?;
    if max_sub_layers_minus1 > 6 {
        return None;
    }
    let temporal_id_nesting = flag(&mut bits)?;
    if max_sub_layers_minus1 == 0 && !temporal_id_nesting {
        return None;
    }
    let profile = profile_tier_level(&mut bits, max_sub_layers_minus1)?;

    ue_at_most(&mut bits, 15)?; // sps_seq_parameter_set_id
    let chroma_format_idc = byte_value(ue_at_most(&mut bits, 3)?)?;
    if chroma_format_idc == 3 {
        bits.skip_bits(1)?; // separate_colour_plane_flag
    }
    let width = ue_u32(&mut bits).filter(|value| *value != 0)?;
    let height = ue_u32(&mut bits).filter(|value| *value != 0)?;
    let conformance_window = if flag(&mut bits)? {
        Window {
            left: ue_u32(&mut bits)?,
            right: ue_u32(&mut bits)?,
            top: ue_u32(&mut bits)?,
            bottom: ue_u32(&mut bits)?,
        }
    } else {
        Window::default()
    };
    let bit_depth_luma_minus8 = byte_value(ue_at_most(&mut bits, 8)?)?;
    let bit_depth_chroma_minus8 = byte_value(ue_at_most(&mut bits, 8)?)?;
    let log2_max_poc_lsb = ue_at_most(&mut bits, 12)? + 4;

    let max_dec_pic_buffering_minus1 = sub_layer_ordering_info(&mut bits, max_sub_layers_minus1)?;

    // Every block size below is a 32-bit value, so the sums fit in a u64.
    let min_cb_log2 = read_ue(&mut bits)? + 3;
    let ctb_log2 = min_cb_log2 + read_ue(&mut bits)?;
    let min_tb_log2 = read_ue(&mut bits)? + 2;
    read_ue(&mut bits)?; // log2_diff_max_min_luma_transform_block_size
    let max_depth = ctb_log2.checked_sub(min_tb_log2)?;
    ue_at_most(&mut bits, max_depth)?; // max_transform_hierarchy_depth_inter
    ue_at_most(&mut bits, max_depth)?; // max_transform_hierarchy_depth_intra

    // scaling_list_enabled_flag, then sps_scaling_list_data_present_flag
    if flag(&mut bits)? && flag(&mut bits)? {
        skip_scaling_list_data(&mut bits)?;
    }
    bits.skip_bits(2)?; // amp_enabled_flag, sample_adaptive_offset_enabled_flag
    if flag(&mut bits)? {
        skip_pcm(
            &mut bits,
            bit_depth_luma_minus8 + 8,
            bit_depth_chroma_minus8 + 8,
            min_cb_log2,
            ctb_log2,
        )?;
    }

    let num_short_term_sets = ue_at_most(&mut bits, MAX_SHORT_TERM_SETS)?;
    // Pictures a set may hold on its own, by 7.4.8; a layer above the
    // base is bounded by the largest DPB instead.
    let max_set_pictures = if layer_id == 0 {
        max_dec_pic_buffering_minus1
    } else {
        MAX_SET_PICTURES as u64
    };
    skip_short_term_ref_pic_sets(&mut bits, num_short_term_sets, layer_id, max_set_pictures)?;

    if flag(&mut bits)? {
        // long_term_ref_pics_present_flag
        let count = ue_at_most(&mut bits, MAX_LONG_TERM_PICTURES)?;
        for _ in 0..count {
            // lt_ref_pic_poc_lsb_sps, used_by_curr_pic_lt_sps_flag
            bits.skip_bits(usize::try_from(log2_max_poc_lsb).ok()? + 1)?;
        }
    }
    // sps_temporal_mvp_enabled_flag, strong_intra_smoothing_enabled_flag
    bits.skip_bits(2)?;

    let vui = if flag(&mut bits)? {
        let context = VuiContext {
            chroma_format_idc,
            bit_depth_luma_minus8,
            bit_depth_chroma_minus8,
            profile,
            conformance_window,
            sub_width_c: u64::from(sub_width_c(chroma_format_idc)),
            sub_height_c: u64::from(sub_height_c(chroma_format_idc)),
            width,
            height,
        };
        Some(vui_parameters(&mut bits, &context)?)
    } else {
        None
    };

    Some(Sps {
        profile_idc: profile.profile_idc,
        progressive_source: profile.progressive_source,
        interlaced_source: profile.interlaced_source,
        level_idc: profile.level_idc,
        chroma_format_idc,
        bit_depth_luma_minus8,
        width,
        height,
        conformance_window,
        vui,
    })
}

/// A small value as a byte.
fn byte_value(value: u64) -> Option<u8> {
    u8::try_from(value).ok()
}

/// The general profile fields of `profile_tier_level()` (7.3.3).
#[derive(Debug, Clone, Copy)]
struct GeneralProfile {
    profile_idc: u8,
    progressive_source: bool,
    interlaced_source: bool,
    frame_only_constraint: bool,
    level_idc: u8,
}

/// Reads `profile_tier_level(1, max_sub_layers_minus1)`, keeping only the
/// general profile fields.
fn profile_tier_level(
    bits: &mut BitReader<'_>,
    max_sub_layers_minus1: u8,
) -> Option<GeneralProfile> {
    bits.skip_bits(3)?; // general_profile_space, general_tier_flag
    let profile_idc = byte(bits, 5)?;
    bits.skip_bits(32)?; // general_profile_compatibility_flag[32]
    let progressive_source = flag(bits)?;
    let interlaced_source = flag(bits)?;
    bits.skip_bits(1)?; // general_non_packed_constraint_flag
    let frame_only_constraint = flag(bits)?;
    // The 43 constraint bits and general_inbld_flag or its reserved bit.
    bits.skip_bits(44)?;
    let level_idc = byte(bits, 8)?;
    if level_idc == 255 {
        return None;
    }

    // Each sub-layer holds a profile entry, a level entry, both or neither.
    let mut sub_layer_bits = 0;
    for _ in 0..max_sub_layers_minus1 {
        if flag(bits)? {
            sub_layer_bits += PROFILE_BITS;
        }
        if flag(bits)? {
            sub_layer_bits += 8;
        }
    }
    if max_sub_layers_minus1 > 0 {
        // reserved_zero_2bits for each sub-layer up to eight
        bits.skip_bits(2 * (8 - usize::from(max_sub_layers_minus1)))?;
    }
    bits.skip_bits(sub_layer_bits)?;

    Some(GeneralProfile {
        profile_idc,
        progressive_source,
        interlaced_source,
        frame_only_constraint,
        level_idc,
    })
}

/// Reads the sub-layer ordering information (7.3.2.2.1) and returns
/// `sps_max_dec_pic_buffering_minus1` of the highest sub-layer.
fn sub_layer_ordering_info(bits: &mut BitReader<'_>, max_sub_layers_minus1: u8) -> Option<u64> {
    let sets = if flag(bits)? {
        max_sub_layers_minus1 + 1
    } else {
        1
    };
    let mut buffering = 0;
    let mut reorder = 0;
    for _ in 0..sets {
        // Each sub-layer's DPB and reorder depth is at least the one below.
        let next_buffering = ue_at_most(bits, 16)?;
        let next_reorder = ue_at_most(bits, next_buffering)?;
        if next_buffering < buffering || next_reorder < reorder {
            return None;
        }
        buffering = next_buffering;
        reorder = next_reorder;
        read_ue(bits)?; // sps_max_latency_increase_plus1
    }
    Some(buffering)
}

/// Steps through `scaling_list_data()` (7.3.4).
///
/// Refuses a list predicted from beyond the first matrix of its size and a
/// coefficient outside its range (7.4.5).
fn skip_scaling_list_data(bits: &mut BitReader<'_>) -> Option<()> {
    for size_id in 0..4_u64 {
        let step = if size_id == 3 { 3 } else { 1 };
        let mut matrix_id = 0;
        while matrix_id < 6 {
            if flag(bits)? {
                if size_id > 1 && !(-7..=247).contains(&read_se(bits)?) {
                    return None; // scaling_list_dc_coef_minus8
                }
                for _ in 0..(16_u32 << (2 * size_id)).min(64) {
                    if !(-128..=127).contains(&read_se(bits)?) {
                        return None; // scaling_list_delta_coef
                    }
                }
            } else {
                // scaling_list_pred_matrix_id_delta
                ue_at_most(bits, matrix_id / step)?;
            }
            matrix_id += step;
        }
    }
    Some(())
}

/// Steps through the PCM parameters, refusing PCM samples deeper than the
/// picture's and PCM block sizes outside the coding block sizes (7.4.3.2.1).
fn skip_pcm(
    bits: &mut BitReader<'_>,
    bit_depth_luma: u8,
    bit_depth_chroma: u8,
    min_cb_log2: u64,
    ctb_log2: u64,
) -> Option<()> {
    if byte(bits, 4)? >= bit_depth_luma || byte(bits, 4)? >= bit_depth_chroma {
        return None; // pcm_sample_bit_depth_{luma,chroma}_minus1
    }
    let min_log2 = read_ue(bits)? + 3;
    let max_log2 = min_log2.checked_add(read_ue(bits)?)?;
    if min_log2 < min_cb_log2.min(5) || max_log2 > ctb_log2.min(5) {
        return None;
    }
    bits.skip_bits(1) // pcm_loop_filter_disabled_flag
}

/// One short-term reference picture set's delta POCs (7.4.8).
#[derive(Debug, Clone, Copy, Default)]
struct RefPicSet {
    negative: usize,
    positive: usize,
    /// `DeltaPocS0` then `DeltaPocS1`, each at most `MAX_SET_PICTURES`.
    s0: [i32; MAX_SET_PICTURES],
    s1: [i32; MAX_SET_PICTURES],
}

impl RefPicSet {
    fn delta_pocs(&self) -> usize {
        self.negative + self.positive
    }

    fn push_s0(&mut self, delta: i32) -> Option<()> {
        *self.s0.get_mut(self.negative)? = delta;
        self.negative += 1;
        Some(())
    }

    fn push_s1(&mut self, delta: i32) -> Option<()> {
        *self.s1.get_mut(self.positive)? = delta;
        self.positive += 1;
        Some(())
    }
}

/// Steps through the SPS's `st_ref_pic_set()` entries (7.3.7), deriving
/// each set's delta POCs, which an inter-predicted set after it needs to
/// know how many flags it holds.
fn skip_short_term_ref_pic_sets(
    bits: &mut BitReader<'_>,
    count: u64,
    layer_id: u8,
    max_set_pictures: u64,
) -> Option<()> {
    let mut sets = [RefPicSet::default(); MAX_SHORT_TERM_SETS as usize];
    for index in 0..usize::try_from(count).ok()? {
        // inter_ref_pic_set_prediction_flag. Within the SPS the reference
        // set is always the previous one: delta_idx_minus1 is only present
        // in a slice header.
        let set = if index > 0 && flag(bits)? {
            let reference = sets.get(index - 1)?;
            predicted_set(bits, reference)?
        } else {
            explicit_set(bits, layer_id, max_set_pictures)?
        };
        *sets.get_mut(index)? = set;
    }
    Some(())
}

fn explicit_set(bits: &mut BitReader<'_>, layer_id: u8, max_pictures: u64) -> Option<RefPicSet> {
    let negative = ue_at_most(bits, max_pictures)?;
    let positive_max = if layer_id == 0 {
        max_pictures - negative
    } else {
        max_pictures
    };
    let positive = ue_at_most(bits, positive_max)?;
    let mut set = RefPicSet::default();
    let mut poc = 0_i32;
    for _ in 0..negative {
        // delta_poc_s0_minus1, used_by_curr_pic_s0_flag
        let step = i32::try_from(ue_at_most(bits, MAX_DELTA_POC_MINUS1)? + 1).ok()?;
        poc = poc.checked_sub(step)?;
        set.push_s0(poc)?;
        bits.skip_bits(1)?;
    }
    poc = 0;
    for _ in 0..positive {
        // delta_poc_s1_minus1, used_by_curr_pic_s1_flag
        let step = i32::try_from(ue_at_most(bits, MAX_DELTA_POC_MINUS1)? + 1).ok()?;
        poc = poc.checked_add(step)?;
        set.push_s1(poc)?;
        bits.skip_bits(1)?;
    }
    Some(set)
}

/// Reads a set predicted from `reference` and derives its delta POCs by
/// equations 7-61 and 7-62. Refuses a set with more than
/// `MAX_SET_PICTURES` pictures on either side.
fn predicted_set(bits: &mut BitReader<'_>, reference: &RefPicSet) -> Option<RefPicSet> {
    let negative_sign = flag(bits)?; // delta_rps_sign
    let magnitude = i32::try_from(ue_at_most(bits, MAX_DELTA_POC_MINUS1)? + 1).ok()?;
    let delta_rps = if negative_sign { -magnitude } else { magnitude };

    // use_delta_flag for each reference picture and the reference set's own
    // picture, inferred set when absent.
    let mut use_delta = [true; 2 * MAX_SET_PICTURES + 1];
    for slot in use_delta.iter_mut().take(reference.delta_pocs() + 1) {
        // used_by_curr_pic_flag, then use_delta_flag when it is clear
        if !flag(bits)? {
            *slot = flag(bits)?;
        }
    }
    let uses = |at: usize| use_delta.get(at).copied();
    let s0 = reference.s0.get(..reference.negative)?;
    let s1 = reference.s1.get(..reference.positive)?;
    let own = reference.delta_pocs();

    let mut set = RefPicSet::default();
    for (j, delta) in s1.iter().enumerate().rev() {
        let poc = delta.checked_add(delta_rps)?;
        if poc < 0 && uses(reference.negative + j)? {
            set.push_s0(poc)?;
        }
    }
    if delta_rps < 0 && uses(own)? {
        set.push_s0(delta_rps)?;
    }
    for (j, delta) in s0.iter().enumerate() {
        let poc = delta.checked_add(delta_rps)?;
        if poc < 0 && uses(j)? {
            set.push_s0(poc)?;
        }
    }

    for (j, delta) in s0.iter().enumerate().rev() {
        let poc = delta.checked_add(delta_rps)?;
        if poc > 0 && uses(j)? {
            set.push_s1(poc)?;
        }
    }
    if delta_rps > 0 && uses(own)? {
        set.push_s1(delta_rps)?;
    }
    for (j, delta) in s1.iter().enumerate() {
        let poc = delta.checked_add(delta_rps)?;
        if poc > 0 && uses(reference.negative + j)? {
            set.push_s1(poc)?;
        }
    }
    Some(set)
}

/// SPS values the VUI semantics depend on.
struct VuiContext {
    chroma_format_idc: u8,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    profile: GeneralProfile,
    conformance_window: Window,
    sub_width_c: u64,
    sub_height_c: u64,
    width: u32,
    height: u32,
}

/// Reads `vui_parameters()` (E.2.1) up to the timing information.
fn vui_parameters(bits: &mut BitReader<'_>, sps: &VuiContext) -> Option<Vui> {
    let mut vui = Vui {
        aspect_ratio_idc: 0,
        sar_width: 0,
        sar_height: 0,
        video_full_range: false,
        colour_primaries: 2,
        transfer_characteristics: 2,
        matrix_coeffs: 2,
        chroma_sample_loc_type_top_field: None,
        field_seq: false,
        timing: None,
    };

    if flag(bits)? {
        vui.aspect_ratio_idc = byte(bits, 8)?;
        if vui.aspect_ratio_idc == 255 {
            vui.sar_width = u16::try_from(bits.read_bits(16)?).ok()?;
            vui.sar_height = u16::try_from(bits.read_bits(16)?).ok()?;
        }
    }
    if flag(bits)? {
        bits.skip_bits(1)?; // overscan_appropriate_flag
    }
    if flag(bits)? {
        bits.skip_bits(3)?; // video_format
        vui.video_full_range = flag(bits)?;
        if flag(bits)? {
            vui.colour_primaries = byte(bits, 8)?;
            vui.transfer_characteristics = byte(bits, 8)?;
            vui.matrix_coeffs = byte(bits, 8)?;
            let luma = sps.bit_depth_luma_minus8;
            let chroma = sps.bit_depth_chroma_minus8;
            let full_chroma = sps.chroma_format_idc == 3;
            // Identity needs equal depths and 4:4:4; YCgCo needs equal
            // depths, or chroma one bit deeper and 4:4:4.
            let refused = match vui.matrix_coeffs {
                0 => !(chroma == luma && full_chroma),
                8 => !(chroma == luma || (Some(chroma) == luma.checked_add(1) && full_chroma)),
                _ => false,
            };
            if refused {
                return None;
            }
        }
    }
    if flag(bits)? {
        vui.chroma_sample_loc_type_top_field = Some(ue_u32(bits)?);
        read_ue(bits)?; // chroma_sample_loc_type_bottom_field
    }
    bits.skip_bits(1)?; // neutral_chroma_indication_flag
    vui.field_seq = flag(bits)?;
    if vui.field_seq && sps.profile.frame_only_constraint {
        return None;
    }
    let frame_field_info_present = flag(bits)?;
    if !frame_field_info_present
        && (vui.field_seq || (sps.profile.progressive_source && sps.profile.interlaced_source))
    {
        return None;
    }
    if flag(bits)? {
        // The default display window, inside the conformance window, must
        // leave part of the picture.
        let window = sps.conformance_window;
        let left = u64::from(window.left) + read_ue(bits)?;
        let right = u64::from(window.right) + read_ue(bits)?;
        let top = u64::from(window.top) + read_ue(bits)?;
        let bottom = u64::from(window.bottom) + read_ue(bits)?;
        let fits = |low: u64, high: u64, scale: u64, size: u32| {
            low.checked_add(high)
                .and_then(|sum| sum.checked_mul(scale))
                .is_some_and(|lost| lost < u64::from(size))
        };
        if !fits(left, right, sps.sub_width_c, sps.width)
            || !fits(top, bottom, sps.sub_height_c, sps.height)
        {
            return None;
        }
    }
    if flag(bits)? {
        let num_units_in_tick = bits.read_bits(32).filter(|value| *value != 0)?;
        let time_scale = bits.read_bits(32).filter(|value| *value != 0)?;
        // vui_poc_proportional_to_timing_flag
        let num_ticks_poc_diff_one_minus1 = if flag(bits)? {
            Some(ue_u32(bits)?)
        } else {
            None
        };
        vui.timing = Some(Timing {
            num_units_in_tick,
            time_scale,
            num_ticks_poc_diff_one_minus1,
        });
    }
    Some(vui)
}
