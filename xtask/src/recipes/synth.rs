//! Large fixtures synthesised from a small imported fixture plus format
//! padding: an MP4 `mdat` that is mostly a hole, an EBML `Void`, null
//! transport-stream packets, an AVI `JUNK` chunk. They are written sparse
//! where the format allows, so a file of more than 4 GiB takes a few
//! kilobytes of disk on a filesystem with holes, and they are never
//! published: `test-corpus hydrate` writes them from their seeds, and the
//! size and BLAKE3 pinned in each recipe prove the bytes match.
//!
//! The seeds are imported fixtures, whose bytes are pinned, so the output is
//! the same on every machine.

use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::Path;

use crate::corpus::{Context as _, Result, fail};

use super::{Expect, Fixture, Generator, Step};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;

/// How a large fixture is laid out around its seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Synth {
    /// The seed's samples at the end of a 64-bit `mdat` whose first `hole`
    /// bytes are zeros, every chunk offset in a `co64`; `moov` after `mdat`
    /// or, with `moov_first`, before it.
    Mp4 { hole: u64, moov_first: bool },
    /// Matroska with eight-byte sizes on every top-level element: SeekHead,
    /// Info, Tracks, the seed's cluster, a `Void` of `hole` bytes, the same
    /// cluster again one seed-length later, then Cues and Tags, which only
    /// the SeekHead leads to.
    MkvSeekHeadAtEnd { hole: u64 },
    /// Matroska with no SeekHead and no Cues: Info and Tracks, the seed's
    /// cluster, then `clusters` clusters of one audio block behind a 1 MiB
    /// `Void` each, so only a linear walk reaches the end.
    MkvLinear { clusters: u32 },
    /// The seed transport stream twice, its timestamps moved so the 33-bit
    /// PTS and PCR wrap four seconds into the first copy, the second copy
    /// twenty seconds after the first, and `padding` null packets between.
    TsWrap { padding: u64 },
    /// An AVI 1.0 file: the seed with a `JUNK` chunk of `hole` bytes before
    /// `movi`, indexed only by `idx1`.
    AviLegacy { hole: u64 },
    /// An OpenDML AVI: the seed's chunks in `RIFF AVI` behind a `JUNK` of
    /// `first` bytes and again in a `RIFF AVIX` behind a `JUNK` of `second`
    /// bytes, with `ix##` standard indexes, an `indx` super index per stream
    /// and an `odml` header.
    AviOpenDml { first: u64, second: u64 },
}

/// A file as data written at offsets over zeros: what is not written is a
/// hole on a filesystem that supports them.
pub(crate) struct Sparse {
    parts: Vec<(u64, Vec<u8>)>,
    length: u64,
}

impl Sparse {
    fn new() -> Self {
        Sparse {
            parts: Vec::new(),
            length: 0,
        }
    }

    /// Append bytes at the current end.
    fn push(&mut self, bytes: Vec<u8>) -> u64 {
        let at = self.length;
        self.length += bytes.len() as u64;
        if !bytes.is_empty() {
            self.parts.push((at, bytes));
        }
        at
    }

    /// Append `length` zeros, written as a hole.
    fn hole(&mut self, length: u64) {
        self.length += length;
    }

    pub(crate) fn len(&self) -> u64 {
        self.length
    }

    /// Bytes actually written, the rest being holes.
    pub(crate) fn data_bytes(&self) -> u64 {
        self.parts.iter().map(|(_, b)| b.len() as u64).sum()
    }

    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        let mut file =
            std::fs::File::create(path).context(|| format!("create {}", path.display()))?;
        // Extending first leaves the whole file a hole; writing into it then
        // allocates only the data. Writing past the end instead lets some
        // filesystems zero-fill short gaps.
        file.set_len(self.length)
            .context(|| format!("extend {}", path.display()))?;
        for (at, bytes) in &self.parts {
            file.seek(SeekFrom::Start(*at))
                .context(|| format!("seek {}", path.display()))?;
            file.write_all(bytes)
                .context(|| format!("write {}", path.display()))?;
        }
        file.sync_all()
            .context(|| format!("sync {}", path.display()))
    }
}

/// Write `out` from the seed at `seed`, and when `pin` is given check the
/// result's size and BLAKE3 against it; a mismatch removes the file.
/// Returns how many bytes were written as data rather than holes.
pub(crate) fn synthesize(
    synth: Synth,
    seed: &Path,
    out: &Path,
    pin: Option<(u64, &str)>,
) -> Result<u64> {
    let bytes = std::fs::read(seed).context(|| format!("read seed {}", seed.display()))?;
    let file = build(synth, &bytes)?;
    file.write(out)?;
    if let Some((size, blake3)) = pin {
        let (digest, length) = crate::corpus::digest_file(out)?;
        if length != size || digest != blake3 {
            let _ = std::fs::remove_file(out);
            return fail(format!(
                "{} synthesised to {length} bytes, BLAKE3 {digest}; the recipe pins {size} bytes, {blake3}",
                out.display()
            ));
        }
    }
    Ok(file.data_bytes())
}

pub(crate) fn build(synth: Synth, seed: &[u8]) -> Result<Sparse> {
    match synth {
        Synth::Mp4 { hole, moov_first } => mp4(seed, hole, moov_first),
        Synth::MkvSeekHeadAtEnd { hole } => mkv_seekhead_at_end(seed, hole),
        Synth::MkvLinear { clusters } => mkv_linear(seed, clusters),
        Synth::TsWrap { padding } => ts_wrap(seed, padding),
        Synth::AviLegacy { hole } => avi_legacy(seed, hole),
        Synth::AviOpenDml { first, second } => avi_opendml(seed, first, second),
    }
}

fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

