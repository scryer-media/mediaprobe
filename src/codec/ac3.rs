//! AC-3 and E-AC-3 sync frame headers: channels, sample rate and bit rate.

use super::bits::BitReader;
use crate::raw::RawTrack;
use crate::scan;

const AC3_CHANNELS_BY_ACMOD: [u8; 8] = [2, 1, 2, 3, 3, 4, 4, 5];
const AC3_SAMPLE_RATES: [u32; 4] = [48_000, 44_100, 32_000, 0];
const AC3_BITRATES_KBPS: [u32; 19] = [
    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
];
const EAC3_BLOCKS: [u32; 4] = [1, 2, 3, 6];

#[derive(Debug, Clone, Copy)]
pub(crate) struct Ac3Header {
    pub channels: u8,
    pub sample_rate: u32,
    pub channel_layout: &'static str,
    pub bit_rate_bps: Option<u32>,
    pub bitrate_is_estimate: bool,
}

impl Ac3Header {
    pub(crate) fn apply(self, track: &mut RawTrack) {
        // An independent core does not describe additional dependent channels.
        if track
            .channels
            .is_none_or(|channels| channels <= i32::from(self.channels))
        {
            track.channels = Some(i32::from(self.channels));
            track.metadata.channel_layout = Some(self.channel_layout.into());
        }
        track.metadata.sample_rate = Some(self.sample_rate);
        if self.bitrate_is_estimate {
            track.metadata.estimated_bitrate_bps = self.bit_rate_bps.map(u64::from);
        } else if track.bit_rate_bps.is_none()
            || track.metadata.bitrate_provenance == crate::Provenance::Observed
        {
            track.bit_rate_bps = self.bit_rate_bps.map(i64::from);
            track.metadata.bitrate_provenance = crate::Provenance::Bitstream;
        }
    }
}

pub(crate) fn ac3_channel_layout(acmod: usize, lfe: bool) -> &'static str {
    match (acmod, lfe) {
        (0 | 2, false) => "stereo",
        (0 | 2, true) => "2.1",
        (1, false) => "mono",
        (1, true) => "FC+LFE",
        (3, false) => "3.0",
        (3, true) => "3.1",
        (4, false) => "3.0(back)",
        (4, true) => "FL+FR+LFE+BC",
        (5, false) => "4.0",
        (5, true) => "4.1",
        (6, false) => "quad(side)",
        (6, true) => "FL+FR+LFE+SL+SR",
        (7, false) => "5.0(side)",
        (7, true) => "5.1(side)",
        _ => unreachable!("AC-3 channel mode is a three-bit value"),
    }
}

pub(crate) fn find_ac3_header(data: &[u8]) -> Option<Ac3Header> {
    find_ac3_header_from(data, 0).ok().map(|(_, header)| header)
}

/// The first AC-3 header at or after `from` and its offset, or the
/// offset at which the search gave up. Every start from `from` up to
/// that offset gives the same answer.
pub(crate) fn find_ac3_header_from(data: &[u8], from: usize) -> Result<(usize, Ac3Header), usize> {
    if data.len().saturating_sub(from) < 7 {
        return Err(data.len());
    }
    let mut cursor = from;
    while let Some(candidate) = scan::find_audio_sync_candidate(data, cursor) {
        if candidate.kind != scan::AudioSyncKind::Ac3 {
            cursor = candidate.offset + 1;
            continue;
        }
        let start = candidate.offset;
        if start + 7 > data.len() {
            return Err(start);
        }
        cursor = start + 1;
        match ac3_header_at(data, start) {
            Some(Some(header)) => return Ok((start, header)),
            Some(None) => {}
            None => return Err(start),
        }
    }

    Err(data.len())
}

/// The AC-3 header whose sync word is at `start`: `Some(None)` when it
/// starts no header, `None` when the header runs past the data.
fn ac3_header_at(data: &[u8], start: usize) -> Option<Option<Ac3Header>> {
    let bsid = data[start + 5] >> 3;
    if bsid > 10 {
        return Some(None);
    }

    let fscod = (data[start + 4] >> 6) as usize;
    let frame_size_code = (data[start + 4] & 0x3F) as usize;
    if fscod == 3 || frame_size_code > 37 {
        return Some(None);
    }

    let mut bits = BitReader::new(&data[start + 6..]);
    let acmod = bits.read_bits(3)? as usize;
    if acmod == 2 {
        bits.skip_bits(2)?;
    } else {
        if (acmod & 1) != 0 && acmod != 1 {
            bits.skip_bits(2)?;
        }
        if (acmod & 4) != 0 {
            bits.skip_bits(2)?;
        }
    }
    let lfe_on = bits.read_bit()? != 0;
    let sr_shift = usize::from(bsid.saturating_sub(8));
    let bit_rate_code = frame_size_code >> 1;

    Some(Some(Ac3Header {
        channels: AC3_CHANNELS_BY_ACMOD[acmod] + u8::from(lfe_on),
        sample_rate: AC3_SAMPLE_RATES[fscod] >> sr_shift,
        channel_layout: ac3_channel_layout(acmod, lfe_on),
        bit_rate_bps: Some((AC3_BITRATES_KBPS[bit_rate_code] * 1000) >> sr_shift),
        bitrate_is_estimate: false,
    }))
}

