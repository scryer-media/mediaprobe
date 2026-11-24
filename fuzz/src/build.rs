//! Minimal, well-formed containers built around fuzz input.
//!
//! A configuration record or a bitstream fragment is only reached by the
//! parser if the container around it is valid. These builders write the
//! smallest MP4, Matroska and transport stream that carry one track, so the
//! fuzz bytes land in the codec parsers instead of being rejected at the
//! container level. Nothing here calls the parser.

use crate::boxes::Mp4Box;
use crate::ebml::Element;

pub fn annexb(nals: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

pub fn length_prefixed(nals: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

// ---------------------------------------------------------------------------
// MP4
// ---------------------------------------------------------------------------

/// One track of a built MP4.
#[derive(Debug, Clone)]
pub struct Mp4Track {
    /// `vide` or `soun`.
    pub handler: [u8; 4],
    /// The sample entry type (`avc1`, `hvc1`, `mp4a`, ...).
    pub entry: [u8; 4],
    /// Boxes inside the sample entry: the configuration record and friends.
    pub entry_boxes: Vec<Mp4Box>,
    /// Extra boxes at the end of `trak`, such as `edts` or `udta`.
    pub trak_boxes: Vec<Mp4Box>,
    /// The single sample's bytes.
    pub sample: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub channels: u16,
    pub sample_rate: u16,
}

impl Mp4Track {
    pub fn video(
        entry: &[u8; 4],
        entry_boxes: Vec<Mp4Box>,
        sample: Vec<u8>,
        w: u16,
        h: u16,
    ) -> Self {
        Self {
            handler: *b"vide",
            entry: *entry,
            entry_boxes,
            trak_boxes: Vec::new(),
            sample,
            width: w,
            height: h,
            channels: 0,
            sample_rate: 0,
        }
    }

    pub fn audio(
        entry: &[u8; 4],
        entry_boxes: Vec<Mp4Box>,
        sample: Vec<u8>,
        channels: u16,
    ) -> Self {
        Self {
            handler: *b"soun",
            entry: *entry,
            entry_boxes,
            trak_boxes: Vec::new(),
            sample,
            width: 0,
            height: 0,
            channels,
            sample_rate: 48000,
        }
    }
}

fn full(version: u8, flags: u32, rest: &[u8]) -> Vec<u8> {
    let mut out = vec![version];
    out.extend_from_slice(&flags.to_be_bytes()[1..]);
    out.extend_from_slice(rest);
    out
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

fn matrix() -> Vec<u8> {
    MATRIX.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn be32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn trak(track: &Mp4Track, id: u32, chunk_offset: u32) -> Mp4Box {
    let mut tkhd = be32s(&[0, 0, id, 0, 1000, 0, 0]);
    tkhd.extend_from_slice(&[0, 0, 0, 0]); // layer, alternate group
    tkhd.extend_from_slice(
        &(if track.handler == *b"soun" {
            0x0100u16
        } else {
            0
        })
        .to_be_bytes(),
    );
    tkhd.extend_from_slice(&[0, 0]);
    tkhd.extend_from_slice(&matrix());
    tkhd.extend_from_slice(&be32s(&[
        (track.width as u32) << 16,
        (track.height as u32) << 16,
    ]));

    let mut mdhd = be32s(&[0, 0, 1000, 1000]);
    mdhd.extend_from_slice(&[0x55, 0xC4, 0, 0]); // "und", pre_defined

    let mut hdlr = be32s(&[0]);
    hdlr.extend_from_slice(&track.handler);
    hdlr.extend_from_slice(&[0; 12]);
    hdlr.push(0);

    let media_header = if track.handler == *b"soun" {
        Mp4Box::leaf(b"smhd", full(0, 0, &[0, 0, 0, 0]))
    } else {
        Mp4Box::leaf(b"vmhd", full(0, 1, &[0; 8]))
    };

    let mut entry_prefix = vec![0u8; 6];
    entry_prefix.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    if track.handler == *b"soun" {
        entry_prefix.extend_from_slice(&[0; 8]);
        entry_prefix.extend_from_slice(&track.channels.to_be_bytes());
        entry_prefix.extend_from_slice(&16u16.to_be_bytes());
        entry_prefix.extend_from_slice(&[0; 4]);
        entry_prefix.extend_from_slice(&((track.sample_rate as u32) << 16).to_be_bytes());
    } else {
        entry_prefix.extend_from_slice(&[0; 16]);
        entry_prefix.extend_from_slice(&track.width.to_be_bytes());
        entry_prefix.extend_from_slice(&track.height.to_be_bytes());
        entry_prefix.extend_from_slice(&be32s(&[0x0048_0000, 0x0048_0000, 0]));
        entry_prefix.extend_from_slice(&1u16.to_be_bytes());
        entry_prefix.extend_from_slice(&[0; 32]);
        entry_prefix.extend_from_slice(&[0x00, 0x18, 0xFF, 0xFF]);
    }
    let entry = Mp4Box::container(&track.entry, entry_prefix, track.entry_boxes.clone());

    let stbl = Mp4Box::container(
        b"stbl",
        Vec::new(),
        vec![
            Mp4Box::container(b"stsd", full(0, 0, &1u32.to_be_bytes()), vec![entry]),
            Mp4Box::leaf(b"stts", full(0, 0, &be32s(&[1, 1, 1000]))),
            Mp4Box::leaf(b"stsc", full(0, 0, &be32s(&[1, 1, 1, 1]))),
            Mp4Box::leaf(
                b"stsz",
                full(0, 0, &be32s(&[0, 1, track.sample.len() as u32])),
            ),
            Mp4Box::leaf(b"stco", full(0, 0, &be32s(&[1, chunk_offset]))),
        ],
    );
    let dinf = Mp4Box::container(
        b"dinf",
        Vec::new(),
        vec![Mp4Box::leaf(b"dref", {
            let mut d = full(0, 0, &1u32.to_be_bytes());
            d.extend_from_slice(&Mp4Box::leaf(b"url ", full(0, 1, &[])).to_bytes());
            d
        })],
    );
    let minf = Mp4Box::container(b"minf", Vec::new(), vec![media_header, dinf, stbl]);
    let mdia = Mp4Box::container(
        b"mdia",
        Vec::new(),
        vec![
            Mp4Box::leaf(b"mdhd", full(0, 0, &mdhd)),
            Mp4Box::leaf(b"hdlr", full(0, 0, &hdlr)),
            minf,
        ],
    );
    let mut children = vec![Mp4Box::leaf(b"tkhd", full(0, 3, &tkhd)), mdia];
    children.extend(track.trak_boxes.iter().cloned());
    Mp4Box::container(b"trak", Vec::new(), children)
}

/// An MP4 with the given tracks, one sample each, all in one `mdat`.
pub fn mp4(tracks: &[Mp4Track]) -> Vec<u8> {
    let ftyp = Mp4Box::leaf(b"ftyp", b"isom\x00\x00\x02\x00isomiso2avc1mp41".to_vec());
    let moov_for = |offsets: &[u32]| {
        let mut mvhd = be32s(&[0, 0, 1000, 1000, 0x0001_0000]);
        mvhd.extend_from_slice(&[0x01, 0x00]);
        mvhd.extend_from_slice(&[0; 10]);
        mvhd.extend_from_slice(&matrix());
        mvhd.extend_from_slice(&[0; 24]);
        mvhd.extend_from_slice(&(tracks.len() as u32 + 1).to_be_bytes());
        let mut children = vec![Mp4Box::leaf(b"mvhd", full(0, 0, &mvhd))];
        for (i, (track, offset)) in tracks.iter().zip(offsets).enumerate() {
            children.push(trak(track, i as u32 + 1, *offset));
        }
        Mp4Box::container(b"moov", Vec::new(), children)
    };
    // Offsets do not change the moov's length, so lay out once with zeros.
    let probe = moov_for(&vec![0; tracks.len()]);
    let mut at = (ftyp.len() + probe.len() + 8) as u32;
    let mut offsets = Vec::new();
    let mut mdat = Vec::new();
    for track in tracks {
        offsets.push(at);
        at += track.sample.len() as u32;
        mdat.extend_from_slice(&track.sample);
    }
    let mut out = ftyp.to_bytes();
    out.extend_from_slice(&moov_for(&offsets).to_bytes());
    out.extend_from_slice(&Mp4Box::leaf(b"mdat", mdat).to_bytes());
    out
}

/// A single video track whose sample entry carries `kind` with `payload`.
/// The entry type follows the record; an HEVC parameter-set record rides
/// along for records that annotate HEVC (Dolby Vision, colour, HDR boxes).
pub fn mp4_with_config(kind: &[u8; 4], payload: &[u8], w: u16, h: u16) -> Vec<u8> {
    let config = Mp4Box::leaf(kind, payload.to_vec());
    let (entry, boxes, sample) = match kind {
        b"avcC" => (
            *b"avc1",
            vec![config],
            length_prefixed(&[&crate::params::H264_SPS[..], &crate::params::H264_PPS[..]]),
        ),
        b"av1C" => (*b"av01", vec![config], vec![0x12, 0x00]),
        b"vpcC" => (*b"vp09", vec![config], vec![0x82, 0x49, 0x83, 0x42, 0x00]),
        b"hvcC" => (*b"hvc1", vec![config], hevc_sample()),
        _ => (
            *b"hvc1",
            vec![Mp4Box::leaf(b"hvcC", crate::params::hevc_hvcc()), config],
            hevc_sample(),
        ),
    };
    mp4(&[Mp4Track::video(&entry, boxes, sample, w, h)])
}

fn hevc_sample() -> Vec<u8> {
    length_prefixed(&[
        &crate::params::HEVC_VPS[..],
        &crate::params::HEVC_SPS[..],
        &crate::params::HEVC_PPS[..],
    ])
}

// ---------------------------------------------------------------------------
// Matroska
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MkvTrack {
    /// 1 video, 2 audio, 17 subtitle.
    pub kind: u64,
    pub codec_id: Vec<u8>,
    pub codec_private: Option<Vec<u8>>,
    /// Children of `Video` (Colour, Projection, ...) or `Audio`.
    pub media_children: Vec<Element>,
    /// Further children of `TrackEntry` (BlockAdditionMapping, Language, ...).
    pub entry_children: Vec<Element>,
    /// One frame, written as a SimpleBlock.
    pub frame: Vec<u8>,
    pub width: u64,
    pub height: u64,
}

pub fn mkv(tracks: &[MkvTrack]) -> Vec<u8> {
    mkv_with(tracks, Vec::new())
}

/// [`mkv`] with further top-level Segment children (Tags, Chapters,
/// Attachments) after the cluster.
pub fn mkv_with(tracks: &[MkvTrack], extra: Vec<Element>) -> Vec<u8> {
    let header = Element::master(
        0x1A45_DFA3,
        vec![
            Element::uint(0x4286, 1),
            Element::uint(0x42F7, 1),
            Element::uint(0x42F2, 4),
            Element::uint(0x42F3, 8),
            Element::leaf(0x4282, b"matroska".to_vec()),
            Element::uint(0x4287, 4),
            Element::uint(0x4285, 2),
        ],
    );
    let info = Element::master(
        0x1549_A966,
        vec![
            Element::uint(0x2AD7B1, 1_000_000),
            Element::leaf(0x4489, 1000.0f64.to_be_bytes().to_vec()),
            Element::leaf(0x4D80, b"mediaprobe-fuzz".to_vec()),
            Element::leaf(0x5741, b"mediaprobe-fuzz".to_vec()),
        ],
    );
    let mut entries = Vec::new();
    let mut blocks = vec![Element::uint(0xE7, 0)];
    for (i, t) in tracks.iter().enumerate() {
        let number = i as u64 + 1;
        let mut children = vec![
            Element::uint(0xD7, number),
            Element::uint(0x73C5, number),
            Element::uint(0x83, t.kind),
            Element::leaf(0x86, t.codec_id.clone()),
        ];
        if let Some(private) = &t.codec_private {
            children.push(Element::leaf(0x63A2, private.clone()));
        }
        match t.kind {
            1 => {
                let mut video = vec![Element::uint(0xB0, t.width), Element::uint(0xBA, t.height)];
                video.extend(t.media_children.iter().cloned());
                children.push(Element::master(0xE0, video));
            }
            2 => {
                let mut audio = vec![
                    Element::leaf(0xB5, 48000.0f64.to_be_bytes().to_vec()),
                    Element::uint(0x9F, 2),
                ];
                audio.extend(t.media_children.iter().cloned());
                children.push(Element::master(0xE1, audio));
            }
            _ => {}
        }
        children.extend(t.entry_children.iter().cloned());
        entries.push(Element::master(0xAE, children));
        let mut block = vec![0x80 | number as u8, 0, 0, 0x80];
        block.extend_from_slice(&t.frame);
        blocks.push(Element::leaf(0xA3, block));
    }
    let mut children = vec![
        info,
        Element::master(0x1654_AE6B, entries),
        Element::master(0x1F43_B675, blocks),
    ];
    children.extend(extra);
    let segment = Element::master(0x1853_8067, children);
    let mut out = Vec::new();
    header.write(&mut out);
    segment.write(&mut out);
    out
}

/// One video track with `codec_private`, and `frame` as its only block.
pub fn mkv_with_codec(
    codec_id: &[u8],
    codec_private: &[u8],
    frame: &[u8],
    w: u64,
    h: u64,
) -> Vec<u8> {
    mkv(&[MkvTrack {
        kind: 1,
        codec_id: codec_id.to_vec(),
        codec_private: Some(codec_private.to_vec()),
        media_children: Vec::new(),
        entry_children: Vec::new(),
        frame: frame.to_vec(),
        width: w,
        height: h,
    }])
}

// ---------------------------------------------------------------------------
// Transport stream
// ---------------------------------------------------------------------------

/// CRC-32/MPEG-2, as PSI sections carry it.
pub fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                crc << 1 ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

const PMT_PID: u16 = 0x1000;
const ES_PID: u16 = 0x0100;

fn section(table_id: u8, id: u16, body: &[u8]) -> Vec<u8> {
    let length = 5 + body.len() + 4;
    let mut s = vec![table_id, 0xB0 | (length >> 8) as u8, length as u8];
    s.extend_from_slice(&id.to_be_bytes());
    s.extend_from_slice(&[0xC1, 0, 0]);
    s.extend_from_slice(body);
    let crc = crc32_mpeg(&s);
    s.extend_from_slice(&crc.to_be_bytes());
    s
}

/// Split `payload` into 188-byte packets on `pid`, stuffing the last one
/// through its adaptation field.
fn packets(pid: u16, payload: &[u8], cc: &mut u8, out: &mut Vec<u8>) {
    let mut first = true;
    let mut rest = payload;
    loop {
        let take = rest.len().min(184);
        let mut p = vec![
            0x47,
            (pid >> 8) as u8 | if first { 0x40 } else { 0 },
            pid as u8,
        ];
        if take == 184 {
            p.push(0x10 | *cc);
        } else {
            p.push(0x30 | *cc);
            let stuffing = 184 - take - 1;
            p.push(stuffing as u8);
            if stuffing > 0 {
                p.push(0x00);
                p.extend(std::iter::repeat_n(0xFF, stuffing - 1));
            }
        }
        p.extend_from_slice(&rest[..take]);
        out.extend_from_slice(&p);
        *cc = (*cc + 1) & 0x0F;
        rest = &rest[take..];
        first = false;
        if rest.is_empty() {
            break;
        }
    }
}

fn pes(stream_id: u8, pts: u64, es: &[u8]) -> Vec<u8> {
    let mut p = vec![0, 0, 1, stream_id, 0, 0, 0x80, 0x80, 5];
    p.extend_from_slice(&[
        0x21 | ((pts >> 29) as u8 & 0x0E),
        (pts >> 22) as u8,
        0x01 | ((pts >> 14) as u8 & 0xFE),
        (pts >> 7) as u8,
        0x01 | ((pts << 1) as u8 & 0xFE),
    ]);
    p.extend_from_slice(es);
    p
}

/// One program with one elementary stream of `stream_type` carrying
/// `descriptors` in its ES_info loop, and `es` sent as three PES packets
/// one second apart.
pub fn ts_with_es(stream_type: u8, descriptors: &[u8], es: &[u8]) -> Vec<u8> {
    let mut pat_body = 1u16.to_be_bytes().to_vec();
    pat_body.extend_from_slice(&(0xE000 | PMT_PID).to_be_bytes());
    let pat = section(0x00, 1, &pat_body);

    let mut pmt_body = (0xE000 | ES_PID).to_be_bytes().to_vec();
    pmt_body.extend_from_slice(&0xF000u16.to_be_bytes());
    pmt_body.push(stream_type);
    pmt_body.extend_from_slice(&(0xE000 | ES_PID).to_be_bytes());
    let info = descriptors.len().min(0x3FF);
    pmt_body.extend_from_slice(&(0xF000 | info as u16).to_be_bytes());
    pmt_body.extend_from_slice(&descriptors[..info]);
    let pmt = section(0x02, 1, &pmt_body);

    let audio = matches!(stream_type, 0x03 | 0x04 | 0x0F | 0x11 | 0x81 | 0x87);
    let stream_id = if audio { 0xC0 } else { 0xE0 };
    let mut out = Vec::new();
    let (mut c0, mut c1, mut c2) = (0, 0, 0);
    for k in 0..3u64 {
        let mut pat_psi = vec![0];
        pat_psi.extend_from_slice(&pat);
        packets(0, &pat_psi, &mut c0, &mut out);
        let mut pmt_psi = vec![0];
        pmt_psi.extend_from_slice(&pmt);
        packets(PMT_PID, &pmt_psi, &mut c1, &mut out);
        packets(
            ES_PID,
            &pes(stream_id, 90_000 * (k + 1), es),
            &mut c2,
            &mut out,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_mpeg_check_value() {
        assert_eq!(crc32_mpeg(b"123456789"), 0x0376_E6E7);
    }

    #[test]
    fn transport_streams_are_whole_packets_with_valid_psi() {
        let ts = ts_with_es(0x24, &[0xB0, 0x05, 1, 0, 0x10, 0x3F, 0x00], &[0; 500]);
        assert_eq!(ts.len() % 188, 0);
        assert!(ts.chunks(188).all(|p| p[0] == 0x47));
        // The PAT's CRC covers its section, which follows the stuffing and
        // the pointer field.
        assert_eq!(ts[3] & 0x30, 0x30);
        let pat = &ts[4 + 1 + ts[4] as usize + 1..];
        let len = (((pat[1] & 0x0F) as usize) << 8) | pat[2] as usize;
        assert_eq!(crc32_mpeg(&pat[..3 + len]), 0);
    }

    #[test]
    fn mp4_chunk_offsets_point_at_the_samples() {
        let sample = b"SAMPLE-BYTES".to_vec();
        let file = mp4(&[Mp4Track::video(b"avc1", vec![], sample.clone(), 64, 36)]);
        let at = file.windows(4).position(|w| w == b"stco").unwrap();
        let offset = u32::from_be_bytes(file[at + 12..at + 16].try_into().unwrap()) as usize;
        assert_eq!(&file[offset..offset + sample.len()], &sample[..]);
    }
}
