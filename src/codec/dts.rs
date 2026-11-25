//! DTS core and extension substream (EXSS) headers: profile and channel
//! count from sync words, without decoding audio.

use super::ac3::ac3_channel_layout;
use super::bits::{AudioBitReader, BitReader};
use crate::scan;

const DTS_SYNCWORD_CORE_BE: u32 = 0x7FFE_8001;
const DTS_SYNCWORD_CORE_LE: u32 = 0xFE7F_0180;
const DTS_SYNCWORD_CORE_14B_BE: u32 = 0x1FFF_E800;
const DTS_SYNCWORD_CORE_14B_LE: u32 = 0xFF1F_00E8;
const DTS_SYNCWORD_SUBSTREAM: u32 = 0x6458_2025;
const DTS_SYNCWORD_XCH: u32 = 0x5A5A_5A5A;
const DTS_SYNCWORD_XXCH: u32 = 0x4700_4A03;
const DTS_SYNCWORD_X96: u32 = 0x1D95_F262;
const DTS_SYNCWORD_XLL: u32 = 0x41A2_9547;
const DTS_SYNCWORD_XLL_X: u32 = 0x0200_0850;
const DTS_SYNCWORD_XLL_X_IMAX: u32 = 0xF140_00D0;

const DTS_CORE_PROBE_BYTES: usize = 32;
/// The shortest buffer searched for an extension substream.
const DTS_EXSS_MIN_BYTES: usize = 16;
const DTS_XLL_CHSETS_MAX: u32 = 3;
const DTS_XLL_PBR_BUFFER_MAX: usize = 240 << 10;
const DTS_CHANNELS: [u8; 16] = [1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8];
const DTS_SAMPLE_RATES: [u32; 16] = [
    0, 8_000, 16_000, 32_000, 0, 0, 11_025, 22_050, 44_100, 0, 0, 12_000, 24_000, 48_000, 96_000,
    192_000,
];
const DCA_EXSS_CORE: u16 = 0x010;
const DCA_EXSS_XBR: u16 = 0x020;
const DCA_EXSS_XXCH: u16 = 0x040;
const DCA_EXSS_X96: u16 = 0x080;
const DCA_EXSS_LBR: u16 = 0x100;
const DCA_EXSS_XLL: u16 = 0x200;
const DCA_EXSS_RSV1: u16 = 0x400;
const DCA_EXSS_RSV2: u16 = 0x800;

pub(crate) fn detect_dts_channels_from_probe_bytes(prefix: &[u8]) -> Option<i32> {
    let asset = parse_dts_exss_asset(prefix);
    asset.map(|asset| asset.nchannels_total)
}

/// Channel counts read from one buffer at later and later offsets.
/// `channels_at(offset)` gives what [`detect_dts_channels_from_probe_bytes`]
/// and then [`find_dts_header`] give for `&data[offset..]`, but each search
/// resumes where the previous one stopped instead of reading the rest of the
/// buffer again.
pub(crate) struct DtsChannelScan<'a> {
    data: &'a [u8],
    exss: scan::ForwardSearch<i32>,
    core: scan::ForwardSearch<i32>,
}

impl<'a> DtsChannelScan<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            exss: scan::ForwardSearch::new(),
            core: scan::ForwardSearch::new(),
        }
    }

    pub(crate) fn channels_at(&mut self, offset: usize) -> Option<i32> {
        let data = self.data;
        let remaining = data.len().saturating_sub(offset);
        let exss = if remaining < DTS_EXSS_MIN_BYTES {
            None
        } else {
            self.exss.find(offset, |from| {
                find_dts_exss_asset_from(data, from)
                    .map(|asset| (asset.exss_start, asset.nchannels_total))
                    .ok_or(data.len())
            })
        };
        exss.or_else(|| {
            self.core.find(offset, |from| {
                find_dts_header_from(data, from)
                    .map(|(start, header)| (start, i32::from(header.channels)))
            })
        })
    }
}

pub(crate) fn detect_dts_profile_from_codec_id(codec_id: &str) -> Option<String> {
    match codec_id {
        "A_DTS/LOSSLESS" => Some("DTS-HD MA".into()),
        "A_DTS/EXPRESS" => Some("DTS Express".into()),
        id if id == "A_DTS" || id == "A_DTS/LOSSY" || id.starts_with("A_DTS/") => {
            Some("DTS".into())
        }
        _ => None,
    }
}