fn read_be32(d: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

fn read_le32(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

// MP4 -----------------------------------------------------------------------

struct Mp4Box<'a> {
    kind: [u8; 4],
    /// The whole box, header included.
    raw: &'a [u8],
    header: usize,
}

fn mp4_boxes(d: &[u8]) -> Result<Vec<Mp4Box<'_>>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= d.len() {
        let mut size = read_be32(d, at) as u64;
        let mut header = 8;
        if size == 1 {
            size = u64::from_be_bytes(d[at + 8..at + 16].try_into().unwrap_or_default());
            header = 16;
        } else if size == 0 {
            size = (d.len() - at) as u64;
        }
        let end = at
            .checked_add(size as usize)
            .filter(|e| *e <= d.len() && size >= header as u64);
        let Some(end) = end else {
            return fail("seed MP4: a box runs past its parent");
        };
        out.push(Mp4Box {
            kind: d[at + 4..at + 8].try_into().unwrap_or_default(),
            raw: &d[at..end],
            header,
        });
        at = end;
    }
    Ok(out)
}

fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&be32(body.len() as u32 + 8));
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

/// `moov` with every `stco` turned into a `co64` whose offsets are
/// `old - old_base + new_base`.
fn mp4_rewrite(raw: &[u8], header: usize, old_base: u64, new_base: u64) -> Result<Vec<u8>> {
    let kind: [u8; 4] = raw[4..8].try_into().unwrap_or_default();
    let body = &raw[header..];
    match &kind {
        b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => {
            let mut out = Vec::new();
            for child in mp4_boxes(body)? {
                out.extend(mp4_rewrite(child.raw, child.header, old_base, new_base)?);
            }
            Ok(mp4_box(&kind, &out))
        }
        b"stco" => {
            let count = read_be32(body, 4) as usize;
            if body.len() < 8 + count * 4 {
                return fail("seed MP4: stco is shorter than its count");
            }
            let mut out = body[..8].to_vec();
            for i in 0..count {
                let old = u64::from(read_be32(body, 8 + i * 4));
                let Some(relative) = old.checked_sub(old_base) else {
                    return fail("seed MP4: a chunk starts before mdat");
                };
                out.extend_from_slice(&(new_base + relative).to_be_bytes());
            }
            Ok(mp4_box(b"co64", &out))
        }
        _ => Ok(raw.to_vec()),
    }
}

fn mp4(seed: &[u8], hole: u64, moov_first: bool) -> Result<Sparse> {
    let top = mp4_boxes(seed)?;
    let find = |kind: &[u8; 4]| top.iter().find(|b| &b.kind == kind);
    let (Some(ftyp), Some(mdat), Some(moov)) = (find(b"ftyp"), find(b"mdat"), find(b"moov")) else {
        return fail("seed MP4 needs ftyp, mdat and moov");
    };
    let old_base = (mdat.raw.as_ptr() as usize - seed.as_ptr() as usize + mdat.header) as u64;
    let payload = &mdat.raw[mdat.header..];
    let mdat_header = |len: u64| {
        let mut h = be32(1).to_vec();
        h.extend_from_slice(b"mdat");
        h.extend_from_slice(&(16 + len).to_be_bytes());
        h
    };
    let mut out = Sparse::new();
    out.push(ftyp.raw.to_vec());
    let length = hole + payload.len() as u64;
    if moov_first {
        // co64 sizes do not depend on the offsets, so one pass finds the
        // moov length and a second writes the real offsets.
        let probe = mp4_rewrite(moov.raw, moov.header, old_base, 0)?;
        let samples = out.len() + probe.len() as u64 + 16 + hole;
        out.push(mp4_rewrite(moov.raw, moov.header, old_base, samples)?);
        out.push(mdat_header(length));
        out.hole(hole);
        out.push(payload.to_vec());
    } else {
        let samples = out.len() + 16 + hole;
        out.push(mdat_header(length));
        out.hole(hole);
        out.push(payload.to_vec());
        out.push(mp4_rewrite(moov.raw, moov.header, old_base, samples)?);
    }
    Ok(out)
}

// Matroska ------------------------------------------------------------------

struct Ebml<'a> {
    id: u32,
    /// The element's body.
    body: &'a [u8],
    /// The whole element.
    raw: &'a [u8],
}

fn ebml_vint(d: &[u8], at: usize) -> Option<(u64, usize, bool)> {
    let first = *d.get(at)?;
    let length = first.leading_zeros() as usize + 1;
    if length > 8 || at + length > d.len() {
        return None;
    }
    let mut value = u64::from(first) & ((1u64 << (8 - length)) - 1);
    for byte in &d[at + 1..at + length] {
        value = (value << 8) | u64::from(*byte);
    }
    let unknown = value == (1u64 << (7 * length)) - 1;
    Some((value, length, unknown))
}

fn ebml_children(d: &[u8]) -> Result<Vec<Ebml<'_>>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < d.len() {
        let first = d[at];
        let id_length = first.leading_zeros() as usize + 1;
        if id_length > 4 || at + id_length > d.len() {
            return fail("seed Matroska: bad element id");
        }
        let id = d[at..at + id_length]
            .iter()
            .fold(0u32, |v, b| (v << 8) | u32::from(*b));
        let Some((size, size_length, unknown)) = ebml_vint(d, at + id_length) else {
            return fail("seed Matroska: bad element size");
        };
        let start = at + id_length + size_length;
        let end = if unknown {
            d.len()
        } else {
            start
                .checked_add(size as usize)
                .filter(|e| *e <= d.len())
                .ok_or_else(|| {
                    crate::corpus::error("seed Matroska: element runs past its parent")
                })?
        };
        out.push(Ebml {
            id,
            body: &d[start..end],
            raw: &d[at..end],
        });
        at = end;
    }
    Ok(out)
}

fn ebml_id(id: u32) -> Vec<u8> {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    bytes[skip..].to_vec()
}

