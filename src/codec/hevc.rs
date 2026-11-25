//! HEVC decoder configuration (hvcC): profile, bit depth, transfer and NAL
//! length size.

use super::CodecInfo;
use super::bits::{BitReader, EXP_GOLOMB_ZERO_RUN, has_terminated_zero_run, read_se, read_ue};

/// Bits in one `profile_tier_level` profile entry, without its level.
const PROFILE_BITS: usize = 88;
/// Bits in one `profile_tier_level` level entry.
const LEVEL_BITS: usize = 8;

/// Parses an HEVC SPS NAL unit.
///
/// `scuffle-h265` does arithmetic on SPS fields unchecked, so an SPS that
/// would overflow it is refused here first: one whose transform block is
/// larger than its coding tree block, one whose scaling list is predicted
/// from before the first matrix or has a coefficient out of range, and one
/// with a value wider than 32 bits up to its PCM block sizes.
///
/// So is one with a run of zero bits, after the profile and level, long
/// enough to overflow the dependency's Exp-Golomb reader. The profile and
/// level hold reserved zero bits of their own and no Exp-Golomb value.
pub(crate) fn parse_sps(nal: &[u8]) -> Option<scuffle_h265::SpsNALUnit> {
    let rbsp = crate::scan::h2645_unescape_rbsp(nal.get(2..)?);
    let prefix = sps_prefix(&rbsp)?;
    if prefix.min_transform_log2 > prefix.coding_tree_log2
        || has_terminated_zero_run(&rbsp, prefix.first_exp_golomb_bit, EXP_GOLOMB_ZERO_RUN)
    {
        return None;
    }
    scuffle_h265::SpsNALUnit::parse(std::io::Cursor::new(nal)).ok()
}

/// The cropped picture size an SPS declares, or `None` when its conformance
/// window does not fit in the picture. The parser's own `cropped_width` and
/// `cropped_height` subtract the window unchecked.
pub(crate) fn sps_dimensions(sps: &scuffle_h265::SpsRbsp) -> Option<(i32, i32)> {
    let window = &sps.conformance_window;
    let cropped = |full: u64, scale: u8, low: u64, high: u64| {
        let lost = low.checked_add(high)?.checked_mul(u64::from(scale))?;
        i32::try_from(full.checked_sub(lost)?).ok()
    };
    let width = cropped(
        sps.pic_width_in_luma_samples.get(),
        sps.sub_width_c(),
        window.conf_win_left_offset,
        window.conf_win_right_offset,
    )?;
    let height = cropped(
        sps.pic_height_in_luma_samples.get(),
        sps.sub_height_c(),
        window.conf_win_top_offset,
        window.conf_win_bottom_offset,
    )?;
    Some((width, height))
}

struct SpsPrefix {
    /// Where the first Exp-Golomb value starts, after the profile and level.
    first_exp_golomb_bit: usize,
    coding_tree_log2: u64,
    min_transform_log2: u64,
}

