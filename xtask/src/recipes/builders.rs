//! Fixtures Rust writes outright: scanner input that is not media, and OGM,
//! the DirectShow-era stream headers inside Ogg that no FFmpeg muxer emits.

use std::path::Path;

use super::matrix::args;
use super::{Expect, Fixture, Generator, Step};
use crate::corpus::{Context as _, Result, fail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Builder {
    SimdScanDense,
    OgmAudioOnly,
    /// Reads `{work}/vorbis.ogg`, which an FFmpeg step writes first.
    OgmVideoAudioText,
}

pub(crate) fn build(builder: Builder, work: &Path) -> Result<Vec<u8>> {
    match builder {
        Builder::SimdScanDense => Ok(simd_scan_dense()),
        Builder::OgmAudioOnly => Ok(ogm_audio_only()),
        Builder::OgmVideoAudioText => {
            let path = work.join("vorbis.ogg");
            let ogg = std::fs::read(&path).context(|| format!("read {}", path.display()))?;
            ogm_video_audio_text(&ogg)
        }
    }
}

pub(crate) fn fixtures() -> Vec<Fixture> {
    let mut audio_only = Expect::video("ogm", "none", 0, 0, 0);
    audio_only.video_codec = None;
    audio_only.valid_video = false;
    audio_only.min_duration_seconds = 0;
    audio_only = audio_only.audio("ac3", 6, Some("deu"));

    let video = Expect::video("ogm", "mpeg4", 1280, 720, 25).audio("vorbis", 2, Some("eng"));
    let mut video = video;
    video.subtitle_stream_count = 1;
    video.min_duration_seconds = 2;

    vec![
        Fixture {
            name: "ogm_audio_only.ogm".into(),
            generator: Generator::Builders,
            steps: vec![Step::Build(Builder::OgmAudioOnly)],
            expect: audio_only,
            encoders: Vec::new(),
        },
        Fixture {
            name: "ogm_video_audio_text.ogm".into(),
            generator: Generator::Builders,
            steps: vec![
                Step::Ffmpeg(args(&[
                    "-f",
                    "lavfi",
                    "-i",
                    "anullsrc=r=48000:cl=stereo",
                    "-t",
                    "0.2",
                    "-c:a",
                    "vorbis",
                    "-strict",
                    "-2",
                    "-f",
                    "ogg",
                    "{work}/vorbis.ogg",
                ])),
                Step::Build(Builder::OgmVideoAudioText),
            ],
            expect: video,
            encoders: vec!["vorbis"],
        },
    ]
}

/// Byte patterns every scanner looks for, each preceded by filler long enough
/// to push it past one or more SIMD blocks, then an AVI idx1 entry.
pub(crate) fn simd_scan_dense() -> Vec<u8> {
    let mut data = Vec::new();
    let late = |data: &mut Vec<u8>, payload: &[u8], pad: usize| {
        data.extend(std::iter::repeat_n(0x55, pad));
        data.extend_from_slice(payload);
    };
    late(&mut data, b"\xA7", 1024);
    late(&mut data, b"\x47", 512);
    late(&mut data, b"\x00\x00\x01\xB3", 1024);
    late(&mut data, b"\x00\x00\x00\x01\x67", 1024);
    late(&mut data, b"\xFF\xF1\x50\x80", 512);
    late(&mut data, b"\x56\xE0\x00\x00", 512);
    late(&mut data, b"\xFF\xE2\x00\x00", 512);
    late(&mut data, b"\x0B\x77\x00\x00", 512);
    late(&mut data, b"\x7F\xFE\x80\x01", 512);
    late(&mut data, b"\x1F\x43\xB6\x75", 1024);
    late(&mut data, b"\xA3", 512);
    late(&mut data, b"\x75\xA1", 512);
    late(&mut data, b"\xE7", 512);
    late(&mut data, b"\x4E\x01\x50\x00", 1024);
    late(&mut data, b"\xB5\x00\x3C\x00\x01\x04", 1024);
    late(&mut data, b"\x00\x00\x03\x01", 512);
    late(&mut data, b"\x00\x00\x00\x0Cmoov\x00\x00\x00\x00", 1024);
    let padding = (16 - data.len() % 16) % 16;
    data.extend(std::iter::repeat_n(0x55, padding));
    data.extend_from_slice(b"02wb");
    data.extend_from_slice(&0_u32.to_le_bytes());
    data.extend_from_slice(&128_u32.to_le_bytes());
    data.extend_from_slice(&64_u32.to_le_bytes());
    data
}

/// The OGM stream header: packet type 1, then the DirectShow `stream_header`.
fn ogm_header(kind: &[u8; 8], subtype: &[u8; 4], time_unit: u64, samples_per_unit: u64) -> Vec<u8> {
    let mut header = vec![1];
    header.extend_from_slice(kind);
    header.extend_from_slice(subtype);
    header.extend_from_slice(&52_u32.to_le_bytes());
    header.extend_from_slice(&time_unit.to_le_bytes());
    header.extend_from_slice(&samples_per_unit.to_le_bytes());
    header.extend_from_slice(&1_u32.to_le_bytes()); // default_len
    header.extend_from_slice(&0_u32.to_le_bytes()); // buffersize
    header.extend_from_slice(&16_u16.to_le_bytes()); // bits_per_sample
    header.extend_from_slice(&0_u16.to_le_bytes()); // padding
    match kind {
        b"video\0\0\0" => {
            header.extend_from_slice(&1280_u32.to_le_bytes());
            header.extend_from_slice(&720_u32.to_le_bytes());
        }
        b"audio\0\0\0" => {
            header.extend_from_slice(&6_u16.to_le_bytes());
            header.extend_from_slice(&0_u16.to_le_bytes());
            header.extend_from_slice(&48_000_u32.to_le_bytes());
        }
        _ => header.extend_from_slice(&[0; 8]),
    }
    header
}

/// A Vorbis-comment packet as OGM streams carry them (packet type 3).
fn ogm_comments(entries: &[&str]) -> Vec<u8> {
    let mut packet = b"\x03vorbis".to_vec();
    packet.extend_from_slice(&7_u32.to_le_bytes());
    packet.extend_from_slice(b"fixture");
    packet.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        packet.extend_from_slice(&(entry.len() as u32).to_le_bytes());
        packet.extend_from_slice(entry.as_bytes());
    }
    packet.push(1);
    packet
}