/// An element with an eight-byte size.
fn ebml8(id: u32, body: &[u8]) -> Vec<u8> {
    let mut out = ebml_id(id);
    out.push(0x01);
    out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
    out.extend_from_slice(body);
    out
}

/// An element header with an eight-byte size, for a body written later or a
/// hole.
fn ebml8_header(id: u32, length: u64) -> Vec<u8> {
    let mut out = ebml_id(id);
    out.push(0x01);
    out.extend_from_slice(&length.to_be_bytes()[1..]);
    out
}

fn ebml_uint(id: u32, value: u64) -> Vec<u8> {
    ebml8(id, &value.to_be_bytes())
}

struct MkvSeed<'a> {
    header: &'a [u8],
    info: Vec<u8>,
    tracks: &'a [u8],
    tags: Option<&'a [u8]>,
    cluster_timestamp: u64,
    cluster_rest: Vec<&'a [u8]>,
    duration_ticks: f64,
}

const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const TAGS: u32 = 0x1254_C367;
const CUES: u32 = 0x1C53_BB6B;
const CLUSTER: u32 = 0x1F43_B675;
const VOID: u32 = 0xEC;
const DURATION: u32 = 0x4489;
const TIMESTAMP: u32 = 0xE7;

fn mkv_seed(seed: &[u8], duration_scale: f64) -> Result<MkvSeed<'_>> {
    let top = ebml_children(seed)?;
    let (Some(header), Some(segment)) = (
        top.iter().find(|e| e.id == 0x1A45_DFA3),
        top.iter().find(|e| e.id == SEGMENT),
    ) else {
        return fail("seed Matroska needs an EBML header and a Segment");
    };
    let children = ebml_children(segment.body)?;
    let find = |id: u32| children.iter().find(|e| e.id == id);
    let (Some(info), Some(tracks), Some(cluster)) = (find(INFO), find(TRACKS), find(CLUSTER))
    else {
        return fail("seed Matroska needs Info, Tracks and a Cluster");
    };
    if children.iter().filter(|e| e.id == CLUSTER).count() != 1 {
        return fail("seed Matroska must have one cluster");
    }
    // Info with its Duration scaled.
    let mut info_body = Vec::new();
    let mut duration_ticks = 0.0;
    for child in ebml_children(info.body)? {
        if child.id == DURATION {
            duration_ticks = match child.body.len() {
                4 => f64::from(f32::from_be_bytes(
                    child.body.try_into().unwrap_or_default(),
                )),
                8 => f64::from_be_bytes(child.body.try_into().unwrap_or_default()),
                _ => return fail("seed Matroska: Duration is not a float"),
            };
            info_body.extend(ebml8(
                DURATION,
                &(duration_ticks * duration_scale).to_be_bytes(),
            ));
        } else {
            info_body.extend_from_slice(child.raw);
        }
    }
    if duration_ticks <= 0.0 {
        return fail("seed Matroska: Info has no Duration");
    }
    let mut cluster_timestamp = 0;
    let mut cluster_rest = Vec::new();
    for child in ebml_children(cluster.body)? {
        if child.id == TIMESTAMP {
            cluster_timestamp = child
                .body
                .iter()
                .fold(0u64, |v, b| (v << 8) | u64::from(*b));
        } else {
            cluster_rest.push(child.raw);
        }
    }
    Ok(MkvSeed {
        header: header.raw,
        info: ebml8(INFO, &info_body),
        tracks: tracks.raw,
        tags: find(TAGS).map(|e| e.raw),
        cluster_timestamp,
        cluster_rest,
        duration_ticks,
    })
}

fn mkv_cluster(timestamp: u64, blocks: &[&[u8]]) -> Vec<u8> {
    let mut body = ebml_uint(TIMESTAMP, timestamp);
    for block in blocks {
        body.extend_from_slice(block);
    }
    ebml8(CLUSTER, &body)
}

fn mkv_seekhead_at_end(seed: &[u8], hole: u64) -> Result<Sparse> {
    let s = mkv_seed(seed, 2.0)?;
    let Some(tags) = s.tags else {
        return fail("seed Matroska has no Tags");
    };
    let later = s.cluster_timestamp + s.duration_ticks.ceil() as u64;
    let first = mkv_cluster(s.cluster_timestamp, &s.cluster_rest);
    let second = mkv_cluster(later, &s.cluster_rest);
    // The SeekHead's length does not depend on the positions it holds.
    let seek = |id: u32, position: u64| {
        let mut body = ebml8(0x53AB, &ebml_id(id));
        body.extend(ebml_uint(0x53AC, position));
        ebml8(0x4DBB, &body)
    };
    let seekhead_length = ebml8(
        SEEK_HEAD,
        &[seek(INFO, 0), seek(TRACKS, 0), seek(CUES, 0), seek(TAGS, 0)].concat(),
    )
    .len() as u64;
    let void_header = ebml8_header(VOID, hole);
    let info_at = seekhead_length;
    let tracks_at = info_at + s.info.len() as u64;
    let first_at = tracks_at + s.tracks.len() as u64;
    let void_at = first_at + first.len() as u64;
    let second_at = void_at + void_header.len() as u64 + hole;
    let cues_at = second_at + second.len() as u64;
    let cue = |time: u64, position: u64| {
        let positions = [ebml_uint(0xF7, 1), ebml_uint(0xF1, position)].concat();
        ebml8(
            0xBB,
            &[ebml_uint(0xB3, time), ebml8(0xB7, &positions)].concat(),
        )
    };
    let cues = ebml8(
        CUES,
        &[cue(s.cluster_timestamp, first_at), cue(later, second_at)].concat(),
    );
    let tags_at = cues_at + cues.len() as u64;
    let seekhead = ebml8(
        SEEK_HEAD,
        &[
            seek(INFO, info_at),
            seek(TRACKS, tracks_at),
            seek(CUES, cues_at),
            seek(TAGS, tags_at),
        ]
        .concat(),
    );
    let segment_length = tags_at + tags.len() as u64;

    let mut out = Sparse::new();
    out.push(s.header.to_vec());
    out.push(ebml8_header(SEGMENT, segment_length));
    out.push([seekhead, s.info, s.tracks.to_vec(), first, void_header].concat());
    out.hole(hole);
    out.push([second, cues, tags.to_vec()].concat());
    Ok(out)
}

