//! Structured AVC/HEVC signaling read from the sequence parameter sets.
use crate::Codec;
use crate::raw::RawTrack;
use crate::warning;
use crate::{
    ChromaLocation, ColorMetadata, ProbeReport, ProbeWarning, Provenance, Rational, StreamMetadata,
};

fn aspect(id: u8, width: u16, height: u16) -> Option<Rational> {
    let (width, height) = match id {
        1 => (1, 1),
        2 => (12, 11),
        3 => (10, 11),
        4 => (16, 11),
        5 => (40, 33),
        6 => (24, 11),
        7 => (20, 11),
        8 => (32, 11),
        9 => (80, 33),
        10 => (18, 11),
        11 => (15, 11),
        12 => (64, 33),
        13 => (160, 99),
        14 => (4, 3),
        15 => (3, 2),
        16 => (2, 1),
        255 => (width, height),
        _ => return None,
    };
    if width == 0 {
        return None;
    }
    Rational::new(i64::from(width), u64::from(height))
}

fn pixel_format(chroma: u8, depth: i32, matrix: Option<u32>) -> Option<String> {
    if !(8..=16).contains(&depth) {
        return None;
    }
    let family = match chroma {
        0 => "gray",
        1 => "yuv420p",
        2 => "yuv422p",
        3 if matrix == Some(0) => "gbrp",
        3 => "yuv444p",
        _ => return None,
    };
    Some(if depth == 8 {
        family.into()
    } else {
        format!("{family}{depth}le")
    })
}

/// The chroma location an H.264 or HEVC `chroma_sample_loc_type_top_field`
/// names (Rec. ITU-T H.264 E.2.1, H.265 E.3.1). It is meaningful only for
/// 4:2:0 sampling; other sampling leaves the location unstated.
fn chroma_location(chroma_format_idc: u8, loc_type: u64) -> Option<ChromaLocation> {
    if chroma_format_idc != 1 {
        return None;
    }
    Some(match loc_type {
        0 => ChromaLocation::Left,
        1 => ChromaLocation::Center,
        2 => ChromaLocation::TopLeft,
        3 => ChromaLocation::Top,
        4 => ChromaLocation::BottomLeft,
        5 => ChromaLocation::Bottom,
        _ => return None,
    })
}

fn avc(data: &[u8]) -> Option<StreamMetadata> {
    let parse = super::h264::parse_sps;
    let sps = if data.first() == Some(&1) {
        let config = super::h264::AvcConfig::parse(data)?;
        config.sps.iter().take(32).find_map(|nal| parse(nal))?
    } else {
        parse(data)?
    };
    let depth = i32::from(sps.bit_depth_luma_minus8.unwrap_or(0)) + 8;
    let chroma = sps.chroma_format_idc;
    let mut details = StreamMetadata {
        bit_depth: Some(depth),
        level: Some(u32::from(sps.level_idc)),
        field_order: Some(
            if sps.frame_mbs_only_flag {
                "progressive"
            } else {
                "interlaced"
            }
            .into(),
        ),
        sample_aspect_ratio: sps
            .sample_aspect_ratio
            .as_ref()
            .and_then(|sar| aspect(sar.aspect_ratio_idc, sar.sar_width, sar.sar_height)),
        declared_frame_rate: sps.timing.as_ref().and_then(|timing| {
            Rational::new(
                i64::from(timing.time_scale.get()),
                u64::from(timing.num_units_in_tick.get()) * 2,
            )
        }),
        ..Default::default()
    };
    if sps.profile_idc == 66 && sps.constraint_set1_flag {
        details.profile = Some("Constrained Baseline".into());
    }
    if let Some(color) = &sps.color {
        details.color = ColorMetadata {
            primaries: (color.primaries != 2).then_some(u32::from(color.primaries)),
            transfer: (color.transfer != 2).then_some(u32::from(color.transfer)),
            matrix: (color.matrix != 2).then_some(u32::from(color.matrix)),
            full_range: Some(color.full_range),
            provenance: Provenance::Bitstream,
            ..Default::default()
        };
    }
    details.chroma_location = sps
        .chroma_sample_loc_type_top_field
        .and_then(|top| chroma_location(chroma, u64::from(top)));
    details.pixel_format = pixel_format(chroma, depth, details.color.matrix);
    Some(details)
}