pub(crate) fn detect_dts_profile_from_probe_bytes(
    prefix: &[u8],
    suffix: Option<&[u8]>,
) -> Option<String> {
    let core = parse_dts_core_header(prefix);
    let has_core = core.is_some();
    let [has_xch, has_xxch, has_x96] = crate::scan::syncword_presence(
        prefix,
        [DTS_SYNCWORD_XCH, DTS_SYNCWORD_XXCH, DTS_SYNCWORD_X96],
    );
    let has_xch = has_xch || has_xxch;
    if let Some(exss) = parse_dts_exss_asset(prefix) {
        let exss_follows_core = core
            .map(|core| exss.exss_start >= core.frame_size)
            .unwrap_or(false);
        if exss_follows_core
            && (exss.extension_mask & DCA_EXSS_XLL) != 0
            && dts_exss_has_valid_xll_sync(prefix, &exss)
        {
            let has_x = dts_exss_remainder_has_syncword(prefix, &exss, DTS_SYNCWORD_XLL_X)
                || suffix.is_some_and(|tail| contains_aligned_syncword(tail, DTS_SYNCWORD_XLL_X));
            let has_x_imax =
                dts_exss_remainder_has_shifted_syncword(prefix, &exss, DTS_SYNCWORD_XLL_X_IMAX)
                    || suffix.is_some_and(|tail| {
                        contains_aligned_syncword_shifted(tail, DTS_SYNCWORD_XLL_X_IMAX)
                    });
            if has_x_imax {
                return Some("DTS-HD MA + DTS:X IMAX".into());
            }
            if has_x {
                return Some("DTS-HD MA + DTS:X".into());
            }
            return Some("DTS-HD MA".into());
        }
        if (exss.extension_mask & DCA_EXSS_XBR) != 0 {
            return Some("DTS-HD HRA".into());
        }
        if (exss.extension_mask & DCA_EXSS_LBR) != 0 {
            return Some("DTS Express".into());
        }
    }
    if has_x96 {
        return Some("DTS 96/24".into());
    }
    if has_xch {
        return Some("DTS-ES".into());
    }
    if has_core {
        return Some("DTS".into());
    }
    None
}

fn contains_aligned_syncword(data: &[u8], syncword: u32) -> bool {
    if data.len() < 4 {
        return false;
    }
    let start = data.len() % 4;
    data[start..]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|chunk| u32::from_be_bytes(*chunk) == syncword)
}

fn contains_aligned_syncword_shifted(data: &[u8], syncword: u32) -> bool {
    if data.len() < 4 {
        return false;
    }
    let start = data.len() % 4;
    data[start..]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|chunk| (u32::from_be_bytes(*chunk) >> 1) == (syncword >> 1))
}

#[derive(Debug, Clone, Copy)]
struct DtsCoreHeaderInfo {
    frame_size: usize,
}

#[derive(Debug, Clone, Copy)]
struct DtsExssAssetInfo {
    exss_start: usize,
    nchannels_total: i32,
    extension_mask: u16,
    asset_size: usize,
    xll_offset: usize,
    xll_size: usize,
    xll_sync_offset: usize,
}

fn parse_dts_core_header(data: &[u8]) -> Option<DtsCoreHeaderInfo> {
    if data.len() < 11 {
        return None;
    }
    for start in 0..=data.len() - 11 {
        let marker = u32::from_be_bytes(data[start..start + 4].try_into().ok()?);
        if !matches!(
            marker,
            DTS_SYNCWORD_CORE_BE
                | DTS_SYNCWORD_CORE_LE
                | DTS_SYNCWORD_CORE_14B_BE
                | DTS_SYNCWORD_CORE_14B_LE
        ) {
            continue;
        }

        let probe_end = (start + DTS_CORE_PROBE_BYTES).min(data.len());
        let normalized = normalize_dts_core_prefix(&data[start..probe_end])?;
        let mut bits = AudioBitReader::new(&normalized);
        if bits.read_bits(32)? != DTS_SYNCWORD_CORE_BE {
            continue;
        }
        bits.read_bit()?;
        let deficit_samples = bits.read_bits(5)? as u8 + 1;
        if deficit_samples != 32 {
            continue;
        }
        bits.skip_bits(1)?;
        let npcmblocks = bits.read_bits(7)? as u8 + 1;
        if (npcmblocks & 0x07) != 0 {
            continue;
        }
        let frame_size = bits.read_bits(14)? + 1;
        if frame_size < 96 {
            continue;
        }
        let audio_mode = bits.read_bits(6)? as usize;
        if audio_mode >= DTS_CHANNELS.len() {
            continue;
        }
        let sample_rate_code = bits.read_bits(4)? as usize;
        if *DTS_SAMPLE_RATES.get(sample_rate_code)? == 0 {
            continue;
        }
        let _bit_rate_code = bits.read_bits(5)? as usize;
        if bits.read_bit()? != 0 {
            continue;
        }
        bits.skip_bits(1 + 1 + 1 + 1 + 3 + 1 + 1)?;
        let lfe_present = bits.read_bits(2)? as u8;
        if lfe_present == 0x3 {
            continue;
        }

        let _channels = DTS_CHANNELS[audio_mode] + u8::from(lfe_present > 0);
        return Some(DtsCoreHeaderInfo {
            frame_size: frame_size as usize,
        });
    }

    None
}

