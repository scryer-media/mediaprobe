//! What each wrapping fuzz target feeds the parser, as plain bytes.
//!
//! The first input byte picks a carrier (a container and a slot in it); the
//! rest becomes the payload of that slot. Keeping this apart from the
//! parser call means the selection and wrapping are unit-tested on stable
//! Rust without the parser, and replay exactly from a crash file.

use crate::boxes::Mp4Box;
use crate::build::{self, MkvTrack, Mp4Track};
use crate::ebml::Element;
use crate::params::{self, H264_PPS, H264_SPS, HEVC_PPS, HEVC_SPS, HEVC_VPS};

/// One input for the parser: the bytes and the extension hint to pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub bytes: Vec<u8>,
    pub extension: String,
}

impl Input {
    fn new(bytes: Vec<u8>, extension: &str) -> Self {
        Self {
            bytes,
            extension: extension.to_owned(),
        }
    }
}

fn split(data: &[u8]) -> (u8, &[u8]) {
    match data.split_first() {
        Some((first, rest)) => (*first, rest),
        None => (0, &[]),
    }
}

// ---------------------------------------------------------------------------
// Extensions
// ---------------------------------------------------------------------------

/// Every extension the dispatcher distinguishes, plus near-misses.
pub const EXTENSIONS: [&str; 52] = [
    "mkv", "mka", "mks", "mk3d", "webm", "mp4", "m4v", "m4a", "mov", "qt", "3gp", "3g2", "ismv",
    "f4v", "avi", "divx", "ts", "m2ts", "mts", "tp", "trp", "m2t", "mpg", "mpeg", "vob", "evo",
    "m2v", "m1v", "mpv", "h264", "264", "avc", "h265", "265", "hevc", "ogg", "ogv", "oga", "ogm",
    "flv", "wmv", "asf", "rm", "rmvb", "dv", "ivf", "obu", "iso", "img", "bin", "", "MKV",
];

/// The dispatcher with an arbitrary hint: the first byte picks a listed
/// extension, or, past the list, takes up to eight following bytes as the
/// extension itself.
pub fn dispatch(data: &[u8]) -> Input {
    let (pick, rest) = split(data);
    let pick = pick as usize;
    if pick < EXTENSIONS.len() {
        return Input::new(rest.to_vec(), EXTENSIONS[pick]);
    }
    let len = (pick - EXTENSIONS.len()) % 9;
    let len = len.min(rest.len());
    let extension = String::from_utf8_lossy(&rest[..len]).into_owned();
    Input::new(rest[len..].to_vec(), &extension)
}

// ---------------------------------------------------------------------------
// Configuration records
// ---------------------------------------------------------------------------

/// MP4 sample-entry boxes whose payload is the fuzz input.
pub const MP4_VIDEO_CONFIGS: [&[u8; 4]; 15] = [
    b"avcC", b"hvcC", b"av1C", b"vpcC", b"dvcC", b"dvvC", b"dvwC", b"mdcv", b"clli", b"colr",
    b"pasp", b"SmDm", b"CoLL", b"btrt", b"fiel",
];

/// MP4 audio sample entries and the configuration box each carries.
pub const MP4_AUDIO_CONFIGS: [(&[u8; 4], &[u8; 4]); 8] = [
    (b"mp4a", b"esds"),
    (b"ac-3", b"dac3"),
    (b"ec-3", b"dec3"),
    (b"Opus", b"dOps"),
    (b"fLaC", b"dfLa"),
    (b"alac", b"alac"),
    (b"ac-4", b"dac4"),
    (b"dtsc", b"ddts"),
];