pub(crate) fn ogm_audio_only() -> Vec<u8> {
    let mut ogg = OggWriter::default();
    ogg.page(
        1,
        &ogm_header(b"audio\0\0\0", b"2000", 10_000_000, 48_000),
        0,
        PageKind::First,
    );
    ogg.page(
        1,
        &ogm_comments(&["LANGUAGE=deu", "TITLE=Surround"]),
        0,
        PageKind::Middle,
    );
    ogg.page(1, &[0x80, 0, 6, 0], 142_464, PageKind::Last);
    ogg.bytes
}

pub(crate) fn ogm_video_audio_text(vorbis_ogg: &[u8]) -> Result<Vec<u8>> {
    let packets = ogg_packets(vorbis_ogg)?;
    let vorbis: Vec<&Vec<u8>> = packets.iter().take(3).collect();
    if vorbis.len() < 3
        || !vorbis[0].starts_with(b"\x01vorbis")
        || !vorbis[2].starts_with(b"\x05vorbis")
    {
        return fail("FFmpeg's Ogg Vorbis output did not start with the three Vorbis headers");
    }
    let rate = u32::from_le_bytes(vorbis[0][12..16].try_into().unwrap_or_default());
    let mut ogg = OggWriter::default();
    ogg.page(
        1,
        &ogm_header(b"video\0\0\0", b"XVID", 400_000, 1),
        0,
        PageKind::First,
    );
    ogg.page(2, vorbis[0], 0, PageKind::First);
    ogg.page(
        3,
        &ogm_header(b"text\0\0\0\0", &[0; 4], 10_000, 1),
        0,
        PageKind::First,
    );
    ogg.page(1, &ogm_comments(&["TITLE=Main video"]), 0, PageKind::Middle);
    ogg.page(
        2,
        &ogm_comments(&["LANGUAGE=eng", "TITLE=Stereo"]),
        0,
        PageKind::Middle,
    );
    ogg.page(2, vorbis[2], 0, PageKind::Middle);
    ogg.page(
        3,
        &ogm_comments(&["Language=jpn", "NAME=Captions"]),
        0,
        PageKind::Middle,
    );
    ogg.page(3, &[0x80, 0xf4, 1, b'H', b'i'], 2_000, PageKind::Last);
    ogg.page(1, &[0x48, 1, 0], 74, PageKind::Last);
    ogg.page(2, &[0], u64::from(rate) * 3, PageKind::Last);
    Ok(ogg.bytes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PageKind {
    First,
    Middle,
    Last,
}

/// One packet per page, which every OGM packet here fits.
#[derive(Default)]
struct OggWriter {
    bytes: Vec<u8>,
    sequence: std::collections::BTreeMap<u32, u32>,
}

impl OggWriter {
    fn page(&mut self, serial: u32, packet: &[u8], granule: u64, kind: PageKind) {
        let sequence = self.sequence.entry(serial).or_insert(0);
        let mut lacing = Vec::new();
        let mut left = packet.len();
        loop {
            let take = left.min(255);
            lacing.push(take as u8);
            left -= take;
            if take < 255 {
                break;
            }
        }
        assert!(lacing.len() <= 255, "OGM builder packets fit one page");
        let start = self.bytes.len();
        self.bytes.extend_from_slice(b"OggS");
        self.bytes.push(0);
        self.bytes.push(match kind {
            PageKind::First => 0x02,
            PageKind::Middle => 0x00,
            PageKind::Last => 0x04,
        });
        self.bytes.extend_from_slice(&granule.to_le_bytes());
        self.bytes.extend_from_slice(&serial.to_le_bytes());
        self.bytes.extend_from_slice(&sequence.to_le_bytes());
        self.bytes.extend_from_slice(&[0; 4]);
        self.bytes.push(lacing.len() as u8);
        self.bytes.extend_from_slice(&lacing);
        self.bytes.extend_from_slice(packet);
        let crc = ogg_crc(&self.bytes[start..]);
        self.bytes[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
        *sequence += 1;
    }
}

/// Ogg's CRC-32: polynomial 0x04c11db7, no reflection, zero init and xor.
pub(crate) fn ogg_crc(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for byte in data {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Reassemble the packets of the first logical stream in an Ogg file.
pub(crate) fn ogg_packets(data: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut packets = Vec::new();
    let mut current = Vec::new();
    let mut at = 0;
    let mut serial = None;
    while at + 27 <= data.len() {
        if &data[at..at + 4] != b"OggS" {
            return fail(format!("no Ogg capture pattern at {at}"));
        }
        let page_serial = u32::from_le_bytes(data[at + 14..at + 18].try_into().unwrap_or_default());
        let segments = usize::from(data[at + 26]);
        let table = data
            .get(at + 27..at + 27 + segments)
            .ok_or_else(|| crate::corpus::error("truncated Ogg page"))?;
        let mut body = at + 27 + segments;
        let mine = *serial.get_or_insert(page_serial) == page_serial;
        for &lace in table {
            let end = body + usize::from(lace);
            let bytes = data
                .get(body..end)
                .ok_or_else(|| crate::corpus::error("truncated Ogg segment"))?;
            if mine {
                current.extend_from_slice(bytes);
                if lace < 255 {
                    packets.push(std::mem::take(&mut current));
                }
            }
            body = end;
        }
        at = body;
    }
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ogg_crc_matches_the_reference_value() {
        // A page header with a zeroed CRC field, checked against libogg.
        assert_eq!(ogg_crc(b""), 0);
        assert_eq!(ogg_crc(b"123456789"), 0x89a1_897f);
    }

    #[test]
    fn written_pages_read_back_as_packets() {
        let bytes = ogm_audio_only();
        let packets = ogg_packets(&bytes).unwrap();
        assert_eq!(packets.len(), 3);
        assert!(packets[0].starts_with(b"\x01audio"));
        assert!(packets[1].starts_with(b"\x03vorbis"));
    }

    #[test]
    fn scanner_input_ends_with_an_idx1_entry() {
        let data = simd_scan_dense();
        assert_eq!(&data[data.len() - 16..data.len() - 12], b"02wb");
        assert_eq!((data.len() - 16) % 16, 0);
    }
}