fn normalize_dts_core_prefix(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 {
        return None;
    }

    let marker = u32::from_be_bytes(data[..4].try_into().ok()?);
    match marker {
        DTS_SYNCWORD_CORE_BE => Some(data.to_vec()),
        DTS_SYNCWORD_CORE_LE => {
            let mut normalized = Vec::with_capacity(data.len());
            for chunk in data.chunks(2) {
                if chunk.len() == 2 {
                    normalized.push(chunk[1]);
                    normalized.push(chunk[0]);
                } else {
                    normalized.push(chunk[0]);
                }
            }
            Some(normalized)
        }
        DTS_SYNCWORD_CORE_14B_BE | DTS_SYNCWORD_CORE_14B_LE => {
            let mut normalized = Vec::with_capacity((data.len() * 14) / 16 + 2);
            let mut bit_buffer = 0u32;
            let mut bits_in_buffer = 0usize;

            for chunk in data.as_chunks::<2>().0 {
                let word = if marker == DTS_SYNCWORD_CORE_14B_BE {
                    u16::from_be_bytes([chunk[0], chunk[1]])
                } else {
                    u16::from_le_bytes([chunk[0], chunk[1]])
                } & 0x3FFF;

                bit_buffer = (bit_buffer << 14) | u32::from(word);
                bits_in_buffer += 14;

                while bits_in_buffer >= 8 {
                    bits_in_buffer -= 8;
                    normalized.push(((bit_buffer >> bits_in_buffer) & 0xFF) as u8);
                    bit_buffer &= (1u32 << bits_in_buffer).saturating_sub(1);
                }
            }

            Some(normalized)
        }
        _ => None,
    }
}

fn parse_dts_exss_asset(data: &[u8]) -> Option<DtsExssAssetInfo> {
    if data.len() < DTS_EXSS_MIN_BYTES {
        return None;
    }
    find_dts_exss_asset_from(data, 0)
}

/// The first extension substream asset at or after `from`. What an asset
/// holds depends only on the bytes from its sync word on, so the asset found
/// from `from` is the one found in `&data[from..]`, offset by `from`.
fn find_dts_exss_asset_from(data: &[u8], from: usize) -> Option<DtsExssAssetInfo> {
    for exss_start in from..=data.len().checked_sub(4)? {
        let marker = u32::from_be_bytes(data[exss_start..exss_start + 4].try_into().ok()?);
        if marker != DTS_SYNCWORD_SUBSTREAM {
            continue;
        }
        if let Some(asset) = parse_dts_exss_asset_at(data, exss_start) {
            return Some(asset);
        }
    }

    None
}

fn parse_dts_exss_asset_at(data: &[u8], exss_start: usize) -> Option<DtsExssAssetInfo> {
    let mut bits = AudioBitReader::new(&data[exss_start..]);
    if bits.read_bits(32)? != DTS_SYNCWORD_SUBSTREAM {
        return None;
    }

    bits.skip_bits(8)?;
    let exss_index = bits.read_bits(2)? as usize;
    let wide_hdr = bits.read_bit()? as usize;
    let header_size = bits.read_bits(8 + 4 * wide_hdr)? as usize + 1;
    let exss_size_nbits = 16 + 4 * wide_hdr;
    let _exss_size = bits.read_bits(exss_size_nbits)? as usize + 1;
    let static_fields_present = bits.read_bit()? != 0;

    let (nassets, mix_metadata_enabled, nmixoutconfigs, nmixoutchs) = if static_fields_present {
        bits.skip_bits(2)?;
        bits.skip_bits(3)?;
        if bits.read_bit()? != 0 {
            bits.skip_bits(32 + 4)?;
        }
        let npresents = bits.read_bits(3)? as usize + 1;
        let nassets = bits.read_bits(3)? as usize + 1;
        for _ in 0..npresents {
            let mask = bits.read_bits(exss_index + 1)?;
            bits.skip_bits(mask.count_ones() as usize * 8)?;
        }
        let mix_metadata_enabled = bits.read_bit()? != 0;
        let mut nmixoutchs = [0usize; 4];
        let nmixoutconfigs = if mix_metadata_enabled {
            bits.skip_bits(2)?;
            let spkr_mask_nbits = ((bits.read_bits(2)? as usize) + 1) << 2;
            let nmixoutconfigs = bits.read_bits(2)? as usize + 1;
            for slot in nmixoutchs.iter_mut().take(nmixoutconfigs) {
                *slot = count_dts_channels_for_mask(bits.read_bits(spkr_mask_nbits)?);
            }
            nmixoutconfigs
        } else {
            0
        };
        (nassets, mix_metadata_enabled, nmixoutconfigs, nmixoutchs)
    } else {
        (1, false, 0, [0; 4])
    };

    let mut asset_ranges = Vec::with_capacity(nassets);
    let mut next_asset_offset = header_size;
    for _ in 0..nassets {
        let size = bits.read_bits(exss_size_nbits)? as usize + 1;
        asset_ranges.push((next_asset_offset, size));
        next_asset_offset = next_asset_offset.checked_add(size)?;
    }

    let mut best: Option<DtsExssAssetInfo> = None;
    for (asset_offset, asset_size) in asset_ranges {
        let asset = parse_dts_exss_descriptor(
            &mut bits,
            static_fields_present,
            mix_metadata_enabled,
            nmixoutconfigs,
            &nmixoutchs,
            exss_size_nbits,
            exss_start,
            asset_offset,
            asset_size,
        )?;
        best = Some(match best {
            Some(current) => merge_dts_exss_assets(current, asset),
            None => asset,
        });
    }

    best
}