/// Reads an SPS as far as its PCM block sizes, following the field order
/// and widths `scuffle-h265` reads so both see the same values.
///
/// Returns `None` when the data ends first, when a value is wider than 32
/// bits, or when the scaling list data would overflow the dependency.
///
/// `rbsp` is the NAL unit after its two-byte header, where emulation
/// prevention starts, with the emulation prevention bytes removed.
fn sps_prefix(rbsp: &[u8]) -> Option<SpsPrefix> {
    let mut bits = BitReader::new(rbsp);

    bits.skip_bits(4)?; // sps_video_parameter_set_id
    let max_sub_layers_minus1 = bits.read_bits(3)? as usize;
    bits.skip_bits(1)?; // sps_temporal_id_nesting_flag

    bits.skip_bits(PROFILE_BITS + LEVEL_BITS)?; // general profile and level
    let mut sub_layer_bits = 0;
    for _ in 0..max_sub_layers_minus1 {
        let profile_present = bits.read_bit()? == 1;
        let level_present = bits.read_bit()? == 1;
        if profile_present {
            sub_layer_bits += PROFILE_BITS;
            // The dependency reads the level with the profile and then
            // again on its own, so a sub-layer with both costs two.
            if level_present {
                sub_layer_bits += LEVEL_BITS;
            }
        }
        if level_present {
            sub_layer_bits += LEVEL_BITS;
        }
    }
    if max_sub_layers_minus1 > 0 {
        bits.skip_bits(2 * (8 - max_sub_layers_minus1))?; // reserved_zero_2bits
    }
    bits.skip_bits(sub_layer_bits)?;
    let first_exp_golomb_bit = bits.position();

    read_ue(&mut bits)?; // sps_seq_parameter_set_id
    if read_ue(&mut bits)? == 3 {
        bits.skip_bits(1)?; // separate_colour_plane_flag
    }
    read_ue(&mut bits)?; // pic_width_in_luma_samples
    read_ue(&mut bits)?; // pic_height_in_luma_samples
    if bits.read_bit()? == 1 {
        for _ in 0..4 {
            read_ue(&mut bits)?; // conformance window offsets
        }
    }
    for _ in 0..3 {
        read_ue(&mut bits)?; // bit depths and log2_max_pic_order_cnt_lsb_minus4
    }
    let ordering_sets = if bits.read_bit()? == 1 {
        max_sub_layers_minus1 + 1
    } else {
        1
    };
    for _ in 0..ordering_sets * 3 {
        read_ue(&mut bits)?;
    }

    let min_coding_log2 = read_ue(&mut bits)? + 3;
    let coding_tree_log2 = min_coding_log2 + read_ue(&mut bits)?;
    let min_transform_log2 = read_ue(&mut bits)? + 2;
    for _ in 0..3 {
        // log2_diff_max_min_luma_transform_block_size and
        // max_transform_hierarchy_depth_inter and _intra
        read_ue(&mut bits)?;
    }
    // scaling_list_enabled_flag, then sps_scaling_list_data_present_flag
    if bits.read_bit()? == 1 && bits.read_bit()? == 1 {
        check_scaling_list_data(&mut bits)?;
    }
    bits.skip_bits(2)?; // amp_enabled_flag, sample_adaptive_offset_enabled_flag
    if bits.read_bit()? == 1 {
        bits.skip_bits(8)?; // PCM sample bit depths
        // The PCM block sizes, which the dependency adds to unchecked.
        read_ue(&mut bits)?;
        read_ue(&mut bits)?;
    }
    Some(SpsPrefix {
        first_exp_golomb_bit,
        coding_tree_log2,
        min_transform_log2,
    })
}

/// Reads `scaling_list_data()` as `scuffle-h265` does, which steps through
/// the 32x32 matrices by three.
///
/// Refuses a list predicted from before the first matrix, which the
/// dependency subtracts unchecked and then indexes a table with, and a
/// coefficient outside its range, which it adds unchecked.
fn check_scaling_list_data(bits: &mut BitReader<'_>) -> Option<()> {
    for size_id in 0..4 {
        let step = if size_id == 3 { 3 } else { 1 };
        let mut matrix_id = 0;
        while matrix_id < 6 {
            if bits.read_bit()? == 0 {
                // scaling_list_pred_matrix_id_delta
                if read_ue(bits)? * step > matrix_id {
                    return None;
                }
            } else {
                if size_id > 1 && !(-7..=247).contains(&read_se(bits)?) {
                    return None; // scaling_list_dc_coef_minus8
                }
                for _ in 0..(16 << (2 * size_id)).min(64) {
                    if !(-128..=127).contains(&read_se(bits)?) {
                        return None; // scaling_list_delta_coef
                    }
                }
            }
            matrix_id += step;
        }
    }
    Some(())
}