fn hevc(data: &[u8]) -> Option<StreamMetadata> {
    let sps = if data.first() == Some(&1) {
        super::hevc::hvcc::parse(data)?
            .sps_nal_units()
            .take(32)
            .find_map(super::hevc::parse_sps)?
    } else {
        super::hevc::parse_sps(data)?
    };
    let depth = i32::from(sps.bit_depth_luma_minus8) + 8;
    let mut details = StreamMetadata {
        bit_depth: Some(depth),
        level: Some(u32::from(sps.level_idc)),
        ..Default::default()
    };
    if sps.progressive_source && !sps.interlaced_source {
        details.field_order = Some("progressive".into());
    }
    if let Some(vui) = &sps.vui {
        if vui.field_seq {
            details.field_order = Some("interlaced".into());
        }
        details.sample_aspect_ratio = aspect(vui.aspect_ratio_idc, vui.sar_width, vui.sar_height);
        details.color = ColorMetadata {
            primaries: (vui.colour_primaries != 2).then_some(u32::from(vui.colour_primaries)),
            transfer: (vui.transfer_characteristics != 2)
                .then_some(u32::from(vui.transfer_characteristics)),
            matrix: (vui.matrix_coeffs != 2).then_some(u32::from(vui.matrix_coeffs)),
            // An absent video signal type reads as limited range with
            // unspecified colour. Do not overwrite an explicit container
            // range with that default.
            full_range: (vui.video_full_range
                || vui.colour_primaries != 2
                || vui.transfer_characteristics != 2
                || vui.matrix_coeffs != 2)
                .then_some(vui.video_full_range),
            provenance: Provenance::Bitstream,
            ..Default::default()
        };
        details.chroma_location = vui
            .chroma_sample_loc_type_top_field
            .and_then(|loc| chroma_location(sps.chroma_format_idc, u64::from(loc)));
        details.declared_frame_rate = vui.timing.and_then(|timing| {
            let ticks = u64::from(timing.num_ticks_poc_diff_one_minus1?) + 1;
            Rational::new(
                i64::from(timing.time_scale),
                u64::from(timing.num_units_in_tick).checked_mul(ticks)?,
            )
        });
    }
    details.pixel_format = pixel_format(sps.chroma_format_idc, depth, details.color.matrix);
    Some(details)
}

pub(crate) fn enrich(track: &mut RawTrack, data: &[u8], report: &mut ProbeReport) {
    let read: fn(&[u8]) -> Option<StreamMetadata> = match track.codec_name {
        Some(Codec::H264) => avc,
        Some(Codec::Hevc) => hevc,
        _ => return,
    };
    if data.len() > 1024 * 1024 {
        report.status = crate::ProbeStatus::Incomplete;
        report.budget_exhausted = true;
        report.warnings.push(ProbeWarning {
            code: warning::SPS_ENRICHMENT_LIMIT.into(),
            message: "SPS enrichment exceeded its bounded configuration size".into(),
            stream_id: track.metadata.id.clone(),
            ..Default::default()
        });
        return;
    }
    let Some(incoming) = read(data) else {
        return;
    };
    let current = &mut track.metadata;
    let conflict = [
        (current.color.primaries, incoming.color.primaries),
        (current.color.transfer, incoming.color.transfer),
        (current.color.matrix, incoming.color.matrix),
    ]
    .iter()
    .any(|(old, new)| old.zip(*new).is_some_and(|(old, new)| old != new))
        || current
            .color
            .full_range
            .zip(incoming.color.full_range)
            .is_some_and(|(old, new)| old != new);
    if conflict
        && !report.warnings.iter().any(|warning| {
            warning.code == warning::COLOR_SIGNALING_CONFLICT && warning.stream_id == current.id
        })
    {
        report.warnings.push(ProbeWarning {
            code: warning::COLOR_SIGNALING_CONFLICT.into(),
            message: "SPS color signaling overrides conflicting container metadata".into(),
            stream_id: current.id.clone(),
            ..Default::default()
        });
    }
    current.color.primaries = incoming.color.primaries.or(current.color.primaries);
    current.color.transfer = incoming.color.transfer.or(current.color.transfer);
    current.color.matrix = incoming.color.matrix.or(current.color.matrix);
    current.color.full_range = incoming.color.full_range.or(current.color.full_range);
    if incoming.color.primaries.is_some()
        || incoming.color.transfer.is_some()
        || incoming.color.matrix.is_some()
        || incoming.color.full_range.is_some()
    {
        current.color.provenance = Provenance::Bitstream;
    }
    current.bit_depth = incoming.bit_depth.or(current.bit_depth);
    current.profile = incoming.profile.or(current.profile.take());
    current.level = incoming.level.or(current.level);
    current.pixel_format = incoming.pixel_format.or(current.pixel_format.take());
    current.chroma_location = incoming.chroma_location.or(current.chroma_location);
    if incoming.field_order.as_deref() != Some("interlaced") || current.field_order.is_none() {
        current.field_order = incoming.field_order.or(current.field_order.take());
    }
    current.declared_frame_rate = incoming.declared_frame_rate.or(current.declared_frame_rate);
    current.sample_aspect_ratio = incoming.sample_aspect_ratio.or(current.sample_aspect_ratio);
    if let (Some(sar), Some(width), Some(height)) =
        (current.sample_aspect_ratio, track.width, track.height)
        && width > 0
        && height > 0
        && sar.numerator > 0
    {
        current.display_aspect_ratio = i64::from(width)
            .checked_mul(sar.numerator)
            .zip((height as u64).checked_mul(sar.denominator))
            .and_then(|(n, d)| Rational::new(n, d));
    }
    track.frame_rate_fps = track
        .frame_rate_fps
        .or(current.declared_frame_rate.and_then(|rate| rate.as_f64()));
    track.color_transfer = current.color.transfer;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aspect_and_pixel_formats_preserve_unspecified_signaling() {
        assert_eq!(aspect(0, 0, 0), None);
        assert_eq!(aspect(255, 0, 1), None);
        assert_eq!(aspect(255, 4, 3), Rational::new(4, 3));
        assert_eq!(aspect(14, 0, 0), Rational::new(4, 3));
        assert_eq!(pixel_format(3, 10, Some(0)).as_deref(), Some("gbrp10le"));
        assert_eq!(pixel_format(2, 10, None).as_deref(), Some("yuv422p10le"));
    }
}