#[allow(clippy::too_many_arguments)]
fn parse_dts_exss_descriptor(
    bits: &mut AudioBitReader<'_>,
    static_fields_present: bool,
    mix_metadata_enabled: bool,
    nmixoutconfigs: usize,
    nmixoutchs: &[usize; 4],
    exss_size_nbits: usize,
    exss_start: usize,
    asset_offset: usize,
    asset_size: usize,
) -> Option<DtsExssAssetInfo> {
    let descr_size = bits.read_bits(9)? as usize + 1;
    let descr_start = bits.bit_pos;
    bits.skip_bits(3)?;

    let mut nchannels_total = 0usize;
    let mut embedded_stereo = false;
    let mut embedded_6ch = false;
    if static_fields_present {
        if bits.read_bit()? != 0 {
            bits.skip_bits(4)?;
        }
        if bits.read_bit()? != 0 {
            bits.skip_bits(24)?;
        }
        if bits.read_bit()? != 0 {
            let text_size = bits.read_bits(10)? as usize + 1;
            bits.skip_bits(text_size * 8)?;
        }
        bits.skip_bits(5)?;
        bits.skip_bits(4)?;
        nchannels_total = bits.read_bits(8)? as usize + 1;
        let one_to_one_map_ch_to_spkr = bits.read_bit()? != 0;
        if one_to_one_map_ch_to_spkr {
            embedded_stereo = nchannels_total > 2 && bits.read_bit()? != 0;
            embedded_6ch = nchannels_total > 6 && bits.read_bit()? != 0;
            let spkr_mask_enabled = bits.read_bit()? != 0;
            let spkr_mask_nbits = if spkr_mask_enabled {
                ((bits.read_bits(2)? as usize) + 1) << 2
            } else {
                0
            };
            if spkr_mask_enabled {
                bits.skip_bits(spkr_mask_nbits)?;
            }
            let spkr_remap_nsets = bits.read_bits(3)? as usize;
            if spkr_remap_nsets != 0 && !spkr_mask_enabled {
                return None;
            }
            for _ in 0..spkr_remap_nsets {
                let nspeakers = count_dts_channels_for_mask(bits.read_bits(spkr_mask_nbits)?);
                let nch_for_remaps = bits.read_bits(5)? as usize + 1;
                for _ in 0..nspeakers {
                    let remap_mask = bits.read_bits(nch_for_remaps)?;
                    bits.skip_bits(remap_mask.count_ones() as usize * 5)?;
                }
            }
        } else {
            bits.skip_bits(3)?;
        }
    }

    let drc_present = bits.read_bit()? != 0;
    if drc_present {
        bits.skip_bits(8)?;
    }
    if bits.read_bit()? != 0 {
        bits.skip_bits(5)?;
    }
    if drc_present && embedded_stereo {
        bits.skip_bits(8)?;
    }
    if mix_metadata_enabled && bits.read_bit()? != 0 {
        bits.skip_bits(1)?;
        bits.skip_bits(6)?;
        if bits.read_bits(2)? == 3 {
            bits.skip_bits(8)?;
        } else {
            bits.skip_bits(3)?;
        }
        if bits.read_bit()? != 0 {
            for &mixoutch in nmixoutchs.iter().take(nmixoutconfigs) {
                bits.skip_bits(6 * mixoutch)?;
            }
        } else {
            bits.skip_bits(6 * nmixoutconfigs)?;
        }

        let nchannels_dmix =
            nchannels_total + usize::from(embedded_6ch) * 6 + usize::from(embedded_stereo) * 2;
        for &mixoutch in nmixoutchs.iter().take(nmixoutconfigs) {
            if mixoutch == 0 {
                return None;
            }
            for _ in 0..nchannels_dmix {
                let mix_map_mask = bits.read_bits(mixoutch)?;
                bits.skip_bits(mix_map_mask.count_ones() as usize * 6)?;
            }
        }
    }

    let coding_mode = bits.read_bits(2)? as u8;
    let mut extension_mask = 0u16;
    let mut xll_offset = 0usize;
    let mut xll_size = 0usize;
    let mut xll_sync_offset = 0usize;

    match coding_mode {
        0 => {
            extension_mask = bits.read_bits(12)? as u16;
            let mut offset = asset_offset;
            if (extension_mask & DCA_EXSS_CORE) != 0 {
                let core_size = bits.read_bits(14)? as usize + 1;
                if bits.read_bit()? != 0 {
                    bits.skip_bits(2)?;
                }
                offset += core_size;
            }
            if (extension_mask & DCA_EXSS_XBR) != 0 {
                offset += bits.read_bits(14)? as usize + 1;
            }
            if (extension_mask & DCA_EXSS_XXCH) != 0 {
                offset += bits.read_bits(14)? as usize + 1;
            }
            if (extension_mask & DCA_EXSS_X96) != 0 {
                offset += bits.read_bits(12)? as usize + 1;
            }
            if (extension_mask & DCA_EXSS_LBR) != 0 {
                offset += parse_dts_exss_lbr(bits)?;
            }
            if (extension_mask & DCA_EXSS_XLL) != 0 {
                let (sync_offset, size) = parse_dts_exss_xll(bits, exss_size_nbits)?;
                xll_offset = offset;
                xll_size = size;
                xll_sync_offset = sync_offset;
            }
            if (extension_mask & DCA_EXSS_RSV1) != 0 {
                let _ = bits.read_bits(16)? as usize + 1;
            }
            if (extension_mask & DCA_EXSS_RSV2) != 0 {
                let _ = bits.read_bits(16)? as usize + 1;
            }
        }
        1 => {
            extension_mask = DCA_EXSS_XLL;
            let (sync_offset, size) = parse_dts_exss_xll(bits, exss_size_nbits)?;
            xll_offset = asset_offset;
            xll_size = size;
            xll_sync_offset = sync_offset;
        }
        2 => {
            extension_mask = DCA_EXSS_LBR;
            let _ = parse_dts_exss_lbr(bits)?;
        }
        3 => {
            bits.skip_bits(14 + 8)?;
            if bits.read_bit()? != 0 {
                bits.skip_bits(3)?;
            }
        }
        _ => return None,
    }

    if (extension_mask & DCA_EXSS_XLL) != 0 {
        bits.skip_bits(3)?;
    }

    let descr_end = descr_start.checked_add(descr_size * 8)?;
    if bits.bit_pos > descr_end {
        return None;
    }
    bits.skip_bits(descr_end - bits.bit_pos)?;

    Some(DtsExssAssetInfo {
        exss_start,
        nchannels_total: nchannels_total as i32,
        extension_mask,
        asset_size,
        xll_offset,
        xll_size,
        xll_sync_offset,
    })
}

