//! H.264 sequence parameter set reader (Rec. ITU-T H.264 7.3.2.1.1 and
//! E.1.1), read as far as the VUI timing information.
//!
//! Exp-Golomb values are at most 32 bits, the range the specification
//! allows, so every value fits a `u32` and no arithmetic on one overflows.

use std::num::NonZeroU32;

use crate::codec::bits::{BitReader, read_se, read_ue};

/// The fields of a sequence parameter set the crate reads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sps {
    pub profile_idc: u8,
    /// Always false for the profiles that require it to be zero
    /// (44, 100, 110, 122 and 244).
    pub constraint_set1_flag: bool,
    pub level_idc: u8,
    /// 1 (4:2:0) when the profile carries no chroma format.
    pub chroma_format_idc: u8,
    /// `None` when the profile carries no bit depth, which is then 8.
    pub bit_depth_luma_minus8: Option<u8>,
    pub pic_width_in_mbs_minus1: u32,
    pub pic_height_in_map_units_minus1: u32,
    pub frame_mbs_only_flag: bool,
    pub frame_crop: Option<FrameCrop>,
    pub sample_aspect_ratio: Option<SampleAspectRatio>,
    pub color: Option<ColorConfig>,
    pub chroma_sample_loc_type_top_field: Option<u8>,
    pub timing: Option<Timing>,
}

/// Frame cropping offsets, in chroma sample units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FrameCrop {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

/// `aspect_ratio_idc`, with `sar_width` and `sar_height` read only for 255
/// (extended SAR) and zero otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SampleAspectRatio {
    pub aspect_ratio_idc: u8,
    pub sar_width: u16,
    pub sar_height: u16,
}

/// Video signal type. The three code points are 2 (unspecified) when the
/// colour description is absent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ColorConfig {
    pub full_range: bool,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
}

/// VUI timing information; both values are nonzero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Timing {
    pub num_units_in_tick: NonZeroU32,
    pub time_scale: NonZeroU32,
}

/// Chroma format and bit depth from the high-profile part of an SPS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpsExtension {
    pub chroma_format_idc: u8,
    pub bit_depth_luma_minus8: u8,
}

/// The profiles whose SPS carries chroma format, bit depth and scaling
/// matrices.
fn has_extension(profile_idc: u8) -> bool {
    matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    )
}

fn read_u8(bits: &mut BitReader<'_>) -> Option<u8> {
    u8::try_from(bits.read_bits(8)?).ok()
}

fn read_u16(bits: &mut BitReader<'_>) -> Option<u16> {
    u16::try_from(bits.read_bits(16)?).ok()
}

/// Reads an unsigned Exp-Golomb value no greater than `max`.
fn read_ue_max(bits: &mut BitReader<'_>, max: u32) -> Option<u32> {
    u32::try_from(read_ue(bits)?)
        .ok()
        .filter(|value| *value <= max)
}

/// Reads an unsigned Exp-Golomb value of any size the reader allows.
fn read_ue_u32(bits: &mut BitReader<'_>) -> Option<u32> {
    read_ue_max(bits, u32::MAX)
}

impl Sps {
    /// Parses an SPS NAL unit, header included, from its RBSP (emulation
    /// prevention bytes removed). Returns `None` when the data ends before
    /// the last field read, or a field is out of its range.
    pub(crate) fn parse(rbsp: &[u8]) -> Option<Self> {
        let mut bits = BitReader::new(rbsp);
        if bits.read_bit()? != 0 {
            return None; // forbidden_zero_bit
        }
        bits.skip_bits(2)?; // nal_ref_idc
        if bits.read_bits(5)? != 7 {
            return None; // nal_unit_type
        }
        let profile_idc = read_u8(&mut bits)?;
        let constraint_flags = read_u8(&mut bits)?;
        let constraint_set1_flag =
            !matches!(profile_idc, 44 | 100 | 110 | 122 | 244) && constraint_flags & 0x40 != 0;
        let level_idc = read_u8(&mut bits)?;
        read_ue_max(&mut bits, 31)?; // seq_parameter_set_id

        let extension = if has_extension(profile_idc) {
            Some(read_extension(&mut bits)?)
        } else {
            None
        };
        let chroma_format_idc = extension.map_or(1, |ext| ext.chroma_format_idc);

        read_ue_max(&mut bits, 12)?; // log2_max_frame_num_minus4
        match read_ue_max(&mut bits, 2)? {
            0 => {
                read_ue_max(&mut bits, 12)?; // log2_max_pic_order_cnt_lsb_minus4
            }
            1 => skip_pic_order_cnt_type1(&mut bits)?,
            _ => {}
        }
        read_ue(&mut bits)?; // max_num_ref_frames
        bits.skip_bits(1)?; // gaps_in_frame_num_value_allowed_flag
        let pic_width_in_mbs_minus1 = read_ue_u32(&mut bits)?;
        let pic_height_in_map_units_minus1 = read_ue_u32(&mut bits)?;
        let frame_mbs_only_flag = bits.read_bit()? == 1;
        if !frame_mbs_only_flag {
            bits.skip_bits(1)?; // mb_adaptive_frame_field_flag
        }
        bits.skip_bits(1)?; // direct_8x8_inference_flag
        let frame_crop = if bits.read_bit()? == 1 {
            Some(FrameCrop {
                left: read_ue_u32(&mut bits)?,
                right: read_ue_u32(&mut bits)?,
                top: read_ue_u32(&mut bits)?,
                bottom: read_ue_u32(&mut bits)?,
            })
        } else {
            None
        };

        let mut sps = Sps {
            profile_idc,
            constraint_set1_flag,
            level_idc,
            chroma_format_idc,
            bit_depth_luma_minus8: extension.map(|ext| ext.bit_depth_luma_minus8),
            pic_width_in_mbs_minus1,
            pic_height_in_map_units_minus1,
            frame_mbs_only_flag,
            frame_crop,
            sample_aspect_ratio: None,
            color: None,
            chroma_sample_loc_type_top_field: None,
            timing: None,
        };
        if bits.read_bit()? == 1 {
            sps.read_vui(&mut bits)?;
        }
        Some(sps)
    }