/// Extracts codec profile, bit depth, and color transfer characteristics from
/// an HEVC (H.265) decoder configuration record.
pub(crate) fn extract_h265_info(codec_private: &[u8]) -> CodecInfo {
    use scuffle_h265::{HEVCDecoderConfigurationRecord, NALUnitType};
    use std::io;

    if let Ok(config) = HEVCDecoderConfigurationRecord::demux(&mut io::Cursor::new(codec_private)) {
        let profile = map_h265_profile(config.general_profile_idc);
        let bit_depth = Some(config.bit_depth_luma_minus8 as i32 + 8);

        // Extract transfer_characteristics from the first SPS VUI.
        let color_transfer = config
            .arrays
            .iter()
            .filter(|arr| arr.nal_unit_type == NALUnitType::SpsNut)
            .flat_map(|arr| arr.nalus.iter())
            .find_map(|nalu_bytes| {
                let sps = parse_sps(nalu_bytes)?;
                let vui = sps.rbsp.vui_parameters?;
                let tc = vui.video_signal_type.transfer_characteristics;
                if tc > 0 && tc != 2 {
                    Some(tc as u32)
                } else {
                    None
                }
            });

        return CodecInfo {
            profile,
            bit_depth,
            color_transfer,
            ..CodecInfo::default()
        };
    }

    let Some(sps) = parse_sps(codec_private) else {
        return CodecInfo::default();
    };
    let color_transfer = sps.rbsp.vui_parameters.as_ref().and_then(|vui| {
        let tc = vui.video_signal_type.transfer_characteristics;
        if tc > 0 && tc != 2 {
            Some(tc as u32)
        } else {
            None
        }
    });

    CodecInfo {
        profile: map_h265_profile(sps.rbsp.profile_tier_level.general_profile.profile_idc),
        bit_depth: Some(sps.rbsp.bit_depth_luma_minus8 as i32 + 8),
        color_transfer,
        ..CodecInfo::default()
    }
}

/// Extract the NAL unit length prefix size from an HEVCDecoderConfigurationRecord.
///
/// Returns `lengthSizeMinusOne + 1` (typically 4). Falls back to 4 if the
/// record is too short.
pub(crate) fn hevc_nal_length_size(hvcc: &[u8]) -> usize {
    if hvcc.len() > 21 {
        ((hvcc[21] & 0x03) as usize) + 1
    } else {
        4
    }
}