fn parse_dts_exss_lbr(bits: &mut AudioBitReader<'_>) -> Option<usize> {
    let lbr_size = bits.read_bits(14)? as usize + 1;
    if bits.read_bit()? != 0 {
        bits.skip_bits(2)?;
    }
    Some(lbr_size)
}

fn merge_dts_exss_assets(
    current: DtsExssAssetInfo,
    candidate: DtsExssAssetInfo,
) -> DtsExssAssetInfo {
    if dts_extension_rank(candidate.extension_mask) > dts_extension_rank(current.extension_mask) {
        DtsExssAssetInfo {
            nchannels_total: current.nchannels_total.max(candidate.nchannels_total),
            ..candidate
        }
    } else {
        DtsExssAssetInfo {
            nchannels_total: current.nchannels_total.max(candidate.nchannels_total),
            ..current
        }
    }
}

fn dts_extension_rank(extension_mask: u16) -> i32 {
    if (extension_mask & DCA_EXSS_XLL) != 0 {
        3
    } else if (extension_mask & DCA_EXSS_XBR) != 0 {
        2
    } else if (extension_mask & DCA_EXSS_LBR) != 0 {
        1
    } else {
        0
    }
}

fn parse_dts_exss_xll(
    bits: &mut AudioBitReader<'_>,
    exss_size_nbits: usize,
) -> Option<(usize, usize)> {
    let xll_size = bits.read_bits(14)? as usize + 1;
    let xll_sync_offset = if bits.read_bit()? != 0 {
        bits.skip_bits(4)?;
        let xll_delay_nbits = bits.read_bits(5)? as usize + 1;
        bits.skip_bits(xll_delay_nbits)?;
        bits.read_bits(exss_size_nbits)? as usize
    } else {
        0
    };
    Some((xll_sync_offset, xll_size))
}