fn mkv_linear(seed: &[u8], clusters: u32) -> Result<Sparse> {
    // Each added cluster holds one AAC frame, 1024 samples at the seed's
    // 48 kHz: about 21 ms at Matroska's default 1 ms timestamp scale.
    const FRAME_TICKS: u64 = 21;
    let seed_ticks = mkv_seed(seed, 1.0)?.duration_ticks.ceil();
    let total = seed_ticks + (u64::from(clusters) * FRAME_TICKS) as f64;
    let s = mkv_seed(seed, total / seed_ticks)?;
    // An audio block: a SimpleBlock of track 2 (the track number is the
    // one-byte vint opening the body), its relative timestamp set to 0 so
    // each copy plays at its cluster's timestamp.
    let mut audio = s
        .cluster_rest
        .iter()
        .find_map(|raw| {
            let element = ebml_children(raw).ok()?;
            let block = element.first()?;
            (block.id == 0xA3 && block.body.first() == Some(&0x82))
                .then(|| (raw.to_vec(), raw.len() - block.body.len()))
        })
        .ok_or_else(|| crate::corpus::error("seed Matroska has no block of track 2"))?;
    audio.0[audio.1 + 1..audio.1 + 3].copy_from_slice(&[0, 0]);
    let audio = audio.0;
    let void_header = ebml8_header(VOID, MIB);
    let seed_end = s.cluster_timestamp + seed_ticks as u64;

    let mut out = Sparse::new();
    out.push(s.header.to_vec());
    // An unknown-size Segment, as a live recording writes it.
    let mut segment = ebml_id(SEGMENT);
    segment.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    out.push(segment);
    out.push(
        [
            s.info.clone(),
            s.tracks.to_vec(),
            mkv_cluster(s.cluster_timestamp, &s.cluster_rest),
        ]
        .concat(),
    );
    for n in 0..u64::from(clusters) {
        let mut body = ebml_uint(TIMESTAMP, seed_end + n * FRAME_TICKS);
        body.extend_from_slice(&void_header);
        out.push(ebml8_header(
            CLUSTER,
            body.len() as u64 + MIB + audio.len() as u64,
        ));
        out.push(body);
        out.hole(MIB);
        out.push(audio.clone());
    }
    Ok(out)
}

// MPEG-TS -------------------------------------------------------------------

const WRAP: u64 = 1 << 33;
const TS: usize = 188;

/// Add `shift` (90 kHz) to every PCR, PTS and DTS of one packet, modulo
/// 2^33, and `cc_shift` to its continuity counter.
fn ts_shift_packet(packet: &mut [u8], shift: u64, cc_shift: u8) -> Result<()> {
    if packet[0] != 0x47 {
        return fail("seed TS: lost packet sync");
    }
    let pid = (u16::from(packet[1] & 0x1F) << 8) | u16::from(packet[2]);
    let start = packet[1] & 0x40 != 0;
    let control = (packet[3] >> 4) & 0x3;
    if pid != 0x1FFF && control & 1 != 0 {
        packet[3] = (packet[3] & 0xF0) | (packet[3].wrapping_add(cc_shift) & 0x0F);
    }
    let mut payload = 4usize;
    if control & 2 != 0 {
        let length = usize::from(packet[4]);
        if length > 0 && packet[5] & 0x10 != 0 {
            let p = &mut packet[6..12];
            let base = (u64::from(p[0]) << 25)
                | (u64::from(p[1]) << 17)
                | (u64::from(p[2]) << 9)
                | (u64::from(p[3]) << 1)
                | (u64::from(p[4]) >> 7);
            let base = (base + shift) % WRAP;
            p[0] = (base >> 25) as u8;
            p[1] = (base >> 17) as u8;
            p[2] = (base >> 9) as u8;
            p[3] = (base >> 1) as u8;
            p[4] = (p[4] & 0x7F) | (((base & 1) as u8) << 7);
        }
        payload = 5 + length;
    }
    if start && control & 1 != 0 && payload + 14 <= TS {
        let pes = &mut packet[payload..];
        let stream_id = pes[3];
        let has_header = pes[0] == 0
            && pes[1] == 0
            && pes[2] == 1
            && !matches!(
                stream_id,
                0xBC | 0xBE | 0xBF | 0xF0 | 0xF1 | 0xF2 | 0xF8 | 0xFF
            );
        if has_header {
            let flags = pes[7] >> 6;
            let mut fix = |at: usize| {
                let t = &mut pes[at..at + 5];
                let value = (u64::from(t[0] >> 1 & 0x07) << 30)
                    | (u64::from(t[1]) << 22)
                    | (u64::from(t[2] >> 1) << 15)
                    | (u64::from(t[3]) << 7)
                    | u64::from(t[4] >> 1);
                let value = (value + shift) % WRAP;
                t[0] = (t[0] & 0xF1) | (((value >> 30) as u8 & 0x07) << 1);
                t[1] = (value >> 22) as u8;
                t[2] = (((value >> 15) as u8) << 1) | 1;
                t[3] = (value >> 7) as u8;
                t[4] = ((value as u8) << 1) | 1;
            };
            if flags & 2 != 0 {
                fix(9);
            }
            if flags == 3 {
                fix(14);
            }
        }
    }
    Ok(())
}