/// Matroska codec IDs whose CodecPrivate is the fuzz input.
pub const MKV_CODECS: [(&[u8], u64); 14] = [
    (b"V_MPEG4/ISO/AVC", 1),
    (b"V_MPEGH/ISO/HEVC", 1),
    (b"V_AV1", 1),
    (b"V_VP9", 1),
    (b"V_MS/VFW/FOURCC", 1),
    (b"V_MPEG2", 1),
    (b"A_AAC", 2),
    (b"A_OPUS", 2),
    (b"A_FLAC", 2),
    (b"A_VORBIS", 2),
    (b"A_MS/ACM", 2),
    (b"A_TRUEHD", 2),
    (b"S_TEXT/ASS", 17),
    (b"S_HDMV/PGS", 17),
];

/// Transport stream types whose ES_info descriptor loop is the fuzz input.
pub const TS_STREAM_TYPES: [u8; 10] = [0x02, 0x1B, 0x24, 0x06, 0x0F, 0x11, 0x81, 0x87, 0xEA, 0x80];

/// A configuration record, in every container that carries one.
pub fn codec_config(data: &[u8]) -> Input {
    let (pick, payload) = split(data);
    let mp4_video = MP4_VIDEO_CONFIGS.len();
    let mp4_audio = mp4_video + MP4_AUDIO_CONFIGS.len();
    let mkv = mp4_audio + MKV_CODECS.len();
    let mkv_colour = mkv + 1;
    let ts = mkv_colour + TS_STREAM_TYPES.len();
    let pick = pick as usize % ts;
    if pick < mp4_video {
        return Input::new(
            build::mp4_with_config(MP4_VIDEO_CONFIGS[pick], payload, 64, 36),
            "mp4",
        );
    }
    if pick < mp4_audio {
        let (entry, kind) = MP4_AUDIO_CONFIGS[pick - mp4_video];
        let track = Mp4Track::audio(
            entry,
            vec![Mp4Box::leaf(kind, payload.to_vec())],
            vec![0; 16],
            2,
        );
        return Input::new(build::mp4(&[track]), "m4a");
    }
    if pick < mkv {
        let (codec, kind) = MKV_CODECS[pick - mp4_audio];
        let track = MkvTrack {
            kind,
            codec_id: codec.to_vec(),
            codec_private: Some(payload.to_vec()),
            media_children: Vec::new(),
            entry_children: Vec::new(),
            frame: vec![0; 16],
            width: 64,
            height: 36,
        };
        return Input::new(build::mkv(&[track]), "mkv");
    }
    if pick < mkv_colour {
        // The payload is the raw body of a Colour master on an HEVC track.
        return Input::new(
            hevc_mkv(vec![Element::leaf(0x55B0, payload.to_vec())], Vec::new()),
            "mkv",
        );
    }
    let stream_type = TS_STREAM_TYPES[pick - mkv_colour];
    Input::new(
        build::ts_with_es(stream_type, payload, &hevc_annexb(&[])),
        "ts",
    )
}

fn hevc_annexb(extra: &[&[u8]]) -> Vec<u8> {
    let mut nals: Vec<&[u8]> = vec![&HEVC_VPS, &HEVC_SPS, &HEVC_PPS];
    nals.extend_from_slice(extra);
    build::annexb(&nals)
}

fn hevc_mkv(video_children: Vec<Element>, entry_children: Vec<Element>) -> Vec<u8> {
    hevc_mkv_frame(video_children, entry_children, &[])
}

fn hevc_mkv_frame(
    video_children: Vec<Element>,
    entry_children: Vec<Element>,
    extra: &[&[u8]],
) -> Vec<u8> {
    let mut nals: Vec<&[u8]> = vec![&HEVC_VPS, &HEVC_SPS, &HEVC_PPS];
    nals.extend_from_slice(extra);
    build::mkv(&[MkvTrack {
        kind: 1,
        codec_id: b"V_MPEGH/ISO/HEVC".to_vec(),
        codec_private: Some(params::hevc_hvcc()),
        media_children: video_children,
        entry_children,
        frame: build::length_prefixed(&nals),
        width: 64,
        height: 36,
    }])
}

// ---------------------------------------------------------------------------
// Bitstreams
// ---------------------------------------------------------------------------

/// How a bitstream reaches the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
    Raw,
    Mkv,
    Mp4,
    Ts,
}