fn dts_exss_has_valid_xll_sync(prefix: &[u8], info: &DtsExssAssetInfo) -> bool {
    let Some(sync_start) = dts_exss_xll_sync_start(info) else {
        return false;
    };
    let Some(sync_bytes) = prefix.get(sync_start..sync_start + 4) else {
        return false;
    };
    if u32::from_be_bytes(sync_bytes.try_into().expect("sync length")) != DTS_SYNCWORD_XLL {
        return false;
    }

    let available_prefix = prefix.len().saturating_sub(sync_start);
    let available = info.asset_size.min(available_prefix);
    dts_xll_common_header_is_valid(&prefix[sync_start..sync_start + available], info)
}

fn dts_exss_remainder_has_syncword(prefix: &[u8], info: &DtsExssAssetInfo, syncword: u32) -> bool {
    dts_exss_remainder_region(prefix, info)
        .is_some_and(|region| contains_aligned_syncword(region, syncword))
}

fn dts_exss_remainder_has_shifted_syncword(
    prefix: &[u8],
    info: &DtsExssAssetInfo,
    syncword: u32,
) -> bool {
    dts_exss_remainder_region(prefix, info)
        .is_some_and(|region| contains_aligned_syncword_shifted(region, syncword))
}

fn dts_exss_xll_sync_start(info: &DtsExssAssetInfo) -> Option<usize> {
    let start = info
        .exss_start
        .checked_add(info.xll_offset)?
        .checked_add(info.xll_sync_offset)?;
    let end = info
        .exss_start
        .checked_add(info.xll_offset)?
        .checked_add(info.xll_size)?;
    (start.checked_add(4)? <= end).then_some(start)
}

fn dts_xll_common_header_is_valid(data: &[u8], info: &DtsExssAssetInfo) -> bool {
    let mut bits = AudioBitReader::new(data);
    if bits.read_bits(32) != Some(DTS_SYNCWORD_XLL) {
        return false;
    }

    let stream_ver = bits.read_bits(4).map(|value| value + 1);
    if stream_ver != Some(1) {
        return false;
    }

    let Some(header_size) = bits.read_bits(8).map(|value| value as usize + 1) else {
        return false;
    };
    if header_size > info.asset_size {
        return false;
    }
    let Some(frame_size_nbits) = bits.read_bits(5).map(|value| value as usize + 1) else {
        return false;
    };
    let Some(frame_size) = bits
        .read_bits(frame_size_nbits)
        .map(|value| value as usize + 1)
    else {
        return false;
    };

    if frame_size > info.asset_size || frame_size >= DTS_XLL_PBR_BUFFER_MAX {
        return false;
    }

    let Some(nchsets) = bits.read_bits(4).map(|value| value + 1) else {
        return false;
    };
    if nchsets > DTS_XLL_CHSETS_MAX {
        return false;
    }

    let Some(nframesegs_log2) = bits.read_bits(4) else {
        return false;
    };
    let nframesegs = 1u32 << nframesegs_log2;
    if nframesegs > 1024 {
        return false;
    }

    let Some(nsegsamples_log2) = bits.read_bits(4) else {
        return false;
    };
    if nsegsamples_log2 == 0 {
        return false;
    }
    let nsegsamples = 1u32 << nsegsamples_log2;
    if nsegsamples > 512 {
        return false;
    }

    let nframesamples = 1u32 << (nsegsamples_log2 + nframesegs_log2);
    if nframesamples > 65_536 {
        return false;
    }

    let Some(ch_mask_nbits) = bits.read_bits(5).map(|value| value + 1) else {
        return false;
    };
    if ch_mask_nbits == 0 || ch_mask_nbits > 32 {
        return false;
    }

    let Some(scalable_lsbs) = bits.read_bit() else {
        return false;
    };
    if scalable_lsbs != 0 && bits.read_bits(4).is_none() {
        return false;
    }

    let _ = ch_mask_nbits;
    true
}

fn dts_exss_remainder_region<'a>(prefix: &'a [u8], info: &DtsExssAssetInfo) -> Option<&'a [u8]> {
    let start = dts_exss_xll_sync_start(info)?;
    let end = info
        .exss_start
        .checked_add(info.asset_size)?
        .min(prefix.len());
    prefix.get(start..end)
}