/// The first PCR of the seed, which every copy is shifted against.
fn ts_first_pcr(seed: &[u8]) -> Option<u64> {
    seed.chunks_exact(TS).find_map(|p| {
        let control = (p[3] >> 4) & 0x3;
        (control & 2 != 0 && p[4] > 0 && p[5] & 0x10 != 0).then(|| {
            (u64::from(p[6]) << 25)
                | (u64::from(p[7]) << 17)
                | (u64::from(p[8]) << 9)
                | (u64::from(p[9]) << 1)
                | (u64::from(p[10]) >> 7)
        })
    })
}

fn ts_copy(seed: &[u8], shift: u64, cc_shift: &[u8; 0x2000]) -> Result<Vec<u8>> {
    let mut out = seed.to_vec();
    for packet in out.chunks_exact_mut(TS) {
        let pid = (usize::from(packet[1] & 0x1F) << 8) | usize::from(packet[2]);
        ts_shift_packet(packet, shift, cc_shift[pid])?;
    }
    Ok(out)
}

fn ts_wrap(seed: &[u8], padding: u64) -> Result<Sparse> {
    if !seed.len().is_multiple_of(TS) {
        return fail("seed TS is not 188-byte packets");
    }
    let Some(first_pcr) = ts_first_pcr(seed) else {
        return fail("seed TS carries no PCR");
    };
    // The first copy starts four seconds before the wrap.
    let start = WRAP - 4 * 90_000;
    let shift_a = (start + WRAP - first_pcr) % WRAP;
    let shift_b = (shift_a + 20 * 90_000) % WRAP;
    // The second copy continues every PID's continuity counter.
    let mut counts = [0u8; 0x2000];
    for packet in seed.chunks_exact(TS) {
        let pid = (usize::from(packet[1] & 0x1F) << 8) | usize::from(packet[2]);
        if (packet[3] >> 4) & 1 != 0 {
            counts[pid] = counts[pid].wrapping_add(1);
        }
    }
    let none = [0u8; 0x2000];
    let mut out = Sparse::new();
    out.push(ts_copy(seed, shift_a, &none)?);
    let mut null = [0xFFu8; TS];
    null[..4].copy_from_slice(&[0x47, 0x1F, 0xFF, 0x10]);
    // Null packets are data: every one starts with a sync byte, so the
    // padding is written, in 1 MiB batches.
    let batch: Vec<u8> = null.repeat(5577);
    let mut left = padding;
    while left > 0 {
        let n = left.min(5577);
        out.push(batch[..n as usize * TS].to_vec());
        left -= n;
    }
    out.push(ts_copy(seed, shift_b, &counts)?);
    Ok(out)
}

// AVI -----------------------------------------------------------------------

struct Riff<'a> {
    fourcc: [u8; 4],
    /// For RIFF and LIST, the list type; otherwise empty.
    list: [u8; 4],
    body: &'a [u8],
    raw: &'a [u8],
}

fn riff_chunks(d: &[u8]) -> Result<Vec<Riff<'_>>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= d.len() {
        let fourcc: [u8; 4] = d[at..at + 4].try_into().unwrap_or_default();
        let size = read_le32(d, at + 4) as usize;
        let end = at + 8 + size;
        if end > d.len() {
            return fail("seed AVI: a chunk runs past its parent");
        }
        let is_list = &fourcc == b"RIFF" || &fourcc == b"LIST";
        out.push(Riff {
            fourcc,
            list: if is_list {
                d[at + 8..at + 12].try_into().unwrap_or_default()
            } else {
                [0; 4]
            },
            body: if is_list {
                &d[at + 12..end]
            } else {
                &d[at + 8..end]
            },
            raw: &d[at..end],
        });
        at = end + (size & 1);
    }
    Ok(out)
}

fn chunk(fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = fourcc.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn list(kind: &[u8; 4], children: &[u8]) -> Vec<u8> {
    let mut body = kind.to_vec();
    body.extend_from_slice(children);
    chunk(b"LIST", &body)
}

fn chunk_header(fourcc: &[u8; 4], length: u64) -> Result<Vec<u8>> {
    let Ok(length) = u32::try_from(length) else {
        return fail("AVI chunk over 4 GiB");
    };
    let mut out = fourcc.to_vec();
    out.extend_from_slice(&length.to_le_bytes());
    Ok(out)
}

struct AviSeed<'a> {
    hdrl: &'a [u8],
    info: Option<&'a [u8]>,
    movi: &'a [u8],
    idx1: &'a [u8],
}

fn avi_seed(seed: &[u8]) -> Result<AviSeed<'_>> {
    let top = riff_chunks(seed)?;
    let Some(riff) = top
        .first()
        .filter(|c| &c.fourcc == b"RIFF" && &c.list == b"AVI ")
    else {
        return fail("seed is not RIFF AVI");
    };
    let children = riff_chunks(riff.body)?;
    let list_of = |kind: &[u8; 4]| {
        children
            .iter()
            .find(|c| &c.fourcc == b"LIST" && &c.list == kind)
    };
    let (Some(hdrl), Some(movi), Some(idx1)) = (
        list_of(b"hdrl"),
        list_of(b"movi"),
        children.iter().find(|c| &c.fourcc == b"idx1"),
    ) else {
        return fail("seed AVI needs hdrl, movi and idx1");
    };
    Ok(AviSeed {
        hdrl: hdrl.body,
        info: list_of(b"INFO").map(|c| c.raw),
        movi: movi.body,
        idx1: idx1.raw,
    })
}

