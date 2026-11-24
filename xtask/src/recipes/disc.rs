//! Disc images: DVD-Video on ISO 9660 or UDF 1.02, and Blu-ray / UHD
//! Blu-ray on UDF 2.50 / 2.60 with a metadata partition.
//!
//! FFmpeg writes the payload (MPEG-2 program streams for DVD, 192-byte M2TS
//! for Blu-ray); this module writes everything around it: the navigation
//! files (IFO; index, MovieObject, MPLS and CLPI) and the file system.
//! Navigation values that describe the payload (sector ranges, VOBU starts,
//! presentation times, source packet counts, PIDs) are read back from what
//! FFmpeg wrote, never assumed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::post::Post;
use super::{Expect, Fixture, Generator, Step};
use crate::corpus::{Context as _, Result, error, fail};

const SECTOR: usize = 2048;

// ---------------------------------------------------------------------------
// Description
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Filesystem {
    Iso9660,
    /// UDF with only a physical partition map (DVD-Video uses 1.02).
    UdfPhysical {
        revision: u16,
    },
    /// UDF 2.50 / 2.60 with a metadata partition and its mirror, as BD-ROM.
    UdfMetadata {
        revision: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Disc {
    Dvd(Dvd),
    Bluray(Bluray),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dvd {
    pub(crate) filesystem: Filesystem,
    pub(crate) label: &'static str,
    pub(crate) pal: bool,
    pub(crate) wide: bool,
    pub(crate) audio: Vec<DvdAudio>,
    pub(crate) subpictures: Vec<DvdSubpicture>,
    /// One title set per title; one cell (and chapter) per program stream.
    pub(crate) titles: Vec<Vec<DvdCell>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DvdCell {
    /// Program stream in the work directory.
    pub(crate) vob: &'static str,
    /// Playback time in frames at the nominal rate (25 or 30).
    pub(crate) frames: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DvdAudioFormat {
    Ac3,
    Mpeg1,
    Lpcm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DvdAudio {
    pub(crate) format: DvdAudioFormat,
    pub(crate) channels: u8,
    /// ISO 639-1 code, or empty for "not specified".
    pub(crate) language: &'static str,
    /// Code extension: 1 normal, 2 visually impaired, 3 and 4 commentary.
    pub(crate) extension: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DvdSubpicture {
    pub(crate) language: &'static str,
    /// Code extension: 1 normal, 5-7 captions, 9 forced, 13-15 commentary.
    pub(crate) extension: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bluray {
    pub(crate) label: &'static str,
    pub(crate) uhd: bool,
    /// Scramble every clip's elementary streams after their timing is read,
    /// as AACS leaves them: the clip information is authored from the clear
    /// stream, and only the image carries ciphertext.
    pub(crate) scrambled: bool,
    pub(crate) clips: Vec<Clip>,
    pub(crate) playlists: Vec<Playlist>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Clip {
    /// Five digits.
    pub(crate) id: &'static str,
    /// M2TS in the work directory.
    pub(crate) m2ts: &'static str,
    pub(crate) video: ClipVideo,
    pub(crate) audio: Vec<ClipAudio>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClipVideo {
    /// Stream coding type: 0x02 MPEG-2, 0x1B H.264, 0x24 HEVC.
    pub(crate) coding: u8,
    /// Video format: 4 1080i, 5 720p, 6 1080p, 8 2160p.
    pub(crate) format: u8,
    /// Frame rate: 1 23.976, 2 24, 3 25, 4 29.97.
    pub(crate) rate: u8,
    /// HEVC only: dynamic range (0 SDR, 1 HDR10, 2 Dolby Vision), colour
    /// space (1 BT.709, 2 BT.2020) and the HDR10+ flag.
    pub(crate) dynamic_range: u8,
    pub(crate) colour_space: u8,
    pub(crate) hdr10plus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipAudio {
    /// Stream coding type: 0x80 LPCM, 0x81 AC-3, 0x83 TrueHD, 0x84 E-AC-3.
    pub(crate) coding: u8,
    /// Presentation type: 1 mono, 3 stereo, 6 multichannel.
    pub(crate) presentation: u8,
    pub(crate) language: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Playlist {
    /// Five digits.
    pub(crate) id: &'static str,
    /// Clip ids, each played whole.
    pub(crate) items: Vec<&'static str>,
    /// Entry marks: (play item index, offset from its in time in 1/45000 s).
    pub(crate) marks: Vec<(u16, u32)>,
}

/// Author the image from payload files in `work`.
pub(crate) fn author(disc: &Disc, work: &Path) -> Result<Vec<u8>> {
    match disc {
        Disc::Dvd(dvd) => author_dvd(dvd, work),
        Disc::Bluray(bluray) => author_bluray(bluray, work),
    }
}

fn read(work: &Path, name: &str) -> Result<Vec<u8>> {
    let path = work.join(name);
    std::fs::read(&path).context(|| format!("read {}", path.display()))
}

// ---------------------------------------------------------------------------
// Byte helpers
// ---------------------------------------------------------------------------

fn put(buffer: &mut [u8], at: usize, bytes: &[u8]) {
    buffer[at..at + bytes.len()].copy_from_slice(bytes);
}
fn be16(buffer: &mut [u8], at: usize, value: u16) {
    put(buffer, at, &value.to_be_bytes());
}
fn be32(buffer: &mut [u8], at: usize, value: u32) {
    put(buffer, at, &value.to_be_bytes());
}
fn le16(buffer: &mut [u8], at: usize, value: u16) {
    put(buffer, at, &value.to_le_bytes());
}
fn le32(buffer: &mut [u8], at: usize, value: u32) {
    put(buffer, at, &value.to_le_bytes());
}
fn le64(buffer: &mut [u8], at: usize, value: u64) {
    put(buffer, at, &value.to_le_bytes());
}
fn sectors(length: usize) -> usize {
    length.div_ceil(SECTOR)
}
fn u32_of(value: u64, what: &str) -> Result<u32> {
    u32::try_from(value).or_else(|_| fail(format!("{what} does not fit 32 bits")))
}

// ---------------------------------------------------------------------------
// File systems
// ---------------------------------------------------------------------------

/// Files in placement order (forward-slash paths) and directories that must
/// exist even when empty.
struct Tree {
    files: Vec<(String, Vec<u8>)>,
    empty_dirs: Vec<String>,
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn name_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

impl Tree {
    /// Every directory, the root (`""`) first, then by depth and path.
    fn directories(&self) -> Vec<String> {
        let mut all = BTreeSet::new();
        all.insert(String::new());
        let mut add = |path: &str| {
            let mut at = String::new();
            for part in path.split('/') {
                if !at.is_empty() {
                    at.push('/');
                }
                at.push_str(part);
                all.insert(at.clone());
            }
        };
        for (path, _) in &self.files {
            let parent = parent_of(path);
            if !parent.is_empty() {
                add(parent);
            }
        }
        for dir in &self.empty_dirs {
            add(dir);
        }
        let mut list: Vec<String> = all.into_iter().collect();
        list.sort_by_key(|d| {
            (
                if d.is_empty() {
                    0
                } else {
                    d.split('/').count()
                },
                d.clone(),
            )
        });
        list
    }

    /// Entries of `dir` by name: subdirectories carry `None`, files their index.
    fn children(&self, dir: &str) -> Vec<(String, Option<usize>)> {
        let mut out: Vec<(String, Option<usize>)> = self
            .directories()
            .into_iter()
            .filter(|d| !d.is_empty() && parent_of(d) == dir)
            .map(|d| (name_of(&d).to_owned(), None))
            .collect();
        for (index, (path, _)) in self.files.iter().enumerate() {
            if parent_of(path) == dir {
                out.push((name_of(path).to_owned(), Some(index)));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// Lay out an image; returns it and each file's first sector.
fn image(
    filesystem: &Filesystem,
    label: &str,
    tree: &Tree,
) -> Result<(Vec<u8>, BTreeMap<String, u32>)> {
    match filesystem {
        Filesystem::Iso9660 => iso9660(label, tree),
        Filesystem::UdfPhysical { revision } => udf(label, tree, *revision, false),
        Filesystem::UdfMetadata { revision } => udf(label, tree, *revision, true),
    }
}

// ISO 9660 ------------------------------------------------------------------

/// 2000-01-01 00:00:00 UTC as a directory record date.
const ISO_DATE: [u8; 7] = [100, 1, 1, 0, 0, 0, 0];

fn iso_record(extent: u32, size: u32, directory: bool, name: &[u8]) -> Vec<u8> {
    let length = 33 + name.len() + (name.len() + 1) % 2;
    let mut r = vec![0u8; length];
    r[0] = length as u8;
    le32(&mut r, 2, extent);
    be32(&mut r, 6, extent);
    le32(&mut r, 10, size);
    be32(&mut r, 14, size);
    put(&mut r, 18, &ISO_DATE);
    r[25] = if directory { 2 } else { 0 };
    le16(&mut r, 28, 1);
    be16(&mut r, 30, 1);
    r[32] = name.len() as u8;
    put(&mut r, 33, name);
    r
}

fn iso9660(label: &str, tree: &Tree) -> Result<(Vec<u8>, BTreeMap<String, u32>)> {
    let directories = tree.directories();
    // 16 PVD, 17 terminator, 18 L path table, 19 M path table, then one
    // sector per directory, then file data in placement order.
    let dir_sector: BTreeMap<&str, u32> = directories
        .iter()
        .enumerate()
        .map(|(i, d)| (d.as_str(), 20 + i as u32))
        .collect();
    let mut next = 20 + directories.len();
    let mut file_sector = Vec::new();
    for (_, data) in &tree.files {
        file_sector.push(next as u32);
        next += sectors(data.len());
    }
    let total = next;
    let mut out = vec![0u8; total * SECTOR];

    for dir in &directories {
        let sector = dir_sector[dir.as_str()];
        let mut records = vec![
            iso_record(sector, SECTOR as u32, true, &[0]),
            iso_record(dir_sector[parent_of(dir)], SECTOR as u32, true, &[1]),
        ];
        for (name, file) in tree.children(dir) {
            records.push(match file {
                None => iso_record(
                    dir_sector[join(dir, &name).as_str()],
                    SECTOR as u32,
                    true,
                    name.as_bytes(),
                ),
                Some(index) => iso_record(
                    file_sector[index],
                    u32_of(tree.files[index].1.len() as u64, "ISO file size")?,
                    false,
                    format!("{name};1").as_bytes(),
                ),
            });
        }
        let bytes = records.concat();
        if bytes.len() > SECTOR {
            return fail(format!("ISO directory {dir:?} outgrows one sector"));
        }
        put(&mut out, sector as usize * SECTOR, &bytes);
    }

    let number: BTreeMap<&str, u16> = directories
        .iter()
        .enumerate()
        .map(|(i, d)| (d.as_str(), i as u16 + 1))
        .collect();
    let mut l_table = Vec::new();
    let mut m_table = Vec::new();
    for dir in &directories {
        let name = if dir.is_empty() {
            vec![0u8]
        } else {
            name_of(dir).as_bytes().to_vec()
        };
        let sector = dir_sector[dir.as_str()];
        let parent = number[parent_of(dir)];
        for (table, big) in [(&mut l_table, false), (&mut m_table, true)] {
            table.push(name.len() as u8);
            table.push(0);
            table.extend(if big {
                sector.to_be_bytes()
            } else {
                sector.to_le_bytes()
            });
            table.extend(if big {
                parent.to_be_bytes()
            } else {
                parent.to_le_bytes()
            });
            table.extend(&name);
            if name.len() % 2 == 1 {
                table.push(0);
            }
        }
    }
    if l_table.len() > SECTOR {
        return fail("ISO path table outgrows one sector");
    }
    put(&mut out, 18 * SECTOR, &l_table);
    put(&mut out, 19 * SECTOR, &m_table);

    let pvd = &mut out[16 * SECTOR..17 * SECTOR];
    pvd[0] = 1;
    put(pvd, 1, b"CD001");
    pvd[6] = 1;
    pvd[8..72].fill(b' ');
    put(pvd, 40, label.to_ascii_uppercase().as_bytes());
    le32(pvd, 80, total as u32);
    be32(pvd, 84, total as u32);
    for at in [120, 124] {
        le16(pvd, at, 1);
        be16(pvd, at + 2, 1);
    }
    le16(pvd, 128, SECTOR as u16);
    be16(pvd, 130, SECTOR as u16);
    le32(pvd, 132, l_table.len() as u32);
    be32(pvd, 136, l_table.len() as u32);
    le32(pvd, 140, 18);
    be32(pvd, 148, 19);
    put(
        pvd,
        156,
        &iso_record(dir_sector[""], SECTOR as u32, true, &[0]),
    );
    pvd[190..813].fill(b' ');
    for at in [813, 830] {
        put(pvd, at, b"2000010100000000");
    }
    for at in [847, 864] {
        put(pvd, at, b"0000000000000000");
    }
    pvd[881] = 1;
    let terminator = &mut out[17 * SECTOR..18 * SECTOR];
    terminator[0] = 255;
    put(terminator, 1, b"CD001");
    terminator[6] = 1;

    let mut placed = BTreeMap::new();
    for ((path, data), sector) in tree.files.iter().zip(&file_sector) {
        put(&mut out, *sector as usize * SECTOR, data);
        placed.insert(path.clone(), *sector);
    }
    Ok((out, placed))
}

// UDF -----------------------------------------------------------------------

/// CRC-CCITT (polynomial 0x1021, initial 0), as ECMA-167 7.2.6 specifies.
fn udf_crc(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Fill the descriptor tag of `d`, with the CRC over `d[16..16 + crc_length]`.
fn tag(d: &mut [u8], id: u16, version: u16, location: u32, crc_length: usize) {
    le16(d, 0, id);
    le16(d, 2, version);
    d[4] = 0;
    d[5] = 0;
    le16(d, 6, 1);
    let crc = udf_crc(&d[16..16 + crc_length]);
    le16(d, 8, crc);
    le16(d, 10, crc_length as u16);
    le32(d, 12, location);
    d[4] = d[..16]
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 4)
        .fold(0u8, |s, (_, b)| s.wrapping_add(*b));
}

fn regid(d: &mut [u8], at: usize, identifier: &[u8], suffix: &[u8]) {
    d[at] = 0;
    put(d, at + 1, identifier);
    put(d, at + 24, suffix);
}

fn domain(d: &mut [u8], at: usize, revision: u16) {
    regid(d, at, b"*OSTA UDF Compliant", &revision.to_le_bytes());
}

fn implementation(d: &mut [u8], at: usize) {
    regid(d, at, b"*mediaprobe corpus", &[]);
}

fn charspec(d: &mut [u8], at: usize) {
    d[at] = 0;
    put(d, at + 1, b"OSTA Compressed Unicode");
}

/// A `width`-byte d-string: compression id 8, the characters, length last.
fn dstring(d: &mut [u8], at: usize, width: usize, text: &str) {
    d[at] = 8;
    put(d, at + 1, text.as_bytes());
    d[at + width - 1] = (text.len() + 1) as u8;
}

/// 2000-01-01 00:00 UTC.
fn timestamp(d: &mut [u8], at: usize) {
    le16(d, at, 0x1000);
    le16(d, at + 2, 2000);
    d[at + 4] = 1;
    d[at + 5] = 1;
}

fn long_ad(d: &mut [u8], at: usize, length: u32, block: u32, partition: u16) {
    le32(d, at, length);
    le32(d, at + 4, block);
    le16(d, at + 8, partition);
}

const VRS: usize = 16;
const MAIN_VDS: usize = 32;
const RESERVE_VDS: usize = 48;
const LVID: usize = 64;
const ANCHOR: usize = 256;
const PARTITION_START: usize = 257;

struct FileEntry {
    file_type: u8,
    /// Allocation descriptor type in the ICB flags: 0 short, 1 long, 3 inline.
    ad_type: u16,
    information_length: u64,
    blocks_recorded: u64,
    links: u16,
    unique_id: u64,
    /// Allocation descriptors, or the data itself when inline.
    tail: Vec<u8>,
}

fn file_entry(entry: &FileEntry, extended: bool, version: u16, location: u32) -> Result<Vec<u8>> {
    let header = if extended { 216 } else { 176 };
    if header + entry.tail.len() > SECTOR {
        return fail("UDF file entry outgrows its block");
    }
    let mut d = vec![0u8; SECTOR];
    le16(&mut d, 20, 4);
    le16(&mut d, 24, 1);
    d[27] = entry.file_type;
    le16(&mut d, 34, entry.ad_type);
    le32(&mut d, 36, u32::MAX);
    le32(&mut d, 40, u32::MAX);
    le32(&mut d, 44, 0x14A5);
    le16(&mut d, 48, entry.links);
    le64(&mut d, 56, entry.information_length);
    if extended {
        le64(&mut d, 64, entry.information_length);
        le64(&mut d, 72, entry.blocks_recorded);
        for at in [80, 92, 104, 116] {
            timestamp(&mut d, at);
        }
        le32(&mut d, 128, 1);
        implementation(&mut d, 168);
        le64(&mut d, 200, entry.unique_id);
        le32(&mut d, 212, entry.tail.len() as u32);
    } else {
        le64(&mut d, 64, entry.blocks_recorded);
        for at in [72, 84, 96] {
            timestamp(&mut d, at);
        }
        le32(&mut d, 108, 1);
        implementation(&mut d, 128);
        le64(&mut d, 160, entry.unique_id);
        le32(&mut d, 172, entry.tail.len() as u32);
    }
    put(&mut d, header, &entry.tail);
    tag(
        &mut d,
        if extended { 266 } else { 261 },
        version,
        location,
        header - 16 + entry.tail.len(),
    );
    Ok(d)
}

fn fid_length(name: &str) -> usize {
    (38 + if name.is_empty() { 0 } else { name.len() + 1 } + 3) & !3
}

fn fid(
    characteristics: u8,
    name: &str,
    icb_block: u32,
    icb_partition: u16,
    version: u16,
    location: u32,
) -> Vec<u8> {
    let encoded: Vec<u8> = if name.is_empty() {
        Vec::new()
    } else {
        [&[8u8][..], name.as_bytes()].concat()
    };
    let length = fid_length(name);
    let mut d = vec![0u8; length];
    le16(&mut d, 16, 1);
    d[18] = characteristics;
    d[19] = encoded.len() as u8;
    long_ad(&mut d, 20, SECTOR as u32, icb_block, icb_partition);
    put(&mut d, 38, &encoded);
    tag(&mut d, 257, version, location, length - 16);
    d
}

/// Allocation descriptors covering `length` bytes from `block`.
fn allocation(length: u64, block: u32, long: bool, partition: u16) -> Vec<u8> {
    const MAX: u64 = 0x3FFF_F800;
    let mut out = Vec::new();
    let mut remaining = length;
    let mut at = block;
    while remaining > 0 {
        let take = remaining.min(MAX);
        out.extend((take as u32).to_le_bytes());
        out.extend(at.to_le_bytes());
        if long {
            out.extend(partition.to_le_bytes());
            out.extend([0u8; 6]);
        }
        remaining -= take;
        at += (take / SECTOR as u64) as u32;
    }
    out
}

fn udf(
    label: &str,
    tree: &Tree,
    revision: u16,
    metadata: bool,
) -> Result<(Vec<u8>, BTreeMap<String, u32>)> {
    let version: u16 = if revision >= 0x0200 { 3 } else { 2 };
    let extended = revision >= 0x0200;
    let header = if extended { 216 } else { 176 };
    // The partition reference that holds directories and file entries.
    let meta_partition: u16 = u16::from(metadata);
    let directories = tree.directories();

    // Blocks of the metadata space (or the start of the physical partition):
    // 0 FSD, 1 terminator, one entry per directory and file, then directory
    // data too large to inline.
    let mut next = 2u32;
    let mut dir_block = BTreeMap::new();
    for dir in &directories {
        dir_block.insert(dir.clone(), next);
        next += 1;
    }
    let mut file_block = Vec::new();
    for _ in &tree.files {
        file_block.push(next);
        next += 1;
    }
    let mut dir_data_block = BTreeMap::new();
    for dir in &directories {
        let size = fid_length("")
            + tree
                .children(dir)
                .iter()
                .map(|(n, _)| fid_length(n))
                .sum::<usize>();
        if header + size > SECTOR {
            dir_data_block.insert(dir.clone(), next);
            next += sectors(size) as u32;
        }
    }
    let meta_blocks = next as usize;

    // Physical partition: [metadata FE, mirror FE, metadata, mirror] when a
    // metadata partition is used, then file data.
    let (meta_at, mirror_at, mut data_next) = if metadata {
        (2usize, 2 + meta_blocks, 2 + 2 * meta_blocks)
    } else {
        (0, 0, meta_blocks)
    };
    let mut data_block = Vec::new();
    for (_, data) in &tree.files {
        data_block.push(data_next as u32);
        data_next += sectors(data.len());
    }
    let partition_length = data_next;
    let total = PARTITION_START + partition_length + 1;
    let mut out = vec![0u8; total * SECTOR];

    let mut meta = vec![0u8; meta_blocks * SECTOR];
    let unique = |index: usize| 16 + index as u64;
    {
        let fsd = &mut meta[..SECTOR];
        timestamp(fsd, 16);
        le16(fsd, 28, 3);
        le16(fsd, 30, 3);
        le32(fsd, 32, 1);
        le32(fsd, 36, 1);
        charspec(fsd, 48);
        dstring(fsd, 112, 128, label);
        charspec(fsd, 240);
        dstring(fsd, 304, 32, label);
        long_ad(fsd, 400, SECTOR as u32, dir_block[""], meta_partition);
        domain(fsd, 416, revision);
        tag(fsd, 256, version, 0, 496);
        tag(&mut meta[SECTOR..2 * SECTOR], 8, version, 1, 496);
    }
    for (index, dir) in directories.iter().enumerate() {
        let block = dir_block[dir];
        let children = tree.children(dir);
        let data_location = dir_data_block.get(dir).copied();
        let mut data = Vec::new();
        // Each identifier is tagged with the block it starts in.
        let push = |data: &mut Vec<u8>, characteristics: u8, name: &str, child: u32| {
            let location =
                data_location.map_or(block, |first| first + (data.len() / SECTOR) as u32);
            data.extend(fid(
                characteristics,
                name,
                child,
                meta_partition,
                version,
                location,
            ));
        };
        push(&mut data, 0x0A, "", dir_block[parent_of(dir)]);
        for (name, file) in &children {
            match file {
                None => push(&mut data, 0x02, name, dir_block[&join(dir, name)]),
                Some(i) => push(&mut data, 0x00, name, file_block[*i]),
            }
        }
        let subdirectories = children.iter().filter(|(_, f)| f.is_none()).count() as u16;
        let (ad_type, tail, recorded) = match data_location {
            None => (3, data.clone(), 0),
            Some(first) => {
                let at = first as usize * SECTOR;
                meta[at..at + data.len()].copy_from_slice(&data);
                (
                    0,
                    allocation(data.len() as u64, first, false, 0),
                    sectors(data.len()) as u64,
                )
            }
        };
        let entry = FileEntry {
            file_type: 4,
            ad_type,
            information_length: data.len() as u64,
            blocks_recorded: recorded,
            links: 1 + subdirectories,
            unique_id: if dir.is_empty() {
                0
            } else {
                unique(tree.files.len() + index)
            },
            tail,
        };
        let bytes = file_entry(&entry, extended, version, block)?;
        put(&mut meta, block as usize * SECTOR, &bytes);
    }
    for (index, (_, data)) in tree.files.iter().enumerate() {
        let block = file_block[index];
        let entry = FileEntry {
            file_type: 5,
            ad_type: u16::from(metadata),
            information_length: data.len() as u64,
            blocks_recorded: sectors(data.len()) as u64,
            links: 1,
            unique_id: unique(index),
            tail: allocation(data.len() as u64, data_block[index], metadata, 0),
        };
        let bytes = file_entry(&entry, extended, version, block)?;
        put(&mut meta, block as usize * SECTOR, &bytes);
    }

    let partition = |block: usize| (PARTITION_START + block) * SECTOR;
    if metadata {
        for (fe_block, file_type, extent) in [(0usize, 250u8, meta_at), (1, 251, mirror_at)] {
            let entry = FileEntry {
                file_type,
                ad_type: 0,
                information_length: (meta_blocks * SECTOR) as u64,
                blocks_recorded: meta_blocks as u64,
                links: 1,
                unique_id: 0,
                tail: allocation((meta_blocks * SECTOR) as u64, extent as u32, false, 0),
            };
            let bytes = file_entry(&entry, extended, version, fe_block as u32)?;
            put(&mut out, partition(fe_block), &bytes);
            put(&mut out, partition(extent), &meta);
        }
    } else {
        put(&mut out, partition(0), &meta);
    }
    let mut placed = BTreeMap::new();
    for (index, (path, data)) in tree.files.iter().enumerate() {
        let at = data_block[index] as usize;
        put(&mut out, partition(at), data);
        placed.insert(path.clone(), (PARTITION_START + at) as u32);
    }

    // Volume recognition sequence.
    let nsr: &[u8] = if version == 3 { b"NSR03" } else { b"NSR02" };
    for (i, id) in [&b"BEA01"[..], nsr, b"TEA01"].into_iter().enumerate() {
        let d = &mut out[(VRS + i) * SECTOR..];
        put(d, 1, id);
        d[6] = 1;
    }

    // Partition maps: the physical partition, then the metadata partition.
    let mut maps = vec![1u8, 6, 0, 0, 0, 0];
    le16(&mut maps, 2, 1);
    if metadata {
        let mut map = vec![0u8; 64];
        map[0] = 2;
        map[1] = 64;
        regid(
            &mut map,
            4,
            b"*UDF Metadata Partition",
            &revision.to_le_bytes(),
        );
        le16(&mut map, 36, 1);
        le16(&mut map, 38, 0);
        le32(&mut map, 40, 0);
        le32(&mut map, 44, 1);
        le32(&mut map, 48, u32::MAX);
        le32(&mut map, 52, 32);
        le16(&mut map, 56, 1);
        map[58] = 1;
        maps.extend(map);
    }
    // Main and reserve volume descriptor sequences: PVD, PD, LVD, USD, TD.
    for start in [MAIN_VDS, RESERVE_VDS] {
        let at = |i: usize| (start + i) * SECTOR;
        let location = |i: usize| (start + i) as u32;
        {
            let d = &mut out[at(0)..at(1)];
            le32(d, 16, 1);
            dstring(d, 24, 32, label);
            le16(d, 56, 1);
            le16(d, 58, 1);
            le16(d, 60, 2);
            le16(d, 62, 3);
            le32(d, 64, 1);
            le32(d, 68, 1);
            dstring(d, 72, 128, label);
            charspec(d, 200);
            charspec(d, 264);
            timestamp(d, 376);
            implementation(d, 388);
            tag(d, 1, version, location(0), 496);
        }
        {
            let d = &mut out[at(1)..at(2)];
            le32(d, 16, 2);
            le16(d, 20, 1);
            regid(d, 24, if version == 3 { b"+NSR03" } else { b"+NSR02" }, &[]);
            le32(d, 184, 1);
            le32(d, 188, PARTITION_START as u32);
            le32(d, 192, partition_length as u32);
            implementation(d, 196);
            tag(d, 5, version, location(1), 496);
        }
        {
            let d = &mut out[at(2)..at(3)];
            le32(d, 16, 3);
            charspec(d, 20);
            dstring(d, 84, 128, label);
            le32(d, 212, SECTOR as u32);
            domain(d, 216, revision);
            long_ad(d, 248, SECTOR as u32, 0, meta_partition);
            le32(d, 264, maps.len() as u32);
            le32(d, 268, if metadata { 2 } else { 1 });
            implementation(d, 272);
            le32(d, 432, SECTOR as u32);
            le32(d, 436, LVID as u32);
            put(d, 440, &maps);
            tag(d, 6, version, location(2), 424 + maps.len());
        }
        {
            let d = &mut out[at(3)..at(4)];
            le32(d, 16, 4);
            tag(d, 7, version, location(3), 8);
        }
        tag(&mut out[at(4)..at(5)], 8, version, location(4), 496);
    }

    // Logical volume integrity: closed, no free space.
    {
        let d = &mut out[LVID * SECTOR..(LVID + 1) * SECTOR];
        timestamp(d, 16);
        le32(d, 28, 1);
        le64(d, 40, 16 + (tree.files.len() + directories.len()) as u64);
        let partitions = if metadata { 2 } else { 1 };
        le32(d, 72, partitions as u32);
        le32(d, 76, 46);
        let sizes = 80 + 4 * partitions;
        le32(d, sizes, partition_length as u32);
        if metadata {
            le32(d, sizes + 4, meta_blocks as u32);
        }
        let iu = 80 + 8 * partitions;
        implementation(d, iu);
        le32(d, iu + 32, tree.files.len() as u32);
        le32(d, iu + 36, directories.len() as u32);
        for at in [40, 42, 44] {
            le16(d, iu + at, revision);
        }
        tag(d, 9, version, LVID as u32, iu + 46 - 16);
    }

    // Anchors at 256 and at the last sector.
    for sector in [ANCHOR, total - 1] {
        let d = &mut out[sector * SECTOR..(sector + 1) * SECTOR];
        le32(d, 16, (16 * SECTOR) as u32);
        le32(d, 20, MAIN_VDS as u32);
        le32(d, 24, (16 * SECTOR) as u32);
        le32(d, 28, RESERVE_VDS as u32);
        tag(d, 2, version, sector as u32, 496);
    }
    Ok((out, placed))
}

// ---------------------------------------------------------------------------
// DVD-Video
// ---------------------------------------------------------------------------

fn bcd(value: u32) -> u8 {
    (((value / 10) << 4) | (value % 10)) as u8
}

/// DVD playback time: BCD hours, minutes, seconds and frames, with the frame
/// rate in the top two bits (01 25 fps, 11 29.97 fps).
fn dvd_time(frames: u32, pal: bool) -> [u8; 4] {
    let rate = if pal { 25 } else { 30 };
    let seconds = frames / rate;
    [
        bcd(seconds / 3600),
        bcd(seconds / 60 % 60),
        bcd(seconds % 60),
        bcd(frames % rate) | if pal { 0x40 } else { 0xC0 },
    ]
}

/// Sectors (relative to the start of `vob`) whose pack carries a system
/// header: FFmpeg's DVD muxer starts each VOBU that way, ahead of its NAV pack.
fn vobu_starts(vob: &[u8]) -> Vec<u32> {
    vob.chunks_exact(SECTOR)
        .enumerate()
        .filter(|(_, pack)| pack[..4] == [0, 0, 1, 0xBA] && pack[14..18] == [0, 0, 1, 0xBB])
        .map(|(i, _)| i as u32)
        .collect()
}

/// A cell's sectors within its title set's VOB: first, last VOBU start, last.
#[derive(Clone, Copy)]
struct CellRange {
    first: u32,
    last_vobu: u32,
    last: u32,
}

fn dvd_vts_ifo(dvd: &Dvd, cells: &[CellRange], frames: &[u32], vob_sectors: usize) -> Vec<u8> {
    let mut ifo = vec![0u8; 3 * SECTOR];
    put(&mut ifo, 0, b"DVDVIDEO-VTS");
    be32(&mut ifo, 0x0C, (3 + vob_sectors + 3 - 1) as u32);
    be32(&mut ifo, 0x1C, 2);
    be16(&mut ifo, 0x20, 0x0011);
    be32(&mut ifo, 0x80, 0x3FF);
    be32(&mut ifo, 0xC4, 3);
    be32(&mut ifo, 0xC8, 1);
    be32(&mut ifo, 0xCC, 2);
    // Title set attributes: MPEG-2, NTSC/PAL, 4:3 or 16:9, 720 wide.
    ifo[0x200] = 0x40 | if dvd.pal { 0x10 } else { 0 } | if dvd.wide { 0x0C } else { 0 };
    be16(&mut ifo, 0x202, dvd.audio.len() as u16);
    for (i, a) in dvd.audio.iter().enumerate() {
        let at = 0x204 + i * 8;
        let format = match a.format {
            DvdAudioFormat::Ac3 => 0,
            DvdAudioFormat::Mpeg1 => 2,
            DvdAudioFormat::Lpcm => 4,
        };
        ifo[at] = (format << 5) | if a.language.is_empty() { 0 } else { 0x04 };
        ifo[at + 1] = a.channels - 1;
        if !a.language.is_empty() {
            put(&mut ifo, at + 2, a.language.as_bytes());
        }
        ifo[at + 5] = a.extension;
    }
    be16(&mut ifo, 0x254, dvd.subpictures.len() as u16);
    for (i, s) in dvd.subpictures.iter().enumerate() {
        let at = 0x256 + i * 6;
        ifo[at] = u8::from(!s.language.is_empty());
        if !s.language.is_empty() {
            put(&mut ifo, at + 2, s.language.as_bytes());
        }
        ifo[at + 5] = s.extension;
    }

    // PTT_SRPT: one title, one part (program) per cell.
    let n = cells.len();
    {
        let t = &mut ifo[SECTOR..2 * SECTOR];
        be16(t, 0, 1);
        be32(t, 4, (12 + 4 * n - 1) as u32);
        be32(t, 8, 12);
        for i in 0..n {
            be16(t, 12 + i * 4, 1);
            be16(t, 14 + i * 4, (i + 1) as u16);
        }
    }

    // PGCIT: one entry PGC with one program per cell.
    let mut pgc = vec![0u8; 236];
    pgc[2] = n as u8;
    pgc[3] = n as u8;
    put(&mut pgc, 4, &dvd_time(frames.iter().sum(), dvd.pal));
    let mut counters = BTreeMap::new();
    for (i, a) in dvd.audio.iter().enumerate() {
        let number = counters.entry(a.format).or_insert(0u16);
        be16(&mut pgc, 12 + i * 2, 0x8000 | (*number << 8));
        *number += 1;
    }
    for i in 0..dvd.subpictures.len() {
        let s = i as u32;
        be32(
            &mut pgc,
            28 + i * 4,
            0x8000_0000 | (s << 24) | (s << 16) | (s << 8) | s,
        );
    }
    let program_at = 236;
    let cell_at = program_at + n + n % 2;
    let position_at = cell_at + 24 * n;
    be16(&mut pgc, 230, program_at as u16);
    be16(&mut pgc, 232, cell_at as u16);
    be16(&mut pgc, 234, position_at as u16);
    pgc.resize(position_at + 4 * n, 0);
    for (i, cell) in cells.iter().enumerate() {
        pgc[program_at + i] = (i + 1) as u8;
        let c = cell_at + 24 * i;
        put(&mut pgc, c + 4, &dvd_time(frames[i], dvd.pal));
        be32(&mut pgc, c + 8, cell.first);
        be32(&mut pgc, c + 16, cell.last_vobu);
        be32(&mut pgc, c + 20, cell.last);
        be16(&mut pgc, position_at + 4 * i, 1);
        pgc[position_at + 4 * i + 3] = (i + 1) as u8;
    }
    let t = &mut ifo[2 * SECTOR..3 * SECTOR];
    be16(t, 0, 1);
    be32(t, 4, (16 + pgc.len() - 1) as u32);
    be32(t, 8, 0x8100_0000);
    be32(t, 12, 16);
    put(t, 16, &pgc);
    ifo
}

fn dvd_vmg_ifo(dvd: &Dvd, vts_start: &[u32]) -> Vec<u8> {
    let mut ifo = vec![0u8; 2 * SECTOR];
    put(&mut ifo, 0, b"DVDVIDEO-VMG");
    be32(&mut ifo, 0x0C, 3);
    be32(&mut ifo, 0x1C, 1);
    be16(&mut ifo, 0x20, 0x0011);
    be16(&mut ifo, 0x26, 1);
    be16(&mut ifo, 0x28, 1);
    ifo[0x2A] = 1;
    be16(&mut ifo, 0x3E, dvd.titles.len() as u16);
    put(&mut ifo, 0x40, b"MEDIAPROBE CORPUS");
    be32(&mut ifo, 0x80, 0x3FF);
    be32(&mut ifo, 0xC4, 1);
    let t = &mut ifo[SECTOR..];
    be16(t, 0, dvd.titles.len() as u16);
    be32(t, 4, (8 + 12 * dvd.titles.len() - 1) as u32);
    for (i, cells) in dvd.titles.iter().enumerate() {
        let e = 8 + 12 * i;
        t[e] = 0x3C;
        t[e + 1] = 1;
        be16(t, e + 2, cells.len() as u16);
        t[e + 6] = (i + 1) as u8;
        t[e + 7] = 1;
        be32(t, e + 8, vts_start.get(i).copied().unwrap_or(0));
    }
    ifo
}

fn author_dvd(dvd: &Dvd, work: &Path) -> Result<Vec<u8>> {
    if dvd.titles.is_empty()
        || dvd.titles.len() > 99
        || dvd.audio.len() > 8
        || dvd.subpictures.len() > 32
    {
        return fail("DVD title, audio or subpicture count out of range");
    }
    let mut files = Vec::new();
    let mut set_ifos = Vec::new();
    for (index, cells) in dvd.titles.iter().enumerate() {
        let set = index + 1;
        let mut vob = Vec::new();
        let mut ranges = Vec::new();
        for cell in cells {
            let data = read(work, cell.vob)?;
            if data.is_empty() || data.len() % SECTOR != 0 {
                return fail(format!("{} is not a whole number of DVD sectors", cell.vob));
            }
            let first = (vob.len() / SECTOR) as u32;
            let Some(&last_vobu) = vobu_starts(&data).last() else {
                return fail(format!("{} has no VOBU", cell.vob));
            };
            ranges.push(CellRange {
                first,
                last_vobu: first + last_vobu,
                last: first + (data.len() / SECTOR) as u32 - 1,
            });
            vob.extend(data);
        }
        let frames: Vec<u32> = cells.iter().map(|c| c.frames).collect();
        let ifo = dvd_vts_ifo(dvd, &ranges, &frames, vob.len() / SECTOR);
        let ifo_path = format!("VIDEO_TS/VTS_{set:02}_0.IFO");
        set_ifos.push(ifo_path.clone());
        files.push((ifo_path, ifo.clone()));
        files.push((format!("VIDEO_TS/VTS_{set:02}_1.VOB"), vob));
        files.push((format!("VIDEO_TS/VTS_{set:02}_0.BUP"), ifo));
    }
    let vmg = dvd_vmg_ifo(dvd, &[]);
    let mut all = vec![
        ("VIDEO_TS/VIDEO_TS.IFO".to_owned(), vmg.clone()),
        ("VIDEO_TS/VIDEO_TS.BUP".to_owned(), vmg),
    ];
    all.extend(files);
    let mut tree = Tree {
        files: all,
        empty_dirs: vec!["AUDIO_TS".into()],
    };
    // Lay out once to learn where each title set starts, then again with
    // those sectors in the title search pointers (no size changes).
    let (_, placed) = image(&dvd.filesystem, dvd.label, &tree)?;
    let starts: Vec<u32> = set_ifos.iter().map(|path| placed[path]).collect();
    let vmg = dvd_vmg_ifo(dvd, &starts);
    tree.files[0].1 = vmg.clone();
    tree.files[1].1 = vmg;
    let (bytes, again) = image(&dvd.filesystem, dvd.label, &tree)?;
    if again != placed {
        return fail("DVD layout moved between passes");
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Blu-ray
// ---------------------------------------------------------------------------

const VIDEO_PID: u16 = 0x1011;
const AUDIO_PID: u16 = 0x1100;
const PMT_PID: u16 = 0x0100;

struct ClipTiming {
    source_packets: u32,
    /// Presentation start and end, 45 kHz.
    input: u32,
    output: u32,
    pids: Vec<u16>,
}

fn pes_pts(payload: &[u8]) -> Option<u64> {
    if payload.len() < 14 || payload[..3] != [0, 0, 1] || payload[7] & 0x80 == 0 {
        return None;
    }
    let p = &payload[9..14];
    Some(
        (u64::from(p[0] >> 1 & 7) << 30)
            | (u64::from(p[1]) << 22)
            | (u64::from(p[2] >> 1) << 15)
            | (u64::from(p[3]) << 7)
            | u64::from(p[4] >> 1),
    )
}

/// One frame in 90 kHz ticks, rounded up.
fn frame_ticks_90k(rate: u8) -> Result<u64> {
    Ok(match rate {
        1 => 3754,
        2 => 3750,
        3 => 3600,
        4 => 3003,
        6 => 1800,
        7 => 1502,
        _ => return fail("unknown Blu-ray frame rate code"),
    })
}

fn clip_timing(m2ts: &[u8], rate: u8) -> Result<ClipTiming> {
    if m2ts.is_empty() || !m2ts.len().is_multiple_of(192) {
        return fail("M2TS is not a whole number of 192-byte packets");
    }
    let mut first = None::<u64>;
    let mut last = None::<u64>;
    let mut pids = Vec::new();
    for packet in m2ts.chunks_exact(192) {
        let p = &packet[4..];
        if p[0] != 0x47 {
            return fail("lost M2TS sync");
        }
        let pid = (u16::from(p[1] & 0x1f) << 8) | u16::from(p[2]);
        if !pids.contains(&pid) {
            pids.push(pid);
        }
        if pid != VIDEO_PID || p[1] & 0x40 == 0 || p[3] & 0x10 == 0 {
            continue;
        }
        let at = if p[3] & 0x20 != 0 {
            5 + usize::from(p[4])
        } else {
            4
        };
        if let Some(pts) = p.get(at..).and_then(pes_pts) {
            first = Some(first.map_or(pts, |f| f.min(pts)));
            last = Some(last.map_or(pts, |l| l.max(pts)));
        }
    }
    let (Some(first), Some(last)) = (first, last) else {
        return fail(format!("no timed video PES on PID {VIDEO_PID:#x}"));
    };
    let end = last + frame_ticks_90k(rate)?;
    Ok(ClipTiming {
        source_packets: u32_of((m2ts.len() / 192) as u64, "source packet count")?,
        input: u32_of(first / 2, "presentation start")?,
        output: u32_of(end.div_ceil(2), "presentation end")?,
        pids,
    })
}

/// Video stream attributes. Clip information pads to 21 bytes and flags
/// HDR10+ in bit 7 of the last byte; the STN table packs them and flags it
/// in bit 6.
fn video_attributes(v: &ClipVideo, clip_info: bool) -> Vec<u8> {
    let mut a = vec![v.coding, (v.format << 4) | v.rate];
    if clip_info {
        a.push(0x30);
    }
    if v.coding == 0x24 {
        a.push((v.dynamic_range << 4) | v.colour_space);
        a.push(match (v.hdr10plus, clip_info) {
            (false, _) => 0,
            (true, true) => 0x80,
            (true, false) => 0x40,
        });
    }
    if clip_info {
        a.resize(0x15, 0);
    }
    a
}

fn audio_attributes(a: &ClipAudio, clip_info: bool) -> Vec<u8> {
    let mut out = vec![a.coding, (a.presentation << 4) | 1];
    out.extend(a.language.as_bytes());
    if clip_info {
        out.resize(0x15, 0);
    }
    out
}

fn clpi(clip: &Clip, timing: &ClipTiming, version: &[u8; 4]) -> Vec<u8> {
    let mut out = vec![0u8; 40];
    put(&mut out, 0, b"HDMV");
    put(&mut out, 4, version);
    let mut info = vec![0u8; 176];
    info[2] = 1; // clip stream type: AV stream
    info[3] = 1; // application type: main TS of a movie
    be32(&mut info, 8, 48_000_000 / 8);
    be32(&mut info, 12, timing.source_packets);
    be16(&mut info, 144, 30);
    info[146] = 0x80;
    put(&mut info, 147, b"HDMV");
    out.extend((info.len() as u32).to_be_bytes());
    out.extend(info);

    let sequence_at = out.len();
    let mut seq = vec![0u8, 1];
    seq.extend(0u32.to_be_bytes());
    seq.extend([1u8, 0]);
    seq.extend(VIDEO_PID.to_be_bytes());
    seq.extend(0u32.to_be_bytes());
    seq.extend(timing.input.to_be_bytes());
    seq.extend(timing.output.to_be_bytes());
    out.extend((seq.len() as u32).to_be_bytes());
    out.extend(seq);

    let program_at = out.len();
    let mut program = vec![0u8, 1];
    program.extend(0u32.to_be_bytes());
    program.extend(PMT_PID.to_be_bytes());
    program.push(1 + clip.audio.len() as u8);
    program.push(0);
    let mut stream = |pid: u16, attributes: Vec<u8>| {
        program.extend(pid.to_be_bytes());
        program.push(attributes.len() as u8);
        program.extend(attributes);
    };
    stream(VIDEO_PID, video_attributes(&clip.video, true));
    for (i, a) in clip.audio.iter().enumerate() {
        stream(AUDIO_PID + i as u16, audio_attributes(a, true));
    }
    out.extend((program.len() as u32).to_be_bytes());
    out.extend(program);
    be32(&mut out, 8, sequence_at as u32);
    be32(&mut out, 12, program_at as u32);
    out
}

fn mpls(
    playlist: &Playlist,
    timings: &BTreeMap<&str, (ClipTiming, &Clip)>,
    version: &[u8; 4],
) -> Result<Vec<u8>> {
    let mut out = vec![0u8; 40];
    put(&mut out, 0, b"MPLS");
    put(&mut out, 4, version);
    // AppInfoPlayList: sequential playback.
    out.extend(14u32.to_be_bytes());
    out.extend([0, 1, 0, 0]);
    out.extend([0u8; 10]);
    let list_at = out.len();
    let mut list = vec![0u8, 0];
    list.extend((playlist.items.len() as u16).to_be_bytes());
    list.extend(0u16.to_be_bytes());
    for id in &playlist.items {
        let Some((timing, clip)) = timings.get(id) else {
            return fail(format!("playlist {} names unknown clip {id}", playlist.id));
        };
        let mut item = Vec::new();
        item.extend(id.as_bytes());
        item.extend(b"M2TS");
        item.extend([0x00, 0x01, 0x00]);
        item.extend(timing.input.to_be_bytes());
        item.extend(timing.output.to_be_bytes());
        item.extend([0u8; 12]);
        // STN table: one video stream and the clip's audio streams.
        let mut stn = vec![
            0u8,
            0,
            1,
            clip.audio.len() as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        let mut entry = |pid: u16, attributes: Vec<u8>| {
            stn.extend([9u8, 1]);
            stn.extend(pid.to_be_bytes());
            stn.extend([0u8; 6]);
            stn.push(attributes.len() as u8);
            stn.extend(attributes);
        };
        entry(VIDEO_PID, video_attributes(&clip.video, false));
        for (i, a) in clip.audio.iter().enumerate() {
            entry(AUDIO_PID + i as u16, audio_attributes(a, false));
        }
        item.extend((stn.len() as u16).to_be_bytes());
        item.extend(stn);
        list.extend((item.len() as u16).to_be_bytes());
        list.extend(item);
    }
    out.extend((list.len() as u32).to_be_bytes());
    out.extend(list);
    let mark_at = out.len();
    let mut marks = (playlist.marks.len() as u16).to_be_bytes().to_vec();
    for (item, offset) in &playlist.marks {
        let id = playlist
            .items
            .get(usize::from(*item))
            .ok_or_else(|| error("mark names a missing play item"))?;
        let (timing, _) = &timings[id];
        marks.extend([0u8, 1]);
        marks.extend(item.to_be_bytes());
        marks.extend((timing.input + offset).to_be_bytes());
        marks.extend(0xFFFFu16.to_be_bytes());
        marks.extend(0u32.to_be_bytes());
    }
    out.extend((marks.len() as u32).to_be_bytes());
    out.extend(marks);
    be32(&mut out, 8, list_at as u32);
    be32(&mut out, 12, mark_at as u32);
    Ok(out)
}

fn index_bdmv(bluray: &Bluray) -> Vec<u8> {
    let mut out = vec![0u8; 40];
    put(
        &mut out,
        0,
        if bluray.uhd { b"INDX0300" } else { b"INDX0200" },
    );
    out.extend(34u32.to_be_bytes());
    out.extend([0u8; 34]);
    let indexes_at = out.len();
    let mut indexes = Vec::new();
    // First playback and top menu: HDMV movie object 0.
    for _ in 0..2 {
        indexes.extend([0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
    indexes.extend((bluray.playlists.len() as u16).to_be_bytes());
    for i in 0..bluray.playlists.len() {
        let mut title = [0u8; 12];
        title[0] = 0x40;
        be16(&mut title, 6, i as u16);
        indexes.extend(title);
    }
    out.extend((indexes.len() as u32).to_be_bytes());
    out.extend(indexes);
    be32(&mut out, 8, indexes_at as u32);
    out
}

fn movie_object(bluray: &Bluray) -> Vec<u8> {
    let mut out = vec![0u8; 40];
    put(&mut out, 0, b"MOBJ0200");
    let mut objects = vec![0u8; 4];
    objects.extend((bluray.playlists.len() as u16).to_be_bytes());
    for playlist in &bluray.playlists {
        objects.extend(0u16.to_be_bytes());
        objects.extend(1u16.to_be_bytes());
        // PLAY PL <id>
        objects.extend(0x2280_0000u32.to_be_bytes());
        objects.extend(playlist.id.parse::<u32>().unwrap_or(0).to_be_bytes());
        objects.extend(0u32.to_be_bytes());
    }
    out.extend((objects.len() as u32).to_be_bytes());
    out.extend(objects);
    out
}

fn author_bluray(bluray: &Bluray, work: &Path) -> Result<Vec<u8>> {
    let version: &[u8; 4] = if bluray.uhd { b"0300" } else { b"0200" };
    let mut payload = BTreeMap::new();
    let mut timings = BTreeMap::new();
    for clip in &bluray.clips {
        let data = read(work, clip.m2ts)?;
        let timing = clip_timing(&data, clip.video.rate)?;
        for pid in
            std::iter::once(VIDEO_PID).chain((0..clip.audio.len()).map(|i| AUDIO_PID + i as u16))
        {
            if !timing.pids.contains(&pid) {
                return fail(format!("{} carries no PID {pid:#x}", clip.m2ts));
            }
        }
        let data = if bluray.scrambled {
            super::inject::apply(
                &super::inject::Inject::TsScramble { packet_size: 192 },
                &data,
            )?
        } else {
            data
        };
        payload.insert(clip.id, data);
        timings.insert(clip.id, (timing, clip));
    }
    let index = index_bdmv(bluray);
    let mobj = movie_object(bluray);
    let mut files = vec![
        ("BDMV/index.bdmv".to_owned(), index.clone()),
        ("BDMV/MovieObject.bdmv".to_owned(), mobj.clone()),
    ];
    let mut backup = vec![
        ("BDMV/BACKUP/index.bdmv".to_owned(), index),
        ("BDMV/BACKUP/MovieObject.bdmv".to_owned(), mobj),
    ];
    for playlist in &bluray.playlists {
        let bytes = mpls(playlist, &timings, version)?;
        files.push((format!("BDMV/PLAYLIST/{}.mpls", playlist.id), bytes.clone()));
        backup.push((format!("BDMV/BACKUP/PLAYLIST/{}.mpls", playlist.id), bytes));
    }
    for clip in &bluray.clips {
        let bytes = clpi(clip, &timings[clip.id].0, version);
        files.push((format!("BDMV/CLIPINF/{}.clpi", clip.id), bytes.clone()));
        backup.push((format!("BDMV/BACKUP/CLIPINF/{}.clpi", clip.id), bytes));
    }
    for clip in &bluray.clips {
        files.push((
            format!("BDMV/STREAM/{}.m2ts", clip.id),
            payload.remove(clip.id).unwrap_or_default(),
        ));
    }
    files.extend(backup);
    let tree = Tree {
        files,
        empty_dirs: vec![
            "BDMV/AUXDATA".into(),
            "BDMV/JAR".into(),
            "BDMV/META".into(),
            "CERTIFICATE".into(),
        ],
    };
    let revision = if bluray.uhd { 0x0260 } else { 0x0250 };
    Ok(image(&Filesystem::UdfMetadata { revision }, bluray.label, &tree)?.0)
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_owned()).collect()
}

fn ffmpeg(parts: Vec<Vec<String>>) -> Step {
    Step::Ffmpeg(parts.concat())
}

/// One DVD cell: `seconds` of MPEG-2 at DVD geometry with the given audio
/// streams (encoder, layout, bitrate), muxed by FFmpeg's DVD muxer.
fn dvd_cell(name: &str, pal: bool, wide: bool, seconds: u32, audio: &[(&str, &str, &str)]) -> Step {
    let (size, rate, target) = if pal {
        ("720x576", "25", "pal-dvd")
    } else {
        ("720x480", "30000/1001", "ntsc-dvd")
    };
    let mut a = vec![s(&[
        "-f",
        "lavfi",
        "-i",
        &format!("smptebars=size={size}:rate={rate}:duration={seconds}"),
    ])];
    for (_, layout, _) in audio {
        a.push(s(&[
            "-f",
            "lavfi",
            "-t",
            &seconds.to_string(),
            "-i",
            &format!("anullsrc=r=48000:cl={layout}"),
        ]));
    }
    a.push(s(&["-map", "0:v"]));
    for i in 0..audio.len() {
        a.push(s(&["-map", &format!("{}:a", i + 1)]));
    }
    a.push(s(&["-target", target, "-b:v", "300k", "-maxrate", "1000k"]));
    if wide {
        a.push(s(&["-aspect", "16:9"]));
    }
    for (i, (codec, _, bitrate)) in audio.iter().enumerate() {
        a.push(s(&[&format!("-c:a:{i}"), codec]));
        if !bitrate.is_empty() {
            a.push(s(&[&format!("-b:a:{i}"), bitrate]));
        }
    }
    a.push(s(&["-f", "dvd", &format!("{{work}}/{name}")]));
    ffmpeg(a)
}

/// One Blu-ray clip: a second of video plus AC-3 per layout, as 192-byte
/// M2TS (FFmpeg's m2ts mode assigns the Blu-ray PIDs).
fn bd_clip(name: &str, size: &str, video: Vec<String>, audio: &[&str]) -> Step {
    let mut a = vec![s(&[
        "-f",
        "lavfi",
        "-i",
        &format!("smptehdbars=size={size}:rate=24000/1001:duration=1"),
    ])];
    for layout in audio {
        a.push(s(&[
            "-f",
            "lavfi",
            "-t",
            "1",
            "-i",
            &format!("anullsrc=r=48000:cl={layout}"),
        ]));
    }
    a.push(s(&["-map", "0:v"]));
    for i in 0..audio.len() {
        a.push(s(&["-map", &format!("{}:a", i + 1)]));
    }
    a.push(video);
    a.push(s(&[
        "-c:a",
        "ac3",
        "-b:a",
        "192k",
        "-mpegts_m2ts_mode",
        "1",
        "-f",
        "mpegts",
        &format!("{{work}}/{name}"),
    ]));
    ffmpeg(a)
}

fn x264_bd() -> Vec<String> {
    s(&[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "35",
        "-pix_fmt",
        "yuv420p",
    ])
}

fn x265_bd(params: &str) -> Vec<String> {
    let mut a = s(&[
        "-c:v",
        "libx265",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p10le",
    ]);
    a.extend(s(&[
        "-x265-params",
        &format!("log-level=error:crf=35:{params}"),
    ]));
    a
}

const MASTER_X265: &str = "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,50)";
const HDR10_PARAMS: &str = "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:repeat-headers=1:hdr10=1:max-cll=1000,400";

fn disc_step(disc: Disc) -> Step {
    Step::Post(Post::Disc(Box::new(disc)))
}

fn scramble(file: &str, inject: super::inject::Inject) -> Step {
    Step::Post(Post::Inject {
        from: format!("{{work}}/{file}"),
        to: format!("{{work}}/{file}"),
        inject,
    })
}

fn iso(name: &str, steps: Vec<Step>, expect: Expect, encoders: &[&'static str]) -> Fixture {
    Fixture {
        name: format!("disc/{name}"),
        generator: Generator::Disc,
        steps,
        expect,
        encoders: encoders.to_vec(),
    }
}

fn dvd_audio(
    format: DvdAudioFormat,
    channels: u8,
    language: &'static str,
    extension: u8,
) -> DvdAudio {
    DvdAudio {
        format,
        channels,
        language,
        extension,
    }
}

/// What a disc whose payload is flagged scrambled reports: the navigation is
/// read, the title is marked encrypted and none is selected.
fn encrypted(disc_type: &str) -> Expect {
    let mut e = Expect::audio_only("iso")
        .check("details.disc.disc_type", disc_type)
        .check("details.disc.titles.0.report.status", "encrypted")
        .absent("details.disc.selected_title_id");
    e.min_duration_seconds = 0;
    e
}

pub(crate) fn fixtures() -> Vec<Fixture> {
    let mut v = Vec::new();

    // NTSC DVD: one title of three chapters (one cell each); AC-3 5.1
    // English, MPEG-1 Layer II stereo French commentary and LPCM stereo
    // German; two declared subpicture streams (English, forced Spanish).
    let ntsc_audio: &[(&str, &str, &str)] = &[
        ("ac3", "5.1", "384k"),
        ("mp2", "stereo", "128k"),
        ("pcm_s16be", "stereo", ""),
    ];
    let ntsc = Dvd {
        filesystem: Filesystem::Iso9660,
        label: "CORPUS_DVD_NTSC",
        pal: false,
        wide: false,
        audio: vec![
            dvd_audio(DvdAudioFormat::Ac3, 6, "en", 1),
            dvd_audio(DvdAudioFormat::Mpeg1, 2, "fr", 3),
            dvd_audio(DvdAudioFormat::Lpcm, 2, "de", 1),
        ],
        subpictures: vec![
            DvdSubpicture {
                language: "en",
                extension: 1,
            },
            DvdSubpicture {
                language: "es",
                extension: 9,
            },
        ],
        titles: vec![vec![
            DvdCell {
                vob: "c1.vob",
                frames: 30,
            },
            DvdCell {
                vob: "c2.vob",
                frames: 30,
            },
            DvdCell {
                vob: "c3.vob",
                frames: 30,
            },
        ]],
    };
    let cells = ["c1.vob", "c2.vob", "c3.vob"];
    let ntsc_steps = |disc: Dvd, scrambled: bool| {
        let mut steps: Vec<Step> = cells
            .iter()
            .map(|c| dvd_cell(c, false, false, 1, ntsc_audio))
            .collect();
        if scrambled {
            steps.extend(
                cells
                    .iter()
                    .map(|c| scramble(c, super::inject::Inject::PsScramble)),
            );
        }
        steps.push(disc_step(Disc::Dvd(disc)));
        steps
    };
    let dvd_encoders: &[&'static str] = &["mpeg2video", "ac3", "mp2", "pcm_s16be"];
    let ntsc_expect = |filesystem: &str| {
        let mut e = Expect::video("iso", "mpeg2video", 720, 480, 30)
            .audio("ac3", 6, Some("en"))
            .audio("mp2", 2, Some("fr"))
            .audio("pcm_dvd", 2, Some("de"))
            .check("details.disc.disc_type", "dvd")
            .check("details.disc.filesystem", filesystem)
            .check("summary.num_chapters", 3i64)
            .check("summary.has_multiaudio", true);
        e.subtitle_stream_count = 2;
        e.min_duration_seconds = 2;
        e
    };
    v.push(iso(
        "dvd_ntsc_iso9660.iso",
        ntsc_steps(ntsc.clone(), false),
        ntsc_expect("ISO9660").check("details.disc.volume_label", "CORPUS_DVD_NTSC"),
        dvd_encoders,
    ));
    let udf = Dvd {
        filesystem: Filesystem::UdfPhysical { revision: 0x0102 },
        ..ntsc.clone()
    };
    v.push(iso(
        "dvd_ntsc_udf102.iso",
        ntsc_steps(udf, false),
        ntsc_expect("UDF"),
        dvd_encoders,
    ));
    // Scrambling flagged in the video PES headers, as CSS does.
    v.push(iso(
        "dvd_css_flagged.iso",
        ntsc_steps(ntsc, true),
        encrypted("dvd"),
        dvd_encoders,
    ));

    // PAL 16:9 with two titles: a one-second extra first, then a two-second
    // feature in two chapters. Automatic selection takes the longer title.
    let pal_audio: &[(&str, &str, &str)] = &[("ac3", "stereo", "192k")];
    let mut steps: Vec<Step> = ["x1.vob", "f1.vob", "f2.vob"]
        .iter()
        .map(|c| dvd_cell(c, true, true, 1, pal_audio))
        .collect();
    steps.push(disc_step(Disc::Dvd(Dvd {
        filesystem: Filesystem::Iso9660,
        label: "CORPUS_DVD_PAL",
        pal: true,
        wide: true,
        audio: vec![dvd_audio(DvdAudioFormat::Ac3, 2, "it", 1)],
        subpictures: vec![],
        titles: vec![
            vec![DvdCell {
                vob: "x1.vob",
                frames: 25,
            }],
            vec![
                DvdCell {
                    vob: "f1.vob",
                    frames: 25,
                },
                DvdCell {
                    vob: "f2.vob",
                    frames: 25,
                },
            ],
        ],
    })));
    let mut e = Expect::video("iso", "mpeg2video", 720, 576, 25)
        .audio("ac3", 2, Some("it"))
        .check("details.disc.disc_type", "dvd")
        .check("details.disc.filesystem", "ISO9660")
        .check("summary.num_chapters", 2i64)
        .check("details.disc.selected_title_id", "2");
    e.min_duration_seconds = 2;
    v.push(iso(
        "dvd_pal_wide_two_titles.iso",
        steps,
        e,
        &["mpeg2video", "ac3"],
    ));

    // Blu-ray: H.264 1080p, a two-clip playlist with a chapter mark per clip
    // and a one-clip playlist; English and Japanese AC-3 in each clip.
    let h264 = ClipVideo {
        coding: 0x1B,
        format: 6,
        rate: 1,
        dynamic_range: 0,
        colour_space: 0,
        hdr10plus: false,
    };
    let ac3 = |presentation, language| ClipAudio {
        coding: 0x81,
        presentation,
        language,
    };
    let bd = Bluray {
        label: "CORPUS_BD",
        uhd: false,
        scrambled: false,
        clips: vec![
            Clip {
                id: "00001",
                m2ts: "a.m2ts",
                video: h264,
                audio: vec![ac3(3, "eng"), ac3(3, "jpn")],
            },
            Clip {
                id: "00002",
                m2ts: "b.m2ts",
                video: h264,
                audio: vec![ac3(3, "eng"), ac3(3, "jpn")],
            },
        ],
        playlists: vec![
            Playlist {
                id: "00000",
                items: vec!["00001", "00002"],
                marks: vec![(0, 0), (1, 0)],
            },
            Playlist {
                id: "00001",
                items: vec!["00002"],
                marks: vec![(0, 0)],
            },
        ],
    };
    let bd_steps = |bd: Bluray, scrambled: bool| {
        vec![
            bd_clip("a.m2ts", "1920x1080", x264_bd(), &["stereo", "stereo"]),
            bd_clip("b.m2ts", "1920x1080", x264_bd(), &["stereo", "stereo"]),
            disc_step(Disc::Bluray(Bluray { scrambled, ..bd })),
        ]
    };
    let mut e = Expect::video("iso", "h264", 1920, 1080, 24)
        .audio("ac3", 2, Some("eng"))
        .audio("ac3", 2, Some("jpn"))
        .check("details.disc.disc_type", "bluray")
        .check("details.disc.filesystem", "UDF")
        .check("details.disc.selected_title_id", "00000")
        .check("summary.num_chapters", 2i64);
    e.min_duration_seconds = 2;
    v.push(iso(
        "bluray_h264_two_clips.iso",
        bd_steps(bd.clone(), false),
        e,
        &["libx264", "ac3"],
    ));
    // Scrambling flagged in the transport packet headers, as AACS does.
    v.push(iso(
        "bluray_aacs_flagged.iso",
        bd_steps(bd, true),
        encrypted("bluray"),
        &["libx264", "ac3"],
    ));

    // UHD Blu-ray: HEVC Main 10 2160p, with what the clip information
    // declares and what the bitstream carries varied independently.
    let hevc = |dynamic_range, colour_space, hdr10plus| ClipVideo {
        coding: 0x24,
        format: 8,
        rate: 1,
        dynamic_range,
        colour_space,
        hdr10plus,
    };
    let uhd = |video: ClipVideo, params: String, label: &'static str, extra: Vec<Step>| {
        let mut steps = extra;
        steps.push(bd_clip("u.m2ts", "3840x2160", x265_bd(&params), &["5.1"]));
        steps.push(disc_step(Disc::Bluray(Bluray {
            label,
            uhd: true,
            scrambled: false,
            clips: vec![Clip {
                id: "00000",
                m2ts: "u.m2ts",
                video,
                audio: vec![ac3(6, "eng")],
            }],
            playlists: vec![Playlist {
                id: "00800",
                items: vec!["00000"],
                marks: vec![(0, 0)],
            }],
        })));
        steps
    };
    let uhd_expect = || {
        let mut e = Expect::video("iso", "hevc", 3840, 2160, 24)
            .audio("ac3", 6, Some("eng"))
            .check("details.disc.disc_type", "uhd_bluray")
            .check("summary.num_chapters", 1i64);
        e.video_bit_depth = Some(10);
        e
    };
    let hdr10 = format!("{HDR10_PARAMS}:master-display={MASTER_X265}");
    let mut e = uhd_expect().check("video.metadata.hdr.hdr10", true);
    e.hdr_format = Some("HDR10");
    v.push(iso(
        "uhd_hdr10.iso",
        uhd(hevc(1, 2, false), hdr10.clone(), "CORPUS_UHD", vec![]),
        e,
        &["libx265", "ac3"],
    ));

    let mut e = uhd_expect().check("video.metadata.hdr.hdr10plus", true);
    e.hdr_format = Some("HDR10+");
    let json = Step::WriteFile {
        name: "hdr10plus.json".into(),
        contents: super::coverage::HDR10PLUS_JSON.into(),
    };
    let plus = format!("{hdr10}:dhdr10-info={{work}}/hdr10plus.json");
    v.push(iso(
        "uhd_hdr10plus.iso",
        uhd(hevc(1, 2, true), plus, "CORPUS_UHD_PLUS", vec![json]),
        e,
        &["libx265", "ac3"],
    ));

    // The clip information declares Dolby Vision over an HDR10 base layer
    // that carries no RPU: the declaration alone.
    let mut e = uhd_expect().check("video.metadata.hdr.dolby_vision", true);
    e.hdr_format = Some("Dolby Vision");
    v.push(iso(
        "uhd_dolby_vision_declared.iso",
        uhd(hevc(2, 2, false), hdr10, "CORPUS_UHD_DV", vec![]),
        e,
        &["libx265", "ac3"],
    ));

    // Conflict: HDR10 declared over a 10-bit BT.709 SDR stream. The
    // bitstream wins and the analysis says so.
    let sdr = "colorprim=bt709:transfer=bt709:colormatrix=bt709:repeat-headers=1".to_owned();
    let e = uhd_expect()
        .warning("disc_hdr_signaling_conflict")
        .absent("summary.video_hdr_format");
    v.push(iso(
        "uhd_hdr10_declared_over_sdr.iso",
        uhd(hevc(1, 2, false), sdr, "CORPUS_UHD_CONFLICT", vec![]),
        e,
        &["libx265", "ac3"],
    ));

    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        Tree {
            files: vec![
                ("BDMV/index.bdmv".into(), vec![1; 10]),
                ("BDMV/STREAM/00000.m2ts".into(), vec![2; 5000]),
                ("TOP.TXT".into(), vec![3; 3]),
            ],
            empty_dirs: vec!["CERTIFICATE".into()],
        }
    }

    #[test]
    fn udf_crc_matches_the_ccitt_check_value() {
        assert_eq!(udf_crc(b"123456789"), 0x31C3);
    }

    #[test]
    fn directories_list_parents_first() {
        assert_eq!(
            tree().directories(),
            ["", "BDMV", "CERTIFICATE", "BDMV/STREAM"]
        );
    }

    #[test]
    fn every_filesystem_places_file_bytes_where_it_reports() {
        for fs in [
            Filesystem::Iso9660,
            Filesystem::UdfPhysical { revision: 0x0102 },
            Filesystem::UdfMetadata { revision: 0x0250 },
        ] {
            let t = tree();
            let (image, placed) = image(&fs, "LABEL", &t).unwrap();
            assert_eq!(image.len() % SECTOR, 0);
            for (path, data) in &t.files {
                let at = placed[path] as usize * SECTOR;
                assert_eq!(&image[at..at + data.len()], &data[..], "{fs:?} {path}");
            }
        }
    }

    #[test]
    fn udf_anchors_point_at_a_terminated_sequence_with_valid_tags() {
        let (image, _) =
            image(&Filesystem::UdfMetadata { revision: 0x0260 }, "L", &tree()).unwrap();
        let last = image.len() / SECTOR - 1;
        for anchor in [ANCHOR, last] {
            let d = &image[anchor * SECTOR..];
            assert_eq!(u16::from_le_bytes([d[0], d[1]]), 2);
            assert_eq!(
                u32::from_le_bytes(d[12..16].try_into().unwrap()) as usize,
                anchor
            );
        }
        let main = u32::from_le_bytes(
            image[ANCHOR * SECTOR + 20..ANCHOR * SECTOR + 24]
                .try_into()
                .unwrap(),
        ) as usize;
        let kinds: Vec<u16> = (0..5)
            .map(|i| {
                u16::from_le_bytes([image[(main + i) * SECTOR], image[(main + i) * SECTOR + 1]])
            })
            .collect();
        assert_eq!(kinds, [1, 5, 6, 7, 8]);
        for i in 0..5 {
            let d = &image[(main + i) * SECTOR..];
            let length = usize::from(u16::from_le_bytes([d[10], d[11]]));
            assert_eq!(
                udf_crc(&d[16..16 + length]),
                u16::from_le_bytes([d[8], d[9]])
            );
            let sum = d[..16]
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != 4)
                .fold(0u8, |s, (_, b)| s.wrapping_add(*b));
            assert_eq!(sum, d[4]);
        }
    }

    #[test]
    fn dvd_time_is_bcd_with_the_rate_flag() {
        assert_eq!(dvd_time(25 * 61 + 3, true), [0x00, 0x01, 0x01, 0x43]);
        assert_eq!(dvd_time(30 * 3600 + 29, false), [0x01, 0x00, 0x00, 0xE9]);
    }

    #[test]
    fn pts_is_read_from_a_pes_header() {
        let pts: u64 = 0x1_2345_6789;
        let mut pes = vec![0, 0, 1, 0xE0, 0, 0, 0x80, 0x80, 5];
        pes.push(0x21 | ((pts >> 29) & 0x0E) as u8);
        pes.push((pts >> 22) as u8);
        pes.push(((pts >> 14) & 0xFE) as u8 | 1);
        pes.push((pts >> 7) as u8);
        pes.push(((pts << 1) & 0xFE) as u8 | 1);
        assert_eq!(pes_pts(&pes), Some(pts));
    }
}