fn count_dts_channels_for_mask(mask: u32) -> usize {
    ((mask & 0xFFFF) | ((mask & 0xAE66) << 16)).count_ones() as usize
}

const DTS_HEADER_PROBE_BYTES: usize = 32;
const DTS_BIT_RATES: [u32; 32] = [
    32_000, 56_000, 64_000, 96_000, 112_000, 128_000, 192_000, 224_000, 256_000, 320_000, 384_000,
    448_000, 512_000, 576_000, 640_000, 768_000, 896_000, 1_024_000, 1_152_000, 1_280_000,
    1_344_000, 1_408_000, 1_411_200, 1_472_000, 1_536_000, 1_920_000, 2_048_000, 3_072_000,
    3_840_000, 1, 2, 3,
];

pub(crate) struct DtsHeader {
    pub channels: u8,
    pub sample_rate: u32,
    pub channel_layout: Option<&'static str>,
    pub bit_rate_bps: u32,
}

pub(crate) fn find_dts_header(data: &[u8]) -> Option<DtsHeader> {
    find_dts_header_from(data, 0).ok().map(|(_, header)| header)
}

/// The first DTS core header at or after `from` and its offset, or the
/// offset at which the search gave up. Every start from `from` up to
/// that offset gives the same answer.
pub(crate) fn find_dts_header_from(data: &[u8], from: usize) -> Result<(usize, DtsHeader), usize> {
    if data.len().saturating_sub(from) < 11 {
        return Err(data.len());
    }
    let mut cursor = from;
    while let Some(candidate) = scan::find_audio_sync_candidate(data, cursor) {
        if candidate.kind != scan::AudioSyncKind::Dts {
            cursor = candidate.offset + 1;
            continue;
        }
        let start = candidate.offset;
        if start + 11 > data.len() {
            return Err(start);
        }
        cursor = start + 1;
        match dts_header_at(data, start) {
            Some(Some(header)) => return Ok((start, header)),
            Some(None) => {}
            None => return Err(start),
        }
    }

    Err(data.len())
}