fn avi_legacy(seed: &[u8], hole: u64) -> Result<Sparse> {
    let s = avi_seed(seed)?;
    let hdrl = list(b"hdrl", s.hdrl);
    let info = s.info.unwrap_or_default();
    let movi = list(b"movi", s.movi);
    let junk = chunk_header(b"JUNK", hole)?;
    let length = 4 + hdrl.len() + info.len() + junk.len() + movi.len() + s.idx1.len();
    let mut out = Sparse::new();
    out.push(
        [
            chunk_header(b"RIFF", length as u64 + hole)?,
            b"AVI ".to_vec(),
            hdrl,
            info.to_vec(),
            junk,
        ]
        .concat(),
    );
    out.hole(hole);
    // idx1 offsets are relative to `movi`, so they hold wherever it lands.
    out.push([movi, s.idx1.to_vec()].concat());
    Ok(out)
}

/// One data chunk of an AVI `movi` list: stream number, fourcc, body.
type DataChunk<'a> = (usize, [u8; 4], &'a [u8]);

/// The seed's data chunks in order.
fn avi_data_chunks(movi: &[u8]) -> Result<Vec<DataChunk<'_>>> {
    let mut out = Vec::new();
    for c in riff_chunks(movi)? {
        if &c.fourcc == b"LIST" {
            for inner in riff_chunks(c.body)? {
                out.push(inner);
            }
            continue;
        }
        out.push(c);
    }
    out.into_iter()
        .filter(|c| c.fourcc[0].is_ascii_digit() && c.fourcc[1].is_ascii_digit())
        .map(|c| {
            let stream = usize::from(c.fourcc[0] - b'0') * 10 + usize::from(c.fourcc[1] - b'0');
            Ok((stream, c.fourcc, c.body))
        })
        .collect()
}

/// Key frames by data-chunk position, from idx1 (AVIIF_KEYFRAME).
fn avi_keyframes(idx1: &[u8]) -> Vec<bool> {
    idx1[8..]
        .chunks_exact(16)
        .map(|e| read_le32(e, 4) & 0x10 != 0)
        .collect()
}

/// One `movi` list of the data chunks after `hole` bytes of `JUNK`, followed
/// by an `ix##` per stream. `base` is the absolute file offset of the list.
/// Returns the list, the hole's position in it, and for each stream the
/// absolute offset and length of its `ix##` chunk and its chunk count.
#[allow(clippy::type_complexity)]
fn avi_movi(
    chunks: &[(usize, [u8; 4], &[u8])],
    keyframes: &[bool],
    streams: usize,
    hole: u64,
    base: u64,
) -> Result<(Vec<u8>, Vec<u8>, Vec<(u64, u32, u32)>)> {
    // Layout: LIST size 'movi' | JUNK header | hole | chunks | ix00 ix01 ...
    let junk = chunk_header(b"JUNK", hole)?;
    let data_start = base + 12 + junk.len() as u64 + hole;
    let mut data = Vec::new();
    let mut entries: Vec<Vec<(u32, u32)>> = vec![Vec::new(); streams];
    for (i, (stream, fourcc, body)) in chunks.iter().enumerate() {
        let at = data_start + data.len() as u64;
        data.extend(chunk(fourcc, body));
        // ix## entries point at the data, past the chunk header, relative
        // to the index's base offset (data_start); bit 31 set is a delta
        // (non-key) frame.
        let key = keyframes.get(i).copied().unwrap_or(true);
        let size = body.len() as u32 | if key { 0 } else { 0x8000_0000 };
        entries[*stream].push(((at + 8 - data_start) as u32, size));
    }
    let mut indexes = Vec::new();
    let mut located = Vec::new();
    for (stream, list) in entries.iter().enumerate() {
        let id = chunks
            .iter()
            .find(|c| c.0 == stream)
            .map(|c| c.1)
            .unwrap_or(*b"00dc");
        let mut body = Vec::new();
        body.extend_from_slice(&2u16.to_le_bytes()); // wLongsPerEntry
        body.push(0); // bIndexSubType
        body.push(1); // bIndexType: AVI_INDEX_OF_CHUNKS
        body.extend_from_slice(&(list.len() as u32).to_le_bytes());
        body.extend_from_slice(&id);
        body.extend_from_slice(&data_start.to_le_bytes()); // qwBaseOffset
        body.extend_from_slice(&0u32.to_le_bytes());
        for (offset, size) in list {
            body.extend_from_slice(&offset.to_le_bytes());
            body.extend_from_slice(&size.to_le_bytes());
        }
        let fourcc = [
            b'i',
            b'x',
            b'0' + (stream / 10) as u8,
            b'0' + (stream % 10) as u8,
        ];
        let at = data_start + data.len() as u64 + indexes.len() as u64;
        let c = chunk(&fourcc, &body);
        located.push((at, c.len() as u32, list.len() as u32));
        indexes.extend(c);
    }
    let after = [data, indexes].concat();
    let header = [
        chunk_header(b"LIST", 4 + junk.len() as u64 + hole + after.len() as u64)?,
        b"movi".to_vec(),
        junk,
    ]
    .concat();
    Ok((header, after, located))
}

