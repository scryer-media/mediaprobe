//! Deterministic Rust passes over files FFmpeg has already written, for the
//! few signals FFmpeg's command line cannot ask for. Each pass edits
//! container structure only; no encoded payload byte is changed.

use std::path::Path;

use crate::corpus::{Context as _, Result, fail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Post {
    /// Rewrite an ASF PCM stream's WAVEFORMATEX as WAVE_FORMAT_EXTENSIBLE.
    AsfWaveExtensible,
    /// Copy another fixture's 188-byte TS packets into wider packets with the
    /// sync byte at `sync_offset`.
    TsReframe {
        source: String,
        packet_size: usize,
        sync_offset: usize,
    },
    /// Insert a Dolby Vision configuration box (`dvcC` for profiles up to 7,
    /// `dvvC` for 8 and up) into the first HEVC sample entry of an MP4 whose
    /// `moov` follows `mdat`.
    DolbyVisionConfig(DoviRecord),
    /// The same, on an intermediate file in the work directory.
    DolbyVisionConfigAt { path: String, record: DoviRecord },
    /// Insert a `free` box of `bytes` payload as the first child of `moov`,
    /// so the movie header outgrows a read budget. `moov` must follow `mdat`.
    GrowMoov { bytes: u64 },
    /// Add an extended language box (`elng`) holding `tag` to the first
    /// audio track. FFmpeg's MP4 muxer writes only the packed ISO 639-2 code
    /// of `mdhd`, which cannot hold a BCP 47 tag. `moov` must follow `mdat`.
    Mp4ExtendedLanguage { tag: String },
    /// Insert a Dolby Vision configuration box into the first video sample
    /// entry (HEVC, AVC or AV1) of `path` (`{out}` or a `{work}` file), with
    /// an explicit box type, optionally renaming the sample entry (for
    /// example `hvc1` to `dvh1`).
    DoviBox {
        path: String,
        record: DoviRecord,
        box_type: [u8; 4],
        rename: Option<[u8; 4]>,
    },
    /// Author bitstream units into `from`, writing `to` (see inject.rs).
    Inject {
        from: String,
        to: String,
        inject: super::inject::Inject,
    },
    /// Author a disc image around payload files in the work directory (see disc.rs).
    Disc(Box<super::disc::Disc>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DoviRecord {
    pub(crate) profile: u8,
    pub(crate) level: u8,
    pub(crate) rpu: bool,
    pub(crate) el: bool,
    pub(crate) bl: bool,
    pub(crate) bl_compat_id: u8,
}

impl DoviRecord {
    /// The 24-byte DOVIDecoderConfigurationRecord (ETSI GS CCM 001, 4.2).
    pub(crate) fn bytes(self) -> [u8; 24] {
        let mut record = [0u8; 24];
        record[0] = 1; // dv_version_major
        record[1] = 0; // dv_version_minor
        let packed: u16 = (u16::from(self.profile & 0x7f) << 9)
            | (u16::from(self.level & 0x3f) << 3)
            | (u16::from(self.rpu) << 2)
            | (u16::from(self.el) << 1)
            | u16::from(self.bl);
        record[2..4].copy_from_slice(&packed.to_be_bytes());
        record[4] = (self.bl_compat_id & 0x0f) << 4;
        record
    }

    pub(crate) fn box_type(self) -> &'static [u8; 4] {
        if self.profile > 7 { b"dvvC" } else { b"dvcC" }
    }
}

fn resolve(path: &str, out: &Path, work: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(
        path.replace("{out}", &out.to_string_lossy())
            .replace("{work}", &work.to_string_lossy()),
    )
}

pub(crate) fn apply(post: &Post, out: &Path, media_root: &Path, work: &Path) -> Result<()> {
    match post {
        Post::DoviBox {
            path,
            record,
            box_type,
            rename,
        } => {
            let target = resolve(path, out, work);
            let data = std::fs::read(&target).context(|| format!("read {}", target.display()))?;
            let mut child = Vec::with_capacity(32);
            child.extend_from_slice(&32u32.to_be_bytes());
            child.extend_from_slice(box_type);
            child.extend_from_slice(&record.bytes());
            let edited = mp4_insert_video(&data, &child, *rename)?;
            crate::corpus::write_atomic(&target, &edited)
        }
        Post::Inject { from, to, inject } => {
            let source = resolve(from, out, work);
            let data = std::fs::read(&source).context(|| format!("read {}", source.display()))?;
            let authored = super::inject::apply(inject, &data)?;
            crate::corpus::write_atomic(&resolve(to, out, work), &authored)
        }
        Post::Disc(disc) => crate::corpus::write_atomic(out, &super::disc::author(disc, work)?),
        Post::DolbyVisionConfigAt { path, record } => {
            let target = std::path::PathBuf::from(path.replace("{work}", &work.to_string_lossy()));
            apply(&Post::DolbyVisionConfig(*record), &target, media_root, work)
        }
        Post::AsfWaveExtensible => {
            let mut data = std::fs::read(out).context(|| format!("read {}", out.display()))?;
            asf_wave_extensible(&mut data)?;
            crate::corpus::write_atomic(out, &data)
        }
        Post::TsReframe {
            source,
            packet_size,
            sync_offset,
        } => {
            let path = media_root.join(source);
            let data = std::fs::read(&path).context(|| format!("read {}", path.display()))?;
            let framed = ts_reframe(&data, *packet_size, *sync_offset)?;
            crate::corpus::write_atomic(out, &framed)
        }
        Post::DolbyVisionConfig(record) => {
            let data = std::fs::read(out).context(|| format!("read {}", out.display()))?;
            let mut child = Vec::with_capacity(32);
            child.extend_from_slice(&32u32.to_be_bytes());
            child.extend_from_slice(record.box_type());
            child.extend_from_slice(&record.bytes());
            let edited = mp4_insert(
                &data,
                &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"],
                SampleEntry::Hevc,
                &child,
            )?;
            crate::corpus::write_atomic(out, &edited)
        }
        Post::Mp4ExtendedLanguage { tag } => {
            let data = std::fs::read(out).context(|| format!("read {}", out.display()))?;
            crate::corpus::write_atomic(out, &mp4_extended_language(&data, tag)?)
        }
        Post::GrowMoov { bytes } => {
            let data = std::fs::read(out).context(|| format!("read {}", out.display()))?;
            let total = bytes + 8;
            let size =
                u32::try_from(total).map_err(|_| crate::corpus::error("free box over 4 GiB"))?;
            let mut child = Vec::with_capacity(total as usize);
            child.extend_from_slice(&size.to_be_bytes());
            child.extend_from_slice(b"free");
            child.resize(total as usize, 0);
            let edited = mp4_insert(&data, &[b"moov"], SampleEntry::None, &child)?;
            crate::corpus::write_atomic(out, &edited)
        }
    }
}

pub(crate) fn ts_reframe(data: &[u8], packet_size: usize, sync_offset: usize) -> Result<Vec<u8>> {
    if sync_offset + 188 > packet_size {
        return fail("TS sync offset does not fit the packet");
    }
    let usable = data.len() - data.len() % 188;
    if usable == 0 {
        return fail("source has no TS packets");
    }
    let mut out = Vec::with_capacity(usable / 188 * packet_size);
    for (index, packet) in data[..usable].chunks_exact(188).enumerate() {
        if packet[0] != 0x47 {
            return fail(format!("source lost TS sync at packet {index}"));
        }
        let start = out.len();
        out.resize(start + packet_size, 0);
        out[start + sync_offset..start + sync_offset + 188].copy_from_slice(packet);
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SampleEntry {
    /// Insert as the first child of the outermost box on the path.
    None,
    /// Descend from `stsd` into the first hvc1/hev1/dvh1/dvhe entry and
    /// append the child after its existing children.
    Hevc,
    /// The same for any HEVC, AVC or AV1 sample entry.
    Video,
}

const HEVC_ENTRIES: &[&[u8; 4]] = &[b"hvc1", b"hev1", b"dvh1", b"dvhe"];
const VIDEO_ENTRIES: &[&[u8; 4]] = &[
    b"hvc1", b"hev1", b"dvh1", b"dvhe", b"avc1", b"avc3", b"dva1", b"dvav", b"av01", b"dav1",
];

const STSD_PATH: &[&[u8; 4]] = &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"];

/// Insert `child` into the first video sample entry of the first track that
/// has one, optionally renaming that entry.
fn mp4_insert_video(data: &[u8], child: &[u8], rename: Option<[u8; 4]>) -> Result<Vec<u8>> {
    let mut out = mp4_insert(data, STSD_PATH, SampleEntry::Video, child)?;
    if let Some(fourcc) = rename {
        // The entry now ends with the child; find it again by its content.
        let at = out
            .windows(child.len())
            .position(|w| w == child)
            .ok_or_else(|| crate::corpus::error("inserted box vanished"))?;
        let entry = sample_entry_start(&out, at)?;
        out[entry + 4..entry + 8].copy_from_slice(&fourcc);
    }
    Ok(out)
}

/// The start of the sample entry whose children include the box at `child_at`.
fn sample_entry_start(data: &[u8], child_at: usize) -> Result<usize> {
    let mut at = child_at;
    while at >= 8 {
        at -= 1;
        let tag = &data[at + 4..at + 8];
        if VIDEO_ENTRIES.iter().any(|e| e.as_slice() == tag) {
            let size = be32(data, at)? as usize;
            if at + size >= child_at {
                return Ok(at);
            }
        }
    }
    fail("no enclosing sample entry")
}

fn be32(data: &[u8], at: usize) -> Result<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap_or_default()))
        .ok_or_else(|| crate::corpus::error(format!("truncated box header at {at}")))
}

/// Find `name` among the boxes in `data[start..end]`; returns (offset, size).
fn find_box(
    data: &[u8],
    start: usize,
    end: usize,
    name: &[u8; 4],
) -> Result<Option<(usize, usize)>> {
    let mut at = start;
    while at + 8 <= end {
        let size = be32(data, at)? as usize;
        if size < 8 || at + size > end {
            return fail(format!(
                "box at {at} has size {size}, which a 32-bit post pass cannot edit"
            ));
        }
        if &data[at + 4..at + 8] == name {
            return Ok(Some((at, size)));
        }
        at += size;
    }
    Ok(None)
}

/// Insert `child` along `path` and grow every enclosing box. Only valid when
/// nothing after the insertion point is referenced by absolute offset, which
/// holds when `moov` follows `mdat` (FFmpeg's default without faststart).
fn mp4_insert(data: &[u8], path: &[&[u8; 4]], entry: SampleEntry, child: &[u8]) -> Result<Vec<u8>> {
    let mut ancestors = Vec::new();
    let (mut start, mut end) = (0usize, data.len());
    let mut mdat_seen = false;
    if let Some((mdat, _)) = find_box(data, 0, data.len(), b"mdat")? {
        mdat_seen = true;
        let (moov, _) = find_box(data, 0, data.len(), b"moov")?
            .ok_or_else(|| crate::corpus::error("no moov box"))?;
        if moov < mdat {
            return fail("moov precedes mdat: inserting would move chunk offsets");
        }
    }
    if !mdat_seen {
        return fail("no mdat box");
    }
    for (depth, name) in path.iter().enumerate() {
        let (offset, size) = find_box(data, start, end, name)?.ok_or_else(|| {
            crate::corpus::error(format!("no {} box", String::from_utf8_lossy(*name)))
        })?;
        ancestors.push(offset);
        start = offset + 8;
        end = offset + size;
        // stsd is a full box with an entry count before its entries.
        if **name == *b"stsd" && depth == path.len() - 1 {
            start += 8;
        }
    }
    let insert_at = match entry {
        SampleEntry::None => start,
        SampleEntry::Hevc | SampleEntry::Video => {
            let names = if entry == SampleEntry::Hevc {
                HEVC_ENTRIES
            } else {
                VIDEO_ENTRIES
            };
            let mut found = None;
            for name in names {
                if let Some(hit) = find_box(data, start, end, name)? {
                    found = Some(hit);
                    break;
                }
            }
            let (offset, size) =
                found.ok_or_else(|| crate::corpus::error("no matching video sample entry"))?;
            ancestors.push(offset);
            offset + size
        }
    };
    let grow = u32::try_from(child.len()).map_err(|_| crate::corpus::error("child too large"))?;
    let mut out = Vec::with_capacity(data.len() + child.len());
    out.extend_from_slice(&data[..insert_at]);
    out.extend_from_slice(child);
    out.extend_from_slice(&data[insert_at..]);
    for offset in ancestors {
        let size = be32(&out, offset)?
            .checked_add(grow)
            .ok_or_else(|| crate::corpus::error("box size overflow"))?;
        out[offset..offset + 4].copy_from_slice(&size.to_be_bytes());
    }
    Ok(out)
}

/// Append an `elng` box to the media box of the first audio track and grow
/// the boxes around it. Valid under the same condition as [`mp4_insert`].
fn mp4_extended_language(data: &[u8], tag: &str) -> Result<Vec<u8>> {
    let (moov, moov_size) = find_box(data, 0, data.len(), b"moov")?
        .ok_or_else(|| crate::corpus::error("no moov box"))?;
    match find_box(data, 0, data.len(), b"mdat")? {
        Some((mdat, _)) if mdat < moov => {}
        _ => return fail("moov must follow mdat: inserting would move chunk offsets"),
    }
    let moov_end = moov + moov_size;
    let mut at = moov + 8;
    while let Some((trak, trak_size)) = find_box(data, at, moov_end, b"trak")? {
        at = trak + trak_size;
        let Some((mdia, mdia_size)) = find_box(data, trak + 8, at, b"mdia")? else {
            continue;
        };
        let mdia_end = mdia + mdia_size;
        let Some((hdlr, _)) = find_box(data, mdia + 8, mdia_end, b"hdlr")? else {
            continue;
        };
        // Full-box header, pre_defined, then the handler type.
        if data.get(hdlr + 16..hdlr + 20) != Some(&b"soun"[..]) {
            continue;
        }
        let size = u32::try_from(8 + 4 + tag.len() + 1)
            .map_err(|_| crate::corpus::error("language tag too long"))?;
        let mut child = Vec::with_capacity(size as usize);
        child.extend_from_slice(&size.to_be_bytes());
        child.extend_from_slice(b"elng");
        child.extend_from_slice(&[0; 4]);
        child.extend_from_slice(tag.as_bytes());
        child.push(0);
        let mut out = Vec::with_capacity(data.len() + child.len());
        out.extend_from_slice(&data[..mdia_end]);
        out.extend_from_slice(&child);
        out.extend_from_slice(&data[mdia_end..]);
        for offset in [moov, trak, mdia] {
            let grown = be32(&out, offset)?
                .checked_add(size)
                .ok_or_else(|| crate::corpus::error("box size overflow"))?;
            out[offset..offset + 4].copy_from_slice(&grown.to_be_bytes());
        }
        return Ok(out);
    }
    fail("no audio track")
}

fn le16(data: &[u8], at: usize) -> Result<u16> {
    data.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| crate::corpus::error(format!("truncated ASF field at {at}")))
}

