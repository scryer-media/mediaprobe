//! HEVC decoder configuration (hvcC): profile, bit depth, transfer and NAL
//! length size.

use super::CodecInfo;

pub(crate) mod hvcc;
pub(crate) mod sps;

pub(crate) use sps::Sps;

/// Parses an HEVC SPS NAL unit, header included.
pub(crate) fn parse_sps(nal: &[u8]) -> Option<Sps> {
    sps::parse(nal)
}

/// The cropped picture size an SPS declares, or `None` when its conformance
/// window does not fit in the picture.
pub(crate) fn sps_dimensions(sps: &Sps) -> Option<(i32, i32)> {
    let window = &sps.conformance_window;
    let cropped = |full: u32, scale: u32, low: u32, high: u32| {
        let lost = u64::from(low)
            .checked_add(u64::from(high))?
            .checked_mul(u64::from(scale))?;
        i32::try_from(u64::from(full).checked_sub(lost)?).ok()
    };
    let width = cropped(sps.width, sps.sub_width_c(), window.left, window.right)?;
    let height = cropped(sps.height, sps.sub_height_c(), window.top, window.bottom)?;
    Some((width, height))
}

/// The transfer characteristics an SPS's VUI states, unless unspecified.
fn sps_transfer(sps: &Sps) -> Option<u32> {
    let transfer = sps.vui.as_ref()?.transfer_characteristics;
    (transfer > 0 && transfer != 2).then_some(u32::from(transfer))
}

/// Extracts codec profile, bit depth, and color transfer characteristics from
/// an HEVC (H.265) decoder configuration record.
pub(crate) fn extract_h265_info(codec_private: &[u8]) -> CodecInfo {
    if let Some(config) = hvcc::parse(codec_private) {
        // Extract transfer_characteristics from the first SPS VUI.
        let color_transfer = config
            .sps_nal_units()
            .find_map(|nal| sps_transfer(&parse_sps(nal)?));
        return CodecInfo {
            profile: map_h265_profile(config.general_profile_idc),
            bit_depth: Some(i32::from(config.bit_depth_luma_minus8) + 8),
            color_transfer,
            ..CodecInfo::default()
        };
    }

    let Some(sps) = parse_sps(codec_private) else {
        return CodecInfo::default();
    };
    CodecInfo {
        profile: map_h265_profile(sps.profile_idc),
        bit_depth: Some(i32::from(sps.bit_depth_luma_minus8) + 8),
        color_transfer: sps_transfer(&sps),
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
mod tests;