fn avi_opendml(seed: &[u8], first: u64, second: u64) -> Result<Sparse> {
    let s = avi_seed(seed)?;
    let chunks = avi_data_chunks(s.movi)?;
    let keyframes = avi_keyframes(s.idx1);
    let streams = chunks.iter().map(|c| c.0 + 1).max().unwrap_or(0);
    let frames_per_stream: Vec<u32> = (0..streams)
        .map(|n| chunks.iter().filter(|c| c.0 == n).count() as u32)
        .collect();

    // hdrl rewritten: each strl's reserved JUNK becomes an `indx` of two
    // entries, each strh's length doubles, and the hdrl JUNK becomes the
    // `odml` list. Its length is fixed, so offsets can be computed first.
    let rebuild_hdrl = |ix: &[Vec<(u64, u32, u32)>]| -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut stream = 0usize;
        for c in riff_chunks(s.hdrl)? {
            if &c.fourcc == b"LIST" && &c.list == b"strl" {
                let mut strl = Vec::new();
                let mut wrote_index = false;
                let mut id = *b"00dc";
                for inner in riff_chunks(c.body)? {
                    match &inner.fourcc {
                        b"strh" => {
                            let mut body = inner.body.to_vec();
                            let length = read_le32(&body, 32);
                            body[32..36].copy_from_slice(&(length * 2).to_le_bytes());
                            if &body[0..4] == b"auds" {
                                id = [
                                    b'0' + (stream / 10) as u8,
                                    b'0' + (stream % 10) as u8,
                                    b'w',
                                    b'b',
                                ];
                            } else {
                                id = [
                                    b'0' + (stream / 10) as u8,
                                    b'0' + (stream % 10) as u8,
                                    b'd',
                                    b'c',
                                ];
                            }
                            strl.extend(chunk(b"strh", &body));
                        }
                        b"JUNK" if !wrote_index => {
                            let mut body = vec![0u8; inner.body.len()];
                            body[0..2].copy_from_slice(&4u16.to_le_bytes());
                            body[2] = 0;
                            body[3] = 0; // AVI_INDEX_OF_INDEXES
                            let entries = ix.get(stream).cloned().unwrap_or_default();
                            body[4..8].copy_from_slice(&(entries.len() as u32).to_le_bytes());
                            body[8..12].copy_from_slice(&id);
                            for (n, (at, length, duration)) in entries.iter().enumerate() {
                                let e = 24 + n * 16;
                                body[e..e + 8].copy_from_slice(&at.to_le_bytes());
                                body[e + 8..e + 12].copy_from_slice(&length.to_le_bytes());
                                body[e + 12..e + 16].copy_from_slice(&duration.to_le_bytes());
                            }
                            strl.extend(chunk(b"indx", &body));
                            wrote_index = true;
                        }
                        _ => strl.extend_from_slice(inner.raw),
                    }
                }
                if !wrote_index {
                    return fail("seed AVI: a strl has no reserved JUNK for indx");
                }
                out.extend(list(b"strl", &strl));
                stream += 1;
            } else if &c.fourcc == b"JUNK" {
                // dmlh: dwTotalFrames, then reserved.
                let mut dmlh = vec![0u8; c.body.len() - 12];
                dmlh[0..4].copy_from_slice(
                    &(frames_per_stream.first().copied().unwrap_or(0) * 2).to_le_bytes(),
                );
                out.extend(list(b"odml", &chunk(b"dmlh", &dmlh)));
            } else {
                out.extend_from_slice(c.raw);
            }
        }
        Ok(list(b"hdrl", &out))
    };
    let placeholder = vec![vec![(0, 0, 0); 2]; streams];
    let hdrl_length = rebuild_hdrl(&placeholder)?.len() as u64;
    let info = s.info.unwrap_or_default();

    let movi1_at = 12 + hdrl_length + info.len() as u64;
    let (movi1_head, movi1_tail, ix1) = avi_movi(&chunks, &keyframes, streams, first, movi1_at)?;
    let riff1_length = 4
        + hdrl_length
        + info.len() as u64
        + movi1_head.len() as u64
        + first
        + movi1_tail.len() as u64
        + s.idx1.len() as u64;
    let riff2_at = 8 + riff1_length;
    let movi2_at = riff2_at + 12;
    let (movi2_head, movi2_tail, ix2) = avi_movi(&chunks, &keyframes, streams, second, movi2_at)?;
    let riff2_length = 4 + movi2_head.len() as u64 + second + movi2_tail.len() as u64;

    let super_index: Vec<Vec<(u64, u32, u32)>> =
        (0..streams).map(|n| vec![ix1[n], ix2[n]]).collect();
    let hdrl = rebuild_hdrl(&super_index)?;
    if hdrl.len() as u64 != hdrl_length {
        return fail("hdrl length changed between passes");
    }

    let mut out = Sparse::new();
    out.push(
        [
            chunk_header(b"RIFF", riff1_length)?,
            b"AVI ".to_vec(),
            hdrl,
            info.to_vec(),
            movi1_head,
        ]
        .concat(),
    );
    out.hole(first);
    // idx1 covers the first RIFF only, as AVI 1.0 readers expect.
    out.push([movi1_tail, s.idx1.to_vec()].concat());
    out.push(
        [
            chunk_header(b"RIFF", riff2_length)?,
            b"AVIX".to_vec(),
            movi2_head,
        ]
        .concat(),
    );
    out.hole(second);
    out.push(movi2_tail);
    Ok(out)
}

// Recipes -------------------------------------------------------------------

struct Pin {
    name: &'static str,
    seed: &'static str,
    synth: Synth,
    size: u64,
    blake3: &'static str,
    adjust: fn(&mut Expect),
}

fn doubled(e: &mut Expect) {
    e.min_duration_seconds = 2;
}

fn keep(_: &mut Expect) {}