fn le32(data: &[u8], at: usize) -> Result<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap_or_default()))
        .ok_or_else(|| crate::corpus::error(format!("truncated ASF field at {at}")))
}

fn le64(data: &[u8], at: usize) -> Result<u64> {
    data.get(at..at + 8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()))
        .ok_or_else(|| crate::corpus::error(format!("truncated ASF field at {at}")))
}

fn put(data: &mut [u8], at: usize, value: &[u8]) -> Result<()> {
    data.get_mut(at..at + value.len())
        .ok_or_else(|| crate::corpus::error(format!("truncated ASF field at {at}")))?
        .copy_from_slice(value);
    Ok(())
}

const ASF_FILE_PROPERTIES: [u8; 16] = [
    0xa1, 0xdc, 0xab, 0x8c, 0x47, 0xa9, 0xcf, 0x11, 0x8e, 0xe4, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65,
];
const ASF_STREAM_PROPERTIES: [u8; 16] = [
    0x91, 0x07, 0xdc, 0xb7, 0xb7, 0xa9, 0xcf, 0x11, 0x8e, 0xe6, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65,
];
pub(crate) const ASF_AUDIO_MEDIA: [u8; 16] = [
    0x40, 0x9e, 0x69, 0xf8, 0x4d, 0x5b, 0xcf, 0x11, 0xa8, 0xfd, 0x00, 0x80, 0x5f, 0x5c, 0x44, 0x2b,
];