pub(crate) fn find_eac3_header(data: &[u8]) -> Option<Ac3Header> {
    find_eac3_header_from(data, 0)
        .ok()
        .map(|(_, header)| header)
}

/// The first E-AC-3 header at or after `from` and its offset, or the
/// offset at which the search gave up. Every start from `from` up to
/// that offset gives the same answer.
pub(crate) fn find_eac3_header_from(data: &[u8], from: usize) -> Result<(usize, Ac3Header), usize> {
    if data.len().saturating_sub(from) < 6 {
        return Err(data.len());
    }
    let mut cursor = from;
    while let Some(candidate) = scan::find_audio_sync_candidate(data, cursor) {
        if candidate.kind != scan::AudioSyncKind::Ac3 {
            cursor = candidate.offset + 1;
            continue;
        }
        let start = candidate.offset;
        if start + 6 > data.len() {
            return Err(start);
        }
        cursor = start + 1;
        match eac3_header_at(data, start) {
            Some(Some(header)) => return Ok((start, header)),
            Some(None) => {}
            None => return Err(start),
        }
    }

    Err(data.len())
}

/// The E-AC-3 header whose sync word is at `start`: `Some(None)` when it
/// starts no header, `None` when the header runs past the data.
fn eac3_header_at(data: &[u8], start: usize) -> Option<Option<Ac3Header>> {
    let bsid = data[start + 5] >> 3;
    if !(11..=16).contains(&bsid) {
        return Some(None);
    }

    let mut bits = BitReader::new(&data[start + 2..]);
    let frame_type = bits.read_bits(2)?;
    if frame_type == 1 || frame_type == 3 {
        return Some(None);
    }
    if bits.read_bits(3)? != 0 {
        return Some(None);
    }
    let frame_size = (bits.read_bits(11)? + 1) * 2;

    let fscod = bits.read_bits(2)? as usize;
    let (sample_rate, num_blocks) = if fscod == 3 {
        let sample_rate = match bits.read_bits(2)? {
            0 => 24_000,
            1 => 22_050,
            2 => 16_000,
            _ => return Some(None),
        };
        (sample_rate, 6)
    } else {
        let sample_rate = *AC3_SAMPLE_RATES.get(fscod)?;
        if sample_rate == 0 {
            return Some(None);
        }
        let num_blocks = *EAC3_BLOCKS.get(bits.read_bits(2)? as usize)?;
        (sample_rate, num_blocks)
    };

    let acmod = bits.read_bits(3)? as usize;
    let lfe_on = bits.read_bit()? != 0;

    Some(Some(Ac3Header {
        channels: AC3_CHANNELS_BY_ACMOD[acmod] + u8::from(lfe_on),
        bit_rate_bps: Some((8 * frame_size * sample_rate) / (num_blocks * 256)),
        sample_rate,
        channel_layout: ac3_channel_layout(acmod, lfe_on),
        bitrate_is_estimate: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ac3_header_channels_and_bitrate() {
        let data = [0x0B, 0x77, 0x00, 0x00, 0x0A, 0x40, 0x50];
        let header = find_ac3_header(&data).unwrap();
        assert_eq!(header.channels, 2);
        assert_eq!(header.bit_rate_bps, Some(80_000));
        assert_eq!(header.sample_rate, 48_000);
        assert_eq!(header.channel_layout, "stereo");
        assert!(!header.bitrate_is_estimate);
        let half_rate = [0x0B, 0x77, 0, 0, 0x0A, 0x48, 0x50];
        assert_eq!(find_ac3_header(&half_rate).unwrap().sample_rate, 24_000);
    }

    #[test]
    fn parses_eac3_header_channels_and_bitrate() {
        let data = [0x0B, 0x77, 0x00, 0x0F, 0x34, 0x80];
        let header = find_eac3_header(&data).unwrap();
        assert_eq!(header.channels, 2);
        assert_eq!(header.bit_rate_bps, Some(8_000));
        assert_eq!(header.sample_rate, 48_000);
        assert_eq!(header.channel_layout, "stereo");
        assert!(header.bitrate_is_estimate);
        let mut reserved = data;
        reserved[5] = 17 << 3;
        assert!(find_eac3_header(&reserved).is_none());
        let mut dependent = data;
        dependent[2] = 0x40;
        assert!(find_eac3_header(&dependent).is_none());
    }
}