const PINS: &[Pin] = &[
    Pin {
        name: "large/mp4_co64_over_4g.mp4",
        seed: "simd_dense_h264_aac.mp4",
        synth: Synth::Mp4 {
            hole: 4 * GIB + MIB,
            moov_first: false,
        },
        size: 4296050916,
        blake3: "80bcda25c94b69eae8cbd4fd17f5cedecf5c1c0cbcd5a618e5135b596ee7d5e3",
        adjust: keep,
    },
    Pin {
        name: "large/mp4_co64_100m.mp4",
        seed: "simd_dense_h264_aac.mp4",
        synth: Synth::Mp4 {
            hole: 100 * MIB,
            moov_first: false,
        },
        size: 104892644,
        blake3: "f6c5061944f03d4fdcdb3569c6a40be2d6a6aaa5eacd54a6d886f65504beedc7",
        adjust: keep,
    },
    Pin {
        name: "large/mp4_moov_first_300m.mp4",
        seed: "simd_dense_h264_aac.mp4",
        synth: Synth::Mp4 {
            hole: 300 * MIB,
            moov_first: true,
        },
        size: 314607844,
        blake3: "e127d52d0a2847bcf30dce844a5bb9285d6226a3e1000104dbbccfdc861c7c43",
        adjust: keep,
    },
    Pin {
        name: "large/mkv_seekhead_cues_tags_over_4g.mkv",
        seed: "h264_aac.mkv",
        synth: Synth::MkvSeekHeadAtEnd {
            hole: 4 * GIB + MIB,
        },
        size: 4296052667,
        blake3: "320482ff378df7b897661e1f481c86f9288e6cbe8b480f9fb4a065b5d171d0fe",
        adjust: doubled,
    },
    Pin {
        name: "large/mkv_seekhead_cues_tags_100m.mkv",
        seed: "h264_aac.mkv",
        synth: Synth::MkvSeekHeadAtEnd { hole: 100 * MIB },
        size: 104894395,
        blake3: "a5f93c212892a980ec2ae7849c675aa94d77bd5978205570a87fcd092a76ed2b",
        adjust: doubled,
    },
    Pin {
        name: "large/mkv_no_seekhead_no_cues_300m.mkv",
        seed: "h264_aac.mkv",
        synth: Synth::MkvLinear { clusters: 300 },
        size: 314706010,
        blake3: "7106684e85496eda26e5e9e59ffb514578c3a451d37862e4f0ead19fc51f8da0",
        adjust: |e| e.min_duration_seconds = 7,
    },
    Pin {
        name: "large/ts_pts_pcr_wrap_300m.ts",
        seed: "simd_dense_h264_aac.ts",
        synth: Synth::TsWrap {
            padding: 300 * MIB / 188,
        },
        size: 315024268,
        blake3: "f9f4e16b3073c0369e777171f7338c050aa5ff6de8b7d5d115e8d682ca957b26",
        adjust: |e| e.min_duration_seconds = 28,
    },
    Pin {
        name: "large/avi_idx1_over_1g.avi",
        seed: "matrix_avi_002.avi",
        synth: Synth::AviLegacy { hole: GIB + MIB },
        size: 1075620214,
        blake3: "8104d84df2d806dd4d3b82b23ec6851f0df8319ddbceea6bc9856cd5caa502da",
        adjust: keep,
    },
    Pin {
        name: "large/avi_opendml_over_2g.avi",
        seed: "matrix_avi_002.avi",
        synth: Synth::AviOpenDml {
            first: GIB - 2 * MIB,
            second: GIB + 64 * MIB,
        },
        size: 2214146150,
        blake3: "89c44fb434388438d991be15de4123ea3e30d783f59c093838bb0d6aa7250f03",
        adjust: doubled,
    },
];

/// The synthesised fixtures, their expectations those of their seed among
/// `existing`, adjusted.
pub(crate) fn fixtures(existing: &[Fixture]) -> Vec<Fixture> {
    PINS.iter()
        .map(|pin| {
            let seed = existing
                .iter()
                .find(|f| f.name == pin.seed)
                .unwrap_or_else(|| {
                    panic!("{} seeds from {}, which has no recipe", pin.name, pin.seed)
                });
            assert_eq!(
                seed.generator,
                Generator::Imported,
                "{}: seeds must be imported",
                pin.name
            );
            let mut expect = seed.expect.clone();
            (pin.adjust)(&mut expect);
            Fixture {
                name: pin.name.to_owned(),
                generator: Generator::Synthesized,
                steps: vec![Step::Synthesize {
                    synth: pin.synth,
                    seed: pin.seed,
                    size: pin.size,
                    blake3: pin.blake3,
                }],
                expect,
                encoders: Vec::new(),
            }
        })
        .collect()
}

/// The pinned size and digest of a synthesised fixture, if it is one.
pub(crate) fn pinned(name: &str) -> Option<(u64, &'static str)> {
    PINS.iter()
        .find(|p| p.name == name)
        .map(|p| (p.size, p.blake3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_files_keep_their_length_and_data() {
        let mut s = Sparse::new();
        s.push(b"head".to_vec());
        s.hole(1000);
        s.push(b"tail".to_vec());
        assert_eq!(s.len(), 1008);
        assert_eq!(s.data_bytes(), 8);
        let dir = std::env::temp_dir().join(format!("xtask-sparse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f");
        s.write(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 1008);
        assert_eq!(&bytes[..4], b"head");
        assert!(bytes[4..1004].iter().all(|b| *b == 0));
        assert_eq!(&bytes[1004..], b"tail");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ts_timestamps_wrap_modulo_two_to_the_33() {
        let mut packet = [0u8; TS];
        packet[..4].copy_from_slice(&[0x47, 0x41, 0x00, 0x30]);
        packet[4] = 7;
        packet[5] = 0x10;
        // PCR base WRAP - 1.
        let base = WRAP - 1;
        packet[6] = (base >> 25) as u8;
        packet[7] = (base >> 17) as u8;
        packet[8] = (base >> 9) as u8;
        packet[9] = (base >> 1) as u8;
        packet[10] = ((base & 1) as u8) << 7 | 0x7E;
        ts_shift_packet(&mut packet, 2, 0).unwrap();
        assert_eq!(ts_first_pcr(&packet), Some(1));
    }

    #[test]
    fn every_pin_names_an_imported_seed_and_a_large_path() {
        let fixtures = super::super::all();
        for pin in PINS {
            assert!(pin.name.starts_with("large/"), "{}", pin.name);
            assert!(
                fixtures
                    .iter()
                    .any(|f| f.name == pin.seed && f.generator == Generator::Imported),
                "{}",
                pin.seed
            );
            assert!(
                crate::corpus::is_blake3_hex(pin.blake3),
                "{} is not pinned",
                pin.name
            );
            assert!(pin.size > 0, "{}", pin.name);
        }
    }
}