/// Grow the first multichannel, more-than-16-bit PCM WAVEFORMATEX in an ASF
/// header into WAVEFORMATEXTENSIBLE (KSDATAFORMAT_SUBTYPE_PCM, 5.1 mask),
/// fixing the header, object, type-data and file sizes it sits inside.
pub(crate) fn asf_wave_extensible(data: &mut Vec<u8>) -> Result<()> {
    const EXTRA: u64 = 22;
    let header_size =
        usize::try_from(le64(data, 16)?).map_err(|_| crate::corpus::error("ASF header size"))?;
    if header_size > data.len() || header_size < 30 {
        return fail("invalid ASF header size");
    }
    let mut position = 30usize;
    let mut file_size_at = None;
    let mut target = None;
    while position < header_size {
        let end = usize::try_from(le64(data, position + 16)?)
            .ok()
            .and_then(|size| position.checked_add(size))
            .filter(|end| *end <= header_size && *end > position)
            .ok_or_else(|| crate::corpus::error(format!("invalid ASF object at {position}")))?;
        let guid = &data[position..position + 16];
        if guid == ASF_FILE_PROPERTIES {
            file_size_at = Some(position + 40);
        } else if guid == ASF_STREAM_PROPERTIES
            && data.get(position + 24..position + 40) == Some(ASF_AUDIO_MEDIA.as_slice())
        {
            let type_size_at = position + 64;
            let type_data_at = position + 78;
            let type_size = le32(data, type_size_at)?;
            let tag = le16(data, type_data_at)?;
            if type_size >= 40 && tag == 0xfffe {
                return Ok(());
            }
            if type_size == 18
                && tag == 1
                && le16(data, type_data_at + 2)? > 2
                && le16(data, type_data_at + 14)? > 16
            {
                target = Some((position + 16, type_size_at, type_data_at));
            }
        }
        position = end;
    }
    let file_size_at =
        file_size_at.ok_or_else(|| crate::corpus::error("no ASF file properties object"))?;
    let (object_size_at, type_size_at, type_data_at) =
        target.ok_or_else(|| crate::corpus::error("no eligible ASF PCM stream"))?;
    let bits = le16(data, type_data_at + 14)?;
    put(data, type_data_at, &0xfffe_u16.to_le_bytes())?;
    put(data, type_data_at + 16, &22_u16.to_le_bytes())?;
    for (at, wide) in [(file_size_at, true), (16, true), (object_size_at, true)] {
        debug_assert!(wide);
        let value = le64(data, at)? + EXTRA;
        put(data, at, &value.to_le_bytes())?;
    }
    let type_size = le32(data, type_size_at)? + EXTRA as u32;
    put(data, type_size_at, &type_size.to_le_bytes())?;
    let mut extension = Vec::with_capacity(EXTRA as usize);
    extension.extend_from_slice(&bits.to_le_bytes());
    extension.extend_from_slice(&0x3f_u32.to_le_bytes());
    extension.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b,
        0x71,
    ]);
    data.splice(type_data_at + 18..type_data_at + 18, extension);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(name);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn dovi_records_pack_like_the_published_fixtures() {
        // The bytes the original profile 7 and profile 8 fixtures carried.
        let p7 = DoviRecord {
            profile: 7,
            level: 6,
            rpu: true,
            el: false,
            bl: true,
            bl_compat_id: 6,
        };
        assert_eq!(&p7.bytes()[..5], &[0x01, 0x00, 0x0e, 0x35, 0x60]);
        let p8 = DoviRecord {
            profile: 8,
            level: 6,
            rpu: true,
            el: false,
            bl: true,
            bl_compat_id: 1,
        };
        assert_eq!(&p8.bytes()[..5], &[0x01, 0x00, 0x10, 0x35, 0x10]);
        assert_eq!(p8.box_type(), b"dvvC");
        assert_eq!(p7.box_type(), b"dvcC");
    }

    #[test]
    fn insertion_grows_every_enclosing_box_and_refuses_faststart() {
        let mut entry_body = vec![0u8; 78];
        entry_body.extend(mp4_box(b"hvcC", &[1, 2, 3]));
        let entry = mp4_box(b"hvc1", &entry_body);
        let mut stsd_body = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd_body.extend(&entry);
        let stsd = mp4_box(b"stsd", &stsd_body);
        let nest = |name: &[u8; 4], inner: Vec<u8>| mp4_box(name, &inner);
        let moov = nest(
            b"moov",
            nest(b"trak", nest(b"mdia", nest(b"minf", nest(b"stbl", stsd)))),
        );
        let mut file = mp4_box(b"ftyp", b"isom");
        file.extend(mp4_box(b"mdat", &[9; 16]));
        file.extend(&moov);
        let child = mp4_box(b"dvcC", &[0; 24]);
        let out = mp4_insert(
            &file,
            &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"],
            SampleEntry::Hevc,
            &child,
        )
        .unwrap();
        assert_eq!(out.len(), file.len() + 32);
        let moov_at = file.len() - moov.len();
        assert_eq!(be32(&out, moov_at).unwrap() as usize, moov.len() + 32);
        assert_eq!(&out[out.len() - 28..out.len() - 24], b"dvcC");

        let mut faststart = mp4_box(b"ftyp", b"isom");
        faststart.extend(&moov);
        faststart.extend(mp4_box(b"mdat", &[9; 16]));
        assert!(mp4_insert(&faststart, &[b"moov"], SampleEntry::None, &child).is_err());
    }

    #[test]
    fn ts_reframing_places_sync_bytes() {
        let mut source = vec![0u8; 188 * 2];
        source[0] = 0x47;
        source[188] = 0x47;
        let out = ts_reframe(&source, 192, 4).unwrap();
        assert_eq!(out.len(), 384);
        assert_eq!(out[4], 0x47);
        assert_eq!(out[196], 0x47);
        assert!(ts_reframe(&source, 188, 4).is_err());
        source[188] = 0;
        assert!(ts_reframe(&source, 204, 0).is_err());
    }
}
