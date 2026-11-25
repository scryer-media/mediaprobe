//! H.264 decoder configuration (avcC): profile, bit depth and dimensions.

use super::CodecInfo;
use super::bits::{EXP_GOLOMB_ZERO_RUN, has_terminated_zero_run};
use crate::scan;

/// Parses an H.264 SPS NAL unit that may hold emulation prevention bytes.
///
/// An SPS with a run of zero bits long enough to overflow the dependency's
/// Exp-Golomb reader is refused first. No field of a conforming SPS is that
/// long a run of zeros.
pub(crate) fn parse_sps(nal: &[u8]) -> Option<scuffle_h264::Sps> {
    let rbsp = scan::h2645_unescape_rbsp(nal);
    if has_terminated_zero_run(&rbsp, 0, EXP_GOLOMB_ZERO_RUN) {
        return None;
    }
    scuffle_h264::Sps::parse(std::io::Cursor::new(rbsp.as_ref())).ok()
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

    #[test]
    fn h264_profile_mapping() {
        assert_eq!(map_h264_profile(66).as_deref(), Some("Baseline"));
        assert_eq!(map_h264_profile(77).as_deref(), Some("Main"));
        assert_eq!(map_h264_profile(100).as_deref(), Some("High"));
        assert_eq!(map_h264_profile(110).as_deref(), Some("High 10"));
        assert_eq!(map_h264_profile(0), None);
    }
}