/// The DTS core header whose sync word is at `start`: `Some(None)` when it
/// starts no header, `None` when the header runs past the data.
fn dts_header_at(data: &[u8], start: usize) -> Option<Option<DtsHeader>> {
    let probe_end = (start + DTS_HEADER_PROBE_BYTES).min(data.len());
    let normalized = normalize_dts_core_prefix(&data[start..probe_end])?;
    let mut bits = BitReader::new(&normalized);
    if bits.read_bits(32)? != DTS_SYNCWORD_CORE_BE {
        return Some(None);
    }
    bits.read_bit()?; // normal frame flag
    let deficit_samples = bits.read_bits(5)? as u8 + 1;
    if deficit_samples != 32 {
        return Some(None);
    }
    bits.skip_bits(1)?; // crc present
    let npcmblocks = bits.read_bits(7)? as u8 + 1;
    if (npcmblocks & 0x07) != 0 {
        return Some(None);
    }
    let frame_size = bits.read_bits(14)? + 1;
    if frame_size < 96 {
        return Some(None);
    }
    let audio_mode = bits.read_bits(6)? as usize;
    if audio_mode >= DTS_CHANNELS.len() {
        return Some(None);
    }
    let sample_rate_code = bits.read_bits(4)? as usize;
    if *DTS_SAMPLE_RATES.get(sample_rate_code)? == 0 {
        return Some(None);
    }
    let bit_rate_code = bits.read_bits(5)? as usize;
    if bit_rate_code >= DTS_BIT_RATES.len() {
        return Some(None);
    }
    if bits.read_bit()? != 0 {
        return Some(None);
    }
    bits.skip_bits(1 + 1 + 1 + 1 + 3 + 1 + 1)?; // drc/timestamp/aux/hdcd/ext/syncssf
    let lfe_present = bits.read_bits(2)? as u8;
    if lfe_present == 0x3 {
        return Some(None);
    }

    Some(Some(DtsHeader {
        channels: DTS_CHANNELS[audio_mode] + u8::from(lfe_present > 0),
        sample_rate: DTS_SAMPLE_RATES[sample_rate_code],
        channel_layout: match audio_mode {
            0 => Some(ac3_channel_layout(1, lfe_present > 0)),
            1..=4 => Some(ac3_channel_layout(2, lfe_present > 0)),
            5..=9 => Some(ac3_channel_layout(audio_mode - 2, lfe_present > 0)),
            _ => None,
        },
        bit_rate_bps: DTS_BIT_RATES[bit_rate_code],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::audio_profile::{
        detect_audio_profile_from_payload, detect_audio_profile_from_probe_bytes,
    };

    #[test]
    fn detects_plain_dts_from_core_syncword() {
        let payload = [
            0x7F, 0xFE, 0x80, 0x01, 0x7C, 0x7C, 0x05, 0xF2, 0xB7, 0x00, 0x00,
        ];
        assert_eq!(
            detect_audio_profile_from_payload(Some(&crate::Codec::Dts), &payload).as_deref(),
            Some("DTS")
        );
    }

    #[test]
    fn detects_dts_es_profile_from_xch_syncword() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&DTS_SYNCWORD_CORE_BE.to_be_bytes());
        payload.extend_from_slice(&DTS_SYNCWORD_XCH.to_be_bytes());
        assert_eq!(
            detect_audio_profile_from_payload(Some(&crate::Codec::Dts), &payload).as_deref(),
            Some("DTS-ES")
        );
    }

    #[test]
    fn does_not_promote_bare_xll_syncwords_without_valid_exss() {
        let mut prefix = vec![
            0x7F, 0xFE, 0x80, 0x01, 0x7C, 0x7C, 0x05, 0xF2, 0xB7, 0x00, 0x00,
        ];
        prefix.extend_from_slice(&DTS_SYNCWORD_XLL.to_be_bytes());
        prefix.resize(4096, 0);

        let mut suffix = vec![0; 64];
        suffix[16..20].copy_from_slice(&DTS_SYNCWORD_XLL_X.to_be_bytes());

        assert_eq!(
            detect_audio_profile_from_probe_bytes(Some(&crate::Codec::Dts), &prefix, Some(&suffix))
                .as_deref(),
            Some("DTS")
        );
    }

    /// Buffers of noise with DTS core headers and extension substream sync
    /// words planted in them, from a fixed seed.
    fn planted_dts_buffers() -> Vec<Vec<u8>> {
        let core = [
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x7d, 0xb2, 0x77, 0x00, 0x0d, 0x3b, 0x80, 0x09,
            0xef, 0x7b,
        ];
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        (0..400)
            .map(|_| {
                let mut buffer: Vec<u8> = (0..384).map(|_| next() as u8).collect();
                for _ in 0..next() % 12 {
                    let at = next() as usize % buffer.len();
                    let planted: &[u8] = match next() % 3 {
                        0 => &core,
                        1 => &core[..4],
                        _ => &DTS_SYNCWORD_SUBSTREAM.to_be_bytes(),
                    };
                    let end = (at + planted.len()).min(buffer.len());
                    buffer[at..end].copy_from_slice(&planted[..end - at]);
                }
                buffer
            })
            .collect()
    }

    #[test]
    fn channel_scan_matches_a_fresh_search_from_every_offset() {
        let (mut exss_hits, mut core_hits) = (0, 0);
        for buffer in planted_dts_buffers() {
            let mut scan = DtsChannelScan::new(&buffer);
            for offset in 0..=buffer.len() {
                let exss = detect_dts_channels_from_probe_bytes(&buffer[offset..]);
                let core = find_dts_header(&buffer[offset..]).map(|h| i32::from(h.channels));
                exss_hits += usize::from(exss.is_some());
                core_hits += usize::from(exss.is_none() && core.is_some());
                assert_eq!(scan.channels_at(offset), exss.or(core), "offset {offset}");
            }
            // Skipping offsets, as a pass over sync words does.
            let mut scan = DtsChannelScan::new(&buffer);
            for offset in (0..buffer.len()).step_by(37) {
                let expected = detect_dts_channels_from_probe_bytes(&buffer[offset..])
                    .or_else(|| find_dts_header(&buffer[offset..]).map(|h| i32::from(h.channels)));
                assert_eq!(scan.channels_at(offset), expected, "offset {offset}");
            }
        }
        assert!(exss_hits > 0 && core_hits > 0, "{exss_hits} {core_hits}");
    }

    #[test]
    fn parses_real_dts_hd_ma_core_header_prefix() {
        let data = [
            0x7f, 0xfe, 0x80, 0x01, 0xfc, 0x3c, 0x7d, 0xb2, 0x77, 0x00, 0x0d, 0x3b, 0x80, 0x09,
            0xef, 0x7b,
        ];
        let header = find_dts_header(&data).unwrap();
        assert_eq!(header.channels, 6);
    }

    #[test]
    fn parses_dts_core_header_channels_and_bitrate() {
        let data = [
            0x7F, 0xFE, 0x80, 0x01, 0x7C, 0x7C, 0x05, 0xF2, 0xB7, 0x00, 0x00,
        ];
        let header = find_dts_header(&data).unwrap();
        assert_eq!(header.channels, 6);
        assert_eq!(header.bit_rate_bps, 1_536_000);
        assert_eq!(header.sample_rate, 48_000);
        assert_eq!(header.channel_layout, None);
    }
}