pub const CARRIERS: [Carrier; 4] = [Carrier::Raw, Carrier::Mkv, Carrier::Mp4, Carrier::Ts];

/// An H.264 or HEVC stream: parameter sets, then one NAL unit whose type
/// and payload come from the input, escaped so it stays one NAL unit.
fn nal_stream(hevc: bool, nal_type: u8, payload: &[u8], carrier: Carrier) -> Input {
    let mut nal = if hevc {
        vec![(nal_type & 0x3F) << 1, 1]
    } else {
        vec![0x60 | (nal_type & 0x1F)]
    };
    nal.extend_from_slice(&params::escape(payload));
    let nals: Vec<&[u8]> = if hevc {
        vec![&HEVC_VPS, &HEVC_SPS, &HEVC_PPS, &nal]
    } else {
        vec![&H264_SPS, &H264_PPS, &nal]
    };
    match carrier {
        Carrier::Raw => Input::new(build::annexb(&nals), if hevc { "hevc" } else { "h264" }),
        Carrier::Ts => Input::new(
            build::ts_with_es(if hevc { 0x24 } else { 0x1B }, &[], &build::annexb(&nals)),
            "ts",
        ),
        Carrier::Mkv => {
            let (codec, private) = if hevc {
                (&b"V_MPEGH/ISO/HEVC"[..], params::hevc_hvcc())
            } else {
                (&b"V_MPEG4/ISO/AVC"[..], params::H264_AVCC.to_vec())
            };
            Input::new(
                build::mkv_with_codec(codec, &private, &build::length_prefixed(&nals), 64, 36),
                "mkv",
            )
        }
        Carrier::Mp4 => {
            let (entry, config) = if hevc {
                (*b"hvc1", Mp4Box::leaf(b"hvcC", params::hevc_hvcc()))
            } else {
                (*b"avc1", Mp4Box::leaf(b"avcC", params::H264_AVCC.to_vec()))
            };
            let track =
                Mp4Track::video(&entry, vec![config], build::length_prefixed(&nals), 64, 36);
            Input::new(build::mp4(&[track]), "mp4")
        }
    }
}