/// Maps an H.265 general_profile_idc value to a human-readable profile name.
fn map_h265_profile(profile_idc: u8) -> Option<String> {
    let name = match profile_idc {
        1 => "Main",
        2 => "Main 10",
        3 => "Main Still Picture",
        4 => "Format Range Extensions",
        5 => "High Throughput",
        9 => "Screen Content Coding",
        11 => "High Throughput Screen Content Coding",
        _ => return None,
    };
    Some(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::bits::{TestBitWriter, escape_rbsp};

    /// A Main profile SPS: 8x8 minimum coding block, 16x16 coding tree block.
    const SPS: [u8; 40] = [
        0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x03, 0x00, 0x1E, 0xA0, 0x20, 0x81, 0x05, 0x96, 0x56, 0xD4, 0xA4, 0xC2, 0xF0, 0x16, 0x80,
        0x80, 0x00, 0x00, 0x03, 0x00, 0x80, 0x00, 0x00, 0x0C, 0x04,
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

    /// Scaling list data in the order `scuffle-h265` reads it, each matrix
    /// written by `matrix(size_id, matrix_id, out)`, and no AMP, SAO or PCM.
    fn scaling_lists(matrix: impl Fn(usize, u64, &mut TestBitWriter)) -> Vec<u8> {
        sps_with(None, |out| {
            out.write_bits(0b11, 2); // enabled and present in the SPS
            for size_id in 0..4 {
                let step = if size_id == 3 { 3 } else { 1 };
                for matrix_id in (0..6).step_by(step) {
                    matrix(size_id, matrix_id, out);
                }
            }
            out.write_bits(0, 3);
        })
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

    /// The SPS parses here exactly as the dependency parses it alone.
    fn parses_unchanged(nal: &[u8]) -> scuffle_h265::SpsNALUnit {
        let sps = parse_sps(nal).expect("SPS parses");
        let direct = scuffle_h265::SpsNALUnit::parse(std::io::Cursor::new(nal)).expect("direct");
        assert_eq!(sps, direct);
        sps
    }

    /// The pre-walk itself refuses the SPS, before the dependency sees it.
    fn refused_by_prefix(nal: &[u8]) {
        let rbsp = crate::scan::h2645_unescape_rbsp(&nal[2..]);
        assert!(sps_prefix(&rbsp).is_none());
        assert!(parse_sps(nal).is_none());
    }

    #[test]
    fn an_sps_whose_transform_block_exceeds_its_coding_tree_block_is_refused() {
        let prefix = sps_prefix(&sps_rbsp()).expect("prefix");
        assert!(prefix.min_transform_log2 > prefix.coding_tree_log2);
        assert!(parse_sps(&SPS).is_none());
        assert_eq!(extract_h265_info(&SPS).bit_depth, None);
    }

    #[test]
    fn a_truncated_sps_is_refused() {
        for len in 0..20 {
            assert!(sps_prefix(&sps_rbsp()[..len]).is_none(), "{len}");
        }
    }

    #[test]
    fn an_hevc_sps_with_or_without_scaling_lists_parses_as_before() {
        let plain = parses_unchanged(&sps_with(None, no_tools));
        assert!(plain.rbsp.scaling_list_data.is_none());

        // Enabled, but with the default lists rather than lists in the SPS.
        let defaults = parses_unchanged(&sps_with(None, |out| {
            out.write_bits(0b10, 2);
            out.write_bits(0, 3);
        }));
        assert!(defaults.rbsp.scaling_list_data.is_none());

        // The first matrix of each size explicit with boundary coefficients,
        // odd ones predicted from it, even ones from the default.
        let lists = parses_unchanged(&scaling_lists(|size_id, matrix_id, out| {
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
        let data = lists.rbsp.scaling_list_data.expect("scaling lists");
        assert_eq!(data.scaling_list[0][0][0], 136);
        assert_eq!(data.scaling_list[1][0][0], 135);
    }

    #[test]
    fn an_hevc_scaling_list_predicted_from_before_the_first_matrix_is_refused() {
        // One matrix too far back at each size. From the second size on, the
        // dependency subtracts the distance from the matrix index unchecked.
        for (size_id, target) in [(0, 2), (1, 0), (2, 5), (3, 3)] {
            let step = if size_id == 3 { 3 } else { 1 };
            refused_by_prefix(&scaling_lists(|size, matrix_id, out| {
                let delta = if (size, matrix_id) == (size_id, target) {
                    target / step + 1
                } else {
                    0
                };
                predicted(out, delta);
            }));
        }
    }

    #[test]
    fn an_hevc_scaling_list_coefficient_out_of_range_is_refused() {
        for (size_id, dc, delta) in [(2, -8, 0), (3, 248, 0), (2, 8, -129), (0, 8, 128)] {
            refused_by_prefix(&scaling_lists(|size, matrix_id, out| {
                if (size, matrix_id) == (size_id, 0) {
                    explicit(out, size, dc, delta);
                } else {
                    predicted(out, 0);
                }
            }));
        }
    }

    #[test]
    fn hevc_pcm_block_sizes_wider_than_32_bits_are_refused() {
        let pcm = |min_minus3: u64, diff: u64| {
            sps_with(None, move |out| {
                out.write_bits(0b0001, 4); // scaling lists, AMP and SAO off; PCM on
                out.write_bits(0x77, 8); // 8-bit PCM samples
                out.write_ue(min_minus3);
                out.write_ue(diff);
                out.write_bits(0, 1); // pcm_loop_filter_disabled_flag
            })
        };
        assert!(parses_unchanged(&pcm(1, 0)).rbsp.pcm.is_some());
        refused_by_prefix(&pcm(1 << 40, 0));
        refused_by_prefix(&pcm(1, 1 << 40));
    }

    #[test]
    fn an_hevc_conformance_window_wider_than_the_picture_has_no_dimensions() {
        let cropped = parses_unchanged(&sps_with(Some([2, 2, 4, 0]), no_tools));
        assert_eq!(sps_dimensions(&cropped.rbsp), Some((56, 56)));
        let too_wide = parses_unchanged(&sps_with(Some([40, 0, 0, 0]), no_tools));
        assert_eq!(sps_dimensions(&too_wide.rbsp), None);
        let too_tall = parses_unchanged(&sps_with(Some([0, 0, 0, 40]), no_tools));
        assert_eq!(sps_dimensions(&too_tall.rbsp), None);
    }

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
}
