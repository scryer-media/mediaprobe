//! H.264 decoder configuration (avcC): profile, bit depth and dimensions.

mod avcc;
mod sps;
#[cfg(test)]
mod tests;

pub(crate) use avcc::AvcConfig;
pub(crate) use sps::Sps;

use super::CodecInfo;
use crate::scan;

/// Parses an H.264 SPS NAL unit that may hold emulation prevention bytes.
pub(crate) fn parse_sps(nal: &[u8]) -> Option<Sps> {
    Sps::parse(&scan::h2645_unescape_rbsp(nal))
}

/// The cropped picture size an SPS declares, or `None` when its fields do
/// not describe one: a crop larger than the picture, or a size beyond `i32`.
///
/// Crop offsets count in chroma samples horizontally, and in chroma rows of
/// a frame or of a field vertically, so their unit depends on the chroma
/// format and on whether the stream codes fields.
pub(crate) fn sps_dimensions(sps: &Sps) -> Option<(i32, i32)> {
    let crop = sps.frame_crop.as_ref();
    let cropped = |units: u32, scale: u64, crop_unit: u64, low: u32, high: u32| {
        let full = u64::from(units)
            .checked_add(1)?
            .checked_mul(16)?
            .checked_mul(scale)?;
        let lost = u64::from(low)
            .checked_add(u64::from(high))?
            .checked_mul(crop_unit)?;
        i32::try_from(full.checked_sub(lost)?).ok()
    };
    let field_scale = if sps.frame_mbs_only_flag { 1 } else { 2 };
    // 4:2:0 and 4:2:2 halve the chroma width; only 4:2:0 halves its height.
    let crop_unit_x = if matches!(sps.chroma_format_idc, 1 | 2) {
        2
    } else {
        1
    };
    let crop_unit_y = if sps.chroma_format_idc == 1 { 2 } else { 1 } * field_scale;
    let width = cropped(
        sps.pic_width_in_mbs_minus1,
        1,
        crop_unit_x,
        crop.map_or(0, |c| c.left),
        crop.map_or(0, |c| c.right),
    )?;
    let height = cropped(
        sps.pic_height_in_map_units_minus1,
        field_scale,
        crop_unit_y,
        crop.map_or(0, |c| c.top),
        crop.map_or(0, |c| c.bottom),
    )?;
    Some((width, height))
}

/// Whether `codec_private` is an avcC record with at least one SPS that
/// parses and at least one PPS.
pub(crate) fn is_valid_h264_avcc(codec_private: &[u8]) -> bool {
    let Some(config) = AvcConfig::parse(codec_private) else {
        return false;
    };
    if config.configuration_version != 1 || config.sps.is_empty() || config.pps_count == 0 {
        return false;
    }
    config
        .sps
        .iter()
        .any(|sps_bytes| parse_sps(sps_bytes).is_some())
}

/// A transfer characteristic worth reporting: not 0 (reserved) or 2
/// (unspecified).
fn reported_transfer(sps: &Sps) -> Option<u32> {
    let transfer = u32::from(sps.color.as_ref()?.transfer);
    (transfer > 0 && transfer != 2).then_some(transfer)
}

/// Extracts codec profile and bit depth from an H.264 AVCDecoderConfigurationRecord.
///
/// The input bytes are the raw codec-private data stored in the container
/// (e.g. MKV CodecPrivate or MP4 avcC box contents), or a bare SPS.
pub(crate) fn extract_h264_info(codec_private: &[u8]) -> CodecInfo {
    if let Some(config) = AvcConfig::parse(codec_private) {
        let profile = map_h264_profile(config.profile_indication);
        let first_sps = config
            .sps
            .first()
            .and_then(|sps_bytes| parse_sps(sps_bytes));

        // Bit depth: prefer the extension of the avcC record itself, fall
        // back to the first SPS.
        let bit_depth = config
            .bit_depth_luma_minus8
            .or_else(|| first_sps.as_ref()?.bit_depth_luma_minus8)
            .map(|depth| i32::from(depth) + 8);
        let size = first_sps.as_ref().and_then(sps_dimensions);

        return CodecInfo {
            profile,
            bit_depth: bit_depth.or(Some(8)),
            color_transfer: first_sps.as_ref().and_then(reported_transfer),
            width: size.map(|size| size.0),
            height: size.map(|size| size.1),
            frame_rate_fps: first_sps.as_ref().and_then(Sps::frame_rate),
        };
    }

    let Some(sps) = parse_sps(codec_private) else {
        return CodecInfo::default();
    };
    let size = sps_dimensions(&sps);
    CodecInfo {
        profile: map_h264_profile(sps.profile_idc),
        bit_depth: Some(
            sps.bit_depth_luma_minus8
                .map_or(8, |depth| i32::from(depth) + 8),
        ),
        color_transfer: reported_transfer(&sps),
        width: size.map(|size| size.0),
        height: size.map(|size| size.1),
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