/// Codec bitstreams: H.264 and HEVC NAL units of any type, MPEG-1/2 video
/// after any start code, and AV1 OBUs, in each carrier.
pub fn bitstream(data: &[u8]) -> Input {
    let (pick, rest) = split(data);
    let (nal_type, payload) = split(rest);
    let carrier = CARRIERS[(pick as usize >> 2) % CARRIERS.len()];
    match pick % 4 {
        0 => nal_stream(false, nal_type, payload, carrier),
        1 => nal_stream(true, nal_type, payload, carrier),
        2 => {
            // MPEG video: a start code of the input's choosing, then bytes.
            let mut es = vec![0, 0, 1, nal_type];
            es.extend_from_slice(payload);
            match carrier {
                Carrier::Ts => Input::new(build::ts_with_es(0x02, &[], &es), "ts"),
                _ => Input::new(es, "m2v"),
            }
        }
        _ => {
            // AV1: the input is an OBU sequence, as the frame of a track.
            let config = vec![0x81, 0x00, 0x0C, 0x00];
            match carrier {
                Carrier::Mp4 => {
                    let config = Mp4Box::leaf(b"av1C", config);
                    let track = Mp4Track::video(b"av01", vec![config], rest.to_vec(), 64, 36);
                    Input::new(build::mp4(&[track]), "mp4")
                }
                _ => Input::new(
                    build::mkv_with_codec(b"V_AV1", &config, rest, 64, 36),
                    "mkv",
                ),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HDR and SEI
// ---------------------------------------------------------------------------

/// SEI payload types the parser reads, plus neighbours.
pub const SEI_TYPES: [u8; 10] = [0, 1, 4, 5, 45, 47, 137, 144, 147, 148];

/// ITU-T T.35 prefixes that steer a type-4 SEI into a specific reader:
/// HDR10+ (Samsung), Dolby Vision metadata and ATSC A/53 captions.
pub const T35_PREFIXES: [&[u8]; 4] = [
    &[0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04],
    &[0xB5, 0x00, 0x3B, 0x00, 0x00, 0x08, 0x00],
    &[0xB5, 0x00, 0x31, b'G', b'A', b'9', b'4', 0x03],
    &[],
];

fn sei_message(payload_type: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(payload_type);
    let mut size = body.len();
    while size >= 255 {
        out.push(0xFF);
        size -= 255;
    }
    out.push(size as u8);
    out.extend_from_slice(body);
    out.push(0x80); // rbsp trailing bits
    out
}

/// An SEI message of a chosen type with the input as its body, in H.264 or
/// HEVC (prefix or suffix SEI), in each carrier.
pub fn hdr_sei(data: &[u8]) -> Input {
    let (pick, rest) = split(data);
    let payload_type = SEI_TYPES[pick as usize % SEI_TYPES.len()];
    let prefix = T35_PREFIXES[(pick as usize / SEI_TYPES.len()) % T35_PREFIXES.len()];
    let mut body = Vec::new();
    if payload_type == 4 {
        body.extend_from_slice(prefix);
    }
    body.extend_from_slice(rest);
    let sei = sei_message(payload_type, &body);
    let selector = pick as usize / (SEI_TYPES.len() * T35_PREFIXES.len());
    let carrier = CARRIERS[selector % CARRIERS.len()];
    match (selector / CARRIERS.len()) % 3 {
        0 => nal_stream(false, 6, &sei, carrier),
        1 => nal_stream(true, 39, &sei, carrier),
        _ => nal_stream(true, 40, &sei, carrier),
    }
}

// ---------------------------------------------------------------------------
// Dolby Vision
// ---------------------------------------------------------------------------

/// Every way Dolby Vision is signalled: configuration records in MP4 under
/// each sample entry, Matroska block-addition mappings, the transport
/// stream descriptor, and RPU or enhancement-layer NAL units.
pub fn dolby_vision(data: &[u8]) -> Input {
    let (pick, payload) = split(data);
    const ENTRIES: [&[u8; 4]; 7] = [
        b"hvc1", b"hev1", b"dvh1", b"dvhe", b"avc1", b"dvav", b"av01",
    ];
    const RECORDS: [&[u8; 4]; 3] = [b"dvcC", b"dvvC", b"dvwC"];
    let mp4_cases = ENTRIES.len() * RECORDS.len();
    let pick = pick as usize % (mp4_cases + 2 + CARRIERS.len() * 2);
    if pick < mp4_cases {
        let entry = ENTRIES[pick / RECORDS.len()];
        let record = Mp4Box::leaf(RECORDS[pick % RECORDS.len()], payload.to_vec());
        let (config, sample) = match entry {
            b"avc1" | b"dvav" => (
                Mp4Box::leaf(b"avcC", params::H264_AVCC.to_vec()),
                build::length_prefixed(&[&H264_SPS[..], &H264_PPS[..]]),
            ),
            b"av01" => (
                Mp4Box::leaf(b"av1C", vec![0x81, 0x00, 0x0C, 0x00]),
                vec![0x12, 0x00],
            ),
            _ => (
                Mp4Box::leaf(b"hvcC", params::hevc_hvcc()),
                build::length_prefixed(&[&HEVC_VPS[..], &HEVC_SPS[..], &HEVC_PPS[..]]),
            ),
        };
        let track = Mp4Track::video(entry, vec![config, record], sample, 64, 36);
        return Input::new(build::mp4(&[track]), "mp4");
    }
    let pick = pick - mp4_cases;
    if pick == 0 {
        // BlockAdditionMapping: type 'dvcC' or 'dvvC', the input as extra data.
        let kind = if payload.first().is_some_and(|b| b & 1 == 1) {
            b"dvvC"
        } else {
            b"dvcC"
        };
        let mapping = Element::master(
            0x41E4,
            vec![
                Element::uint(0x41E7, u32::from_be_bytes(*kind) as u64),
                Element::leaf(0x41ED, payload.to_vec()),
            ],
        );
        return Input::new(hevc_mkv(Vec::new(), vec![mapping]), "mkv");
    }
    if pick == 1 {
        // The registration-style DOVI video stream descriptor (tag 0xB0).
        let mut descriptor = vec![0xB0, payload.len().min(255) as u8];
        descriptor.extend_from_slice(&payload[..payload.len().min(255)]);
        return Input::new(
            build::ts_with_es(0x24, &descriptor, &hevc_annexb(&[])),
            "ts",
        );
    }
    let pick = pick - 2;
    let nal_type = if pick.is_multiple_of(2) { 62 } else { 63 };
    nal_stream(
        true,
        nal_type,
        payload,
        CARRIERS[(pick / 2) % CARRIERS.len()],
    )
}

// ---------------------------------------------------------------------------
// Matroska tags
// ---------------------------------------------------------------------------

/// Header bytes of a [`mkv_tags`] input before its payload.
pub const MKV_TAGS_HEADER: usize = 6;
/// Most `TagTrackUID`s across all `Tag` elements of one input.
pub const MKV_TAGS_MAX_TARGETS: usize = 65_536;
/// Most `SimpleTag`s in one `Tag`.
pub const MKV_TAGS_MAX_SIMPLE: usize = 1024;
/// Most value bytes across every `SimpleTag` of one input, so a built input
/// stays a few megabytes whatever the header asks for.
pub const MKV_TAGS_MAX_VALUE_BYTES: usize = 4 << 20;

/// A Matroska file whose `Tags` element is shaped by the input. The header:
///
/// - byte 0: how many `Tag` elements, 1 to 64;
/// - bytes 1-2: `TagTrackUID` targets per `Tag`, little-endian, at most
///   [`MKV_TAGS_MAX_TARGETS`] across all of them;
/// - bytes 3-4: `SimpleTag`s per `Tag`, little-endian, modulo
///   [`MKV_TAGS_MAX_SIMPLE`] + 1;
/// - byte 5: bit 7 names the simple tags `tag-<index>`, all distinct; bits
///   0-6 times 64 is the value length, and 0 makes the value the name.
///
/// The payload picks each target UID (a byte under 0xC0 names the real
/// track, UID 1; otherwise the next byte is the UID) and, unless bit 7 of
/// byte 5 is set, the names (a length byte modulo 16, then that many
/// bytes). Many targets times many simple tags with long values is the
/// shape that makes tag collection multiply, so the memory cap of the fuzz
/// run catches a collection that allocates past its caps.
pub fn mkv_tags(data: &[u8]) -> Input {
    let header = |i: usize| data.get(i).copied().unwrap_or(0);
    let payload = data.get(MKV_TAGS_HEADER..).unwrap_or(&[]);
    let tags = 1 + usize::from(header(0)) % 64;
    let targets =
        usize::from(u16::from_le_bytes([header(1), header(2)])).min(MKV_TAGS_MAX_TARGETS / tags);
    let unique_names = header(5) & 0x80 != 0;
    let value_len = usize::from(header(5) & 0x7F) * 64;
    let simple = (usize::from(u16::from_le_bytes([header(3), header(4)]))
        % (MKV_TAGS_MAX_SIMPLE + 1))
        .min(MKV_TAGS_MAX_VALUE_BYTES / (tags * value_len.max(16)));
    let byte = |i: usize| payload.get(i % payload.len().max(1)).copied().unwrap_or(0);
    let mut at = 0usize;
    let mut tag_elements = Vec::with_capacity(tags);
    for _ in 0..tags {
        let mut target_children = Vec::with_capacity(targets + 1);
        target_children.push(Element::uint(0x68CA, 50));
        for _ in 0..targets {
            let uid = if byte(at) < 0xC0 {
                1
            } else {
                u64::from(byte(at + 1))
            };
            at += 2;
            target_children.push(Element::uint(0x63C5, uid));
        }
        let mut children = Vec::with_capacity(simple + 1);
        children.push(Element::master(0x63C0, target_children));
        for index in 0..simple {
            let name: Vec<u8> = if unique_names {
                format!("tag-{index}").into_bytes()
            } else {
                let len = usize::from(byte(at) % 16);
                let name = (0..len).map(|k| byte(at + 1 + k)).collect();
                at += 1 + len;
                name
            };
            let value = if value_len == 0 {
                name.clone()
            } else {
                (0..value_len).map(|k| b'a' + (k % 26) as u8).collect()
            };
            children.push(Element::master(
                0x67C8,
                vec![Element::leaf(0x45A3, name), Element::leaf(0x4487, value)],
            ));
        }
        tag_elements.push(Element::master(0x7373, children));
    }
    let track = MkvTrack {
        kind: 1,
        codec_id: b"V_MPEG4/ISO/AVC".to_vec(),
        codec_private: Some(params::H264_AVCC.to_vec()),
        media_children: Vec::new(),
        entry_children: Vec::new(),
        frame: build::length_prefixed(&[&H264_SPS[..], &H264_PPS[..]]),
        width: 64,
        height: 64,
    };
    Input::new(
        build::mkv_with(&[track], vec![Element::master(0x1254_C367, tag_elements)]),
        "mkv",
    )
}

/// The [`mkv_tags`] input of the tag flood the parser bounds: one `Tag`
/// naming 40,000 `TagTrackUID`s, most matching no track and every
/// sixteenth repeating the real one, with 300 uniquely named `SimpleTag`s of
/// 4 KiB values. Unbounded, it would keep every value once per matching
/// UID; bounded, the probe keeps at most 256 KiB of tags and warns once.
pub fn mkv_tags_flood_seed() -> Vec<u8> {
    let targets: u16 = 40_000;
    let simple: u16 = 300;
    let mut data = vec![0u8];
    data.extend_from_slice(&targets.to_le_bytes());
    data.extend_from_slice(&simple.to_le_bytes());
    data.push(0x80 | 64);
    // Target pairs: (0xC8, uid) names another UID, (0x00, _) the real one.
    for i in 0..16u8 {
        if i == 15 {
            data.extend_from_slice(&[0x00, 0x00]);
        } else {
            data.extend_from_slice(&[0xC8, 2 + i * 13]);
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_selector_byte_builds_an_input() {
        for pick in 0..=255u8 {
            let data = [pick, 0x41, 0x00, 0x00, 0x03, 0x01, 0xFF];
            // The wrapping targets always produce a container around the payload.
            for input in [
                codec_config(&data),
                bitstream(&data),
                hdr_sei(&data),
                dolby_vision(&data),
            ] {
                assert!(input.bytes.len() > data.len() - 1, "selector {pick}");
            }
            let _ = dispatch(&data);
        }
        for f in [dispatch, codec_config, bitstream, hdr_sei, dolby_vision] {
            let _ = f(&[]);
        }
    }

    #[test]
    fn selectors_reach_every_carrier() {
        let mut extensions = std::collections::BTreeSet::new();
        for pick in 0..=255u8 {
            for f in [codec_config, bitstream, hdr_sei, dolby_vision] {
                extensions.insert(f(&[pick, 1, 2, 3]).extension);
            }
        }
        for ext in ["mp4", "m4a", "mkv", "ts", "h264", "hevc", "m2v"] {
            assert!(extensions.contains(ext), "no selector reaches {ext}");
        }
    }

    #[test]
    fn dispatch_takes_listed_or_literal_extensions() {
        assert_eq!(dispatch(&[0, 9]).extension, "mkv");
        let literal = dispatch(&[EXTENSIONS.len() as u8 + 3, b'a', b'b', b'c', 7]);
        assert_eq!(
            (literal.extension.as_str(), literal.bytes.as_slice()),
            ("abc", &[7u8][..])
        );
    }
}