    /// Reads the VUI parameters up to and including the timing information.
    fn read_vui(&mut self, bits: &mut BitReader<'_>) -> Option<()> {
        if bits.read_bit()? == 1 {
            let aspect_ratio_idc = read_u8(bits)?;
            let (sar_width, sar_height) = if aspect_ratio_idc == 255 {
                (read_u16(bits)?, read_u16(bits)?)
            } else {
                (0, 0)
            };
            self.sample_aspect_ratio = Some(SampleAspectRatio {
                aspect_ratio_idc,
                sar_width,
                sar_height,
            });
        }
        if bits.read_bit()? == 1 {
            bits.skip_bits(1)?; // overscan_appropriate_flag
        }
        if bits.read_bit()? == 1 {
            bits.skip_bits(3)?; // video_format
            let full_range = bits.read_bit()? == 1;
            let (primaries, transfer, matrix) = if bits.read_bit()? == 1 {
                (read_u8(bits)?, read_u8(bits)?, read_u8(bits)?)
            } else {
                (2, 2, 2)
            };
            self.color = Some(ColorConfig {
                full_range,
                primaries,
                transfer,
                matrix,
            });
        }
        if bits.read_bit()? == 1 {
            // Chroma location is defined for 4:2:0 only.
            if self.chroma_format_idc != 1 {
                return None;
            }
            let top = read_ue_max(bits, 5)?;
            read_ue_max(bits, 5)?; // chroma_sample_loc_type_bottom_field
            self.chroma_sample_loc_type_top_field = u8::try_from(top).ok();
        }
        if bits.read_bit()? == 1 {
            self.timing = Some(Timing {
                num_units_in_tick: NonZeroU32::new(bits.read_bits(32)?)?,
                time_scale: NonZeroU32::new(bits.read_bits(32)?)?,
            });
        }
        Some(())
    }

    /// `time_scale / (2 * num_units_in_tick)`, when timing is present.
    pub(crate) fn frame_rate(&self) -> Option<f64> {
        self.timing.map(|timing| {
            f64::from(timing.time_scale.get()) / (2.0 * f64::from(timing.num_units_in_tick.get()))
        })
    }
}

/// Reads the chroma format, bit depths and scaling matrices of a
/// high-profile SPS. The scaling lists are walked and not kept.
pub(crate) fn read_extension(bits: &mut BitReader<'_>) -> Option<SpsExtension> {
    let chroma_format_idc = u8::try_from(read_ue_max(bits, 3)?).ok()?;
    if chroma_format_idc == 3 {
        bits.skip_bits(1)?; // separate_colour_plane_flag
    }
    let bit_depth_luma_minus8 = u8::try_from(read_ue_max(bits, 6)?).ok()?;
    read_ue_max(bits, 6)?; // bit_depth_chroma_minus8
    bits.skip_bits(1)?; // qpprime_y_zero_transform_bypass_flag
    if bits.read_bit()? == 1 {
        let lists = if chroma_format_idc == 3 { 12 } else { 8 };
        for list in 0..lists {
            if bits.read_bit()? == 1 {
                skip_scaling_list(bits, if list < 6 { 16 } else { 64 })?;
            }
        }
    }
    Some(SpsExtension {
        chroma_format_idc,
        bit_depth_luma_minus8,
    })
}

/// Walks one `scaling_list()` of `size` entries. A `delta_scale` outside
/// -128..=127 is refused.
fn skip_scaling_list(bits: &mut BitReader<'_>, size: usize) -> Option<()> {
    let mut next_scale: i64 = 8;
    for _ in 0..size {
        let delta_scale = read_se(bits)?;
        if !(-128..=127).contains(&delta_scale) {
            return None;
        }
        // Both terms are bounded: next_scale is 0..=255.
        next_scale = (next_scale + delta_scale + 256) % 256;
        if next_scale == 0 {
            break;
        }
    }
    Some(())
}

/// Walks the fields of picture order count type 1. Each offset is a signed
/// value of at most 32 bits, within the range the specification allows.
fn skip_pic_order_cnt_type1(bits: &mut BitReader<'_>) -> Option<()> {
    bits.skip_bits(1)?; // delta_pic_order_always_zero_flag
    read_se(bits)?; // offset_for_non_ref_pic
    read_se(bits)?; // offset_for_top_to_bottom_field
    let cycle = read_ue_max(bits, 255)?;
    for _ in 0..cycle {
        read_se(bits)?; // offset_for_ref_frame
    }
    Some(())
}
