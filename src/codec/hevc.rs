//! HEVC decoder configuration (hvcC): profile, bit depth, transfer and NAL
//! length size.

use super::CodecInfo;
use super::bits::BitReader;

/// Bits in one `profile_tier_level` profile entry, without its level.
const PROFILE_BITS: usize = 88;
/// Bits in one `profile_tier_level` level entry.
const LEVEL_BITS: usize = 8;

/// Parses an HEVC SPS NAL unit.
///
/// `scuffle-h265` subtracts the minimum transform block size from the coding
/// tree block size unchecked, so an SPS whose transform block is the larger
/// of the two overflows inside it. Such an SPS is refused here first.
pub(crate) fn parse_sps(nal: &[u8]) -> Option<scuffle_h265::SpsNALUnit> {
    sps_block_sizes(nal).filter(|sizes| sizes.min_transform_log2 <= sizes.coding_tree_log2)?;
    scuffle_h265::SpsNALUnit::parse(std::io::Cursor::new(nal)).ok()
}

struct SpsBlockSizes {
    coding_tree_log2: u64,
    min_transform_log2: u64,
}

/// Reads an SPS as far as its block sizes, following the field order and
/// widths `scuffle-h265` reads so both see the same values.
fn sps_block_sizes(nal: &[u8]) -> Option<SpsBlockSizes> {
    // Emulation prevention starts after the two-byte NAL header.
    let rbsp = crate::scan::h2645_unescape_rbsp(nal.get(2..)?);
    let mut bits = BitReader::new(&rbsp);

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
    Some(SpsBlockSizes {
        coding_tree_log2,
        min_transform_log2,
    })
}

/// Reads one unsigned Exp-Golomb value of at most 32 bits.
fn read_ue(bits: &mut BitReader<'_>) -> Option<u64> {
    let mut leading_zeros = 0;
    while bits.read_bit()? == 0 {
        leading_zeros += 1;
        if leading_zeros > 31 {
            return None;
        }
    }
    let suffix = if leading_zeros == 0 {
        0
    } else {
        u64::from(bits.read_bits(leading_zeros)?)
    };
    Some((1_u64 << leading_zeros) - 1 + suffix)
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

    /// A Main profile SPS: 8x8 minimum coding block, 16x16 coding tree block.
    const SPS: [u8; 40] = [
        0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x03, 0x00, 0x1E, 0xA0, 0x20, 0x81, 0x05, 0x96, 0x56, 0xD4, 0xA4, 0xC2, 0xF0, 0x16, 0x80,
        0x80, 0x00, 0x00, 0x03, 0x00, 0x80, 0x00, 0x00, 0x0C, 0x04,
    ];

    #[test]
    fn an_sps_whose_transform_block_exceeds_its_coding_tree_block_is_refused() {
        let sizes = sps_block_sizes(&SPS).expect("block sizes");
        assert!(sizes.min_transform_log2 > sizes.coding_tree_log2);
        assert!(parse_sps(&SPS).is_none());
        assert_eq!(extract_h265_info(&SPS).bit_depth, None);
    }

    #[test]
    fn a_truncated_sps_is_refused() {
        for len in 0..20 {
            assert!(sps_block_sizes(&SPS[..len]).is_none(), "{len}");
        }
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
