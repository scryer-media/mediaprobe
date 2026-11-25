//! H.264 decoder configuration (avcC): profile, bit depth and dimensions.

use super::CodecInfo;
use super::bits::{BitReader, EXP_GOLOMB_ZERO_RUN, has_terminated_zero_run, read_se, read_ue};
use crate::scan;

/// Parses an H.264 SPS NAL unit that may hold emulation prevention bytes.
///
/// An SPS with a run of zero bits long enough to overflow the dependency's
/// Exp-Golomb reader is refused first. No field of a conforming SPS is that
/// long a run of zeros. So is one whose scaling matrices would overflow the
/// dependency.
pub(crate) fn parse_sps(nal: &[u8]) -> Option<scuffle_h264::Sps> {
    let rbsp = scan::h2645_unescape_rbsp(nal);
    if has_terminated_zero_run(&rbsp, 0, EXP_GOLOMB_ZERO_RUN) {
        return None;
    }
    check_scaling_matrices(&rbsp)?;
    scuffle_h264::Sps::parse(std::io::Cursor::new(rbsp.as_ref())).ok()
}

/// Reads an SPS as far as its scaling matrices, following the field order
/// `scuffle-h264` reads, and refuses a `delta_scale` outside -128..=127,
/// which the dependency adds to the running scale unchecked. Also returns
/// `None` when the data ends first or a value is wider than 32 bits.
fn check_scaling_matrices(rbsp: &[u8]) -> Option<()> {
    let mut bits = BitReader::new(rbsp);
    bits.skip_bits(8)?; // NAL unit header
    let profile_idc = bits.read_bits(8)?;
    bits.skip_bits(16)?; // constraint flags, reserved_zero_2bits, level_idc
    read_ue(&mut bits)?; // seq_parameter_set_id
    if !matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        return Some(());
    }
    let chroma_format_idc = read_ue(&mut bits)?;
    if chroma_format_idc == 3 {
        bits.skip_bits(1)?; // separate_colour_plane_flag
    }
    read_ue(&mut bits)?; // bit_depth_luma_minus8
    read_ue(&mut bits)?; // bit_depth_chroma_minus8
    bits.skip_bits(1)?; // qpprime_y_zero_transform_bypass_flag
    if bits.read_bit()? == 0 {
        return Some(()); // seq_scaling_matrix_present_flag
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

/// The cropped picture size an SPS declares, or `None` when its fields do
/// not describe one. The parser's own `width` and `height` do this
/// arithmetic unchecked, so a crop larger than the picture, or a huge
/// macroblock count, overflows there.
pub(crate) fn sps_dimensions(sps: &scuffle_h264::Sps) -> Option<(i32, i32)> {
    let crop = sps.frame_crop_info.as_ref();
    let cropped = |units: u64, scale: u64, low: u64, high: u64| {
        let full = units.checked_add(1)?.checked_mul(16)?.checked_mul(scale)?;
        let lost = low.checked_add(high)?.checked_mul(2)?;
        i32::try_from(full.checked_sub(lost)?).ok()
    };
    let field_scale = if sps.mb_adaptive_frame_field_flag.is_none() {
        1
    } else {
        2
    };
    let width = cropped(
        sps.pic_width_in_mbs_minus1,
        1,
        crop.map_or(0, |c| c.frame_crop_left_offset),
        crop.map_or(0, |c| c.frame_crop_right_offset),
    )?;
    let height = cropped(
        sps.pic_height_in_map_units_minus1,
        field_scale,
        crop.map_or(0, |c| c.frame_crop_top_offset),
        crop.map_or(0, |c| c.frame_crop_bottom_offset),
    )?;
    Some((width, height))
}

/// Extracts codec profile and bit depth from an H.264 AVCDecoderConfigurationRecord.
///
/// The input bytes are the raw codec-private data stored in the container
/// (e.g. MKV CodecPrivate or MP4 avcC box contents).
pub(crate) fn is_valid_h264_avcc(codec_private: &[u8]) -> bool {
    use bytes::Bytes;
    use scuffle_h264::AVCDecoderConfigurationRecord;
    use std::io;

    let Ok(config) = AVCDecoderConfigurationRecord::parse(&mut io::Cursor::new(
        Bytes::copy_from_slice(codec_private),
    )) else {
        return false;
    };
    if config.configuration_version != 1 || config.sps.is_empty() || config.pps.is_empty() {
        return false;
    }
    config
        .sps
        .iter()
        .any(|sps_bytes| parse_sps(sps_bytes).is_some())
}

pub(crate) fn extract_h264_info(codec_private: &[u8]) -> CodecInfo {
    use bytes::Bytes;
    use scuffle_h264::{AVCDecoderConfigurationRecord, Sps};
    use std::io;

    let data = Bytes::from(codec_private.to_vec());
    if let Ok(config) = AVCDecoderConfigurationRecord::parse(&mut io::Cursor::new(data)) {
        let profile = map_h264_profile(config.profile_indication);

        // Bit depth: prefer the extended_config field (from the AVCC record itself),
        // fall back to parsing the first SPS NAL unit.
        let bit_depth = if let Some(ref ext) = config.extended_config {
            Some(ext.bit_depth_luma_minus8 as i32 + 8)
        } else {
            config.sps.first().and_then(|sps_bytes| {
                parse_sps(sps_bytes)
                    .and_then(|sps| sps.ext.map(|ext| ext.bit_depth_luma_minus8 as i32 + 8))
            })
        };

        let first_sps = config
            .sps
            .first()
            .and_then(|sps_bytes| parse_sps(sps_bytes));
        let color_transfer = first_sps.as_ref().and_then(|sps| {
            let transfer = sps.color_config.as_ref()?.transfer_characteristics as u32;
            if transfer > 0 && transfer != 2 {
                Some(transfer)
            } else {
                None
            }
        });

        return CodecInfo {
            profile,
            bit_depth: bit_depth.or(Some(8)),
            color_transfer,
            width: first_sps
                .as_ref()
                .and_then(sps_dimensions)
                .map(|size| size.0),
            height: first_sps
                .as_ref()
                .and_then(sps_dimensions)
                .map(|size| size.1),
            frame_rate_fps: first_sps.as_ref().and_then(Sps::frame_rate),
        };
    }

    let Some(sps) = parse_sps(codec_private) else {
        return CodecInfo::default();
    };
    let bit_depth = sps
        .ext
        .as_ref()
        .map(|ext| ext.bit_depth_luma_minus8 as i32 + 8)
        .or(Some(8));
    let color_transfer = sps.color_config.as_ref().and_then(|color| {
        let transfer = color.transfer_characteristics as u32;
        if transfer > 0 && transfer != 2 {
            Some(transfer)
        } else {
            None
        }
    });

    CodecInfo {
        profile: map_h264_profile(sps.profile_idc),
        bit_depth,
        color_transfer,
        width: sps_dimensions(&sps).map(|size| size.0),
        height: sps_dimensions(&sps).map(|size| size.1),
        frame_rate_fps: sps.frame_rate(),
    }
}

/// Maps an H.264 profile_idc value to a human-readable profile name.
fn map_h264_profile(profile_idc: u8) -> Option<String> {
    let name = match profile_idc {
        66 => "Baseline",
        77 => "Main",
        88 => "Extended",
        100 => "High",
        110 => "High 10",
        122 => "High 4:2:2",
        244 => "High 4:4:4 Predictive",
        44 => "CAVLC 4:4:4 Intra",
        83 => "Scalable Baseline",
        86 => "Scalable High",
        118 => "Multiview High",
        128 => "Stereo High",
        _ => return None,
    };
    Some(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::bits::{TestBitWriter, escape_rbsp};

    /// A baseline SPS whose first Exp-Golomb value has a 64-bit zero prefix.
    const SPS_WITH_LONG_ZERO_PREFIX: [u8; 22] = [
        0x67, 0x42, 0x00, 0x0A, 0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF,
    ];

    #[test]
    fn an_sps_with_an_overlong_exp_golomb_prefix_is_refused() {
        assert!(parse_sps(&SPS_WITH_LONG_ZERO_PREFIX).is_none());
        // Seven zero bits fewer in the prefix, and no zero run in the value.
        let mut shorter = SPS_WITH_LONG_ZERO_PREFIX;
        shorter[4] = 0x01;
        shorter[13..].fill(0xFF);
        let rbsp = scan::h2645_unescape_rbsp(&shorter);
        assert!(!has_terminated_zero_run(&rbsp, 0, EXP_GOLOMB_ZERO_RUN));
    }

    /// A High profile 64x48 SPS whose first scaling list holds `deltas`.
    fn high_sps_with_scaling_list(deltas: &[i64]) -> Vec<u8> {
        let mut out = TestBitWriter::new();
        out.write_bits(0x67, 8); // NAL unit header
        out.write_bits(100, 8); // profile_idc
        out.write_bits(0, 8); // constraint flags
        out.write_bits(30, 8); // level_idc
        out.write_ue(0); // seq_parameter_set_id
        out.write_ue(1); // 4:2:0
        out.write_ue(0); // bit_depth_luma_minus8
        out.write_ue(0); // bit_depth_chroma_minus8
        out.write_bits(0b011, 3); // no bypass; scaling matrix; first list
        for &delta in deltas {
            out.write_se(delta);
        }
        out.write_bits(0, 7); // the other lists absent
        out.write_ue(0); // log2_max_frame_num_minus4
        out.write_ue(2); // pic_order_cnt_type
        out.write_ue(1); // max_num_ref_frames
        out.write_bits(0, 1); // gaps_in_frame_num_value_allowed_flag
        out.write_ue(3); // pic_width_in_mbs_minus1
        out.write_ue(2); // pic_height_in_map_units_minus1
        out.write_bits(0b110, 3); // frame_mbs_only, direct_8x8, no cropping
        out.write_bits(0b01, 2); // no VUI, rbsp_stop_one_bit
        escape_rbsp(&out.finish())
    }

    #[test]
    fn an_h264_sps_with_scaling_matrices_parses_as_before() {
        let mut boundary = vec![127, -128];
        boundary.resize(16, 0);
        // A list that ends early when its scale reaches zero.
        for deltas in [boundary, vec![-8]] {
            let nal = high_sps_with_scaling_list(&deltas);
            let sps = parse_sps(&nal).expect("SPS parses");
            let rbsp = scan::h2645_unescape_rbsp(&nal);
            let direct = scuffle_h264::Sps::parse(std::io::Cursor::new(rbsp.as_ref()));
            assert_eq!(sps, direct.expect("direct"));
            assert_eq!(
                sps.ext.as_ref().expect("extension").scaling_matrix[0],
                deltas
            );
            assert_eq!(sps_dimensions(&sps), Some((64, 48)));
        }
    }

    #[test]
    fn an_h264_scaling_matrix_delta_out_of_range_is_refused() {
        // The largest delta has a 63-bit zero prefix, short of the zero run
        // refused before, and overflows the dependency's running scale.
        for delta in [128, -129, i64::MAX, i64::MIN + 1] {
            let nal = high_sps_with_scaling_list(&[delta]);
            let rbsp = scan::h2645_unescape_rbsp(&nal);
            assert!(check_scaling_matrices(&rbsp).is_none(), "{delta}");
            assert!(parse_sps(&nal).is_none(), "{delta}");
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
}
