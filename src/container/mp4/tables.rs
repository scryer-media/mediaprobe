//! Sizes and counts that `mp4parse` allocates from, checked against the
//! bytes behind them before `mp4parse` sees the buffer.
//!
//! `mp4parse` 0.17 reserves memory from a size or count it has just read,
//! before it reads the bytes that size describes:
//!
//! - a table's entries from its 32-bit count (`stts`, `stsc`, `stsz`,
//!   `stco`, `co64`, `stss`, `ctts`, `elst`, `stsd`, and the key IDs of a
//!   `pssh`), and a `pssh` payload from its 32-bit data size;
//! - a box it reads whole, from the box's declared size: `ftyp`, `hdlr`,
//!   `pssh` and `auxl`; in a sample entry `avcC`, `d263`, `esds` and `av1C`,
//!   and the metadata blocks of `dfLa`; in `udta` the `data` of a metadata
//!   item.
//!
//! A nested box's declared size is not held to its parent, so one corrupt
//! size or count asks for up to 2^64 bytes however small the file is. The
//! walk follows the boxes `mp4parse` descends into, in the order it reads
//! them, and reads a box that runs past its parent only as far as the
//! parent's end, as `mp4parse` does. A size or count that `mp4parse` would
//! fail on once it ran out of bytes is refused here first, which changes no
//! result, only what was allocated before the failure. Where `mp4parse`
//! carries on past such a box (anywhere in `udta`, and in a sample entry it
//! may already have given up), the box's size is lowered to the bytes
//! present instead, which reads the same bytes.
//!
//! `mp4parse` also keeps a record for every track, sample description,
//! protection scheme, FLAC metadata block and key ID it reads, each many
//! times the size of the bytes that declare it. Those records are held to
//! [`MP4PARSE_RECORD_BUDGET`].

use super::*;
use std::mem::size_of;

/// Bytes of the per-box records `mp4parse` may keep. Real files use a few
/// kilobytes; a hostile one could otherwise turn the 16 MiB metadata budget
/// into gigabytes of track records.
pub(super) const MP4PARSE_RECORD_BUDGET: usize = 16 * 1024 * 1024;

/// Fixed fields of a visual sample entry that `mp4parse` reads or skips
/// before the entry's child boxes.
const VIDEO_ENTRY_FIELDS: usize = 78;

/// Something in the metadata `mp4parse` would size an allocation from
/// without the bytes to back it, or more records than the budget allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mp4parseRefusal {
    /// A table whose entry count claims more entries than its box holds.
    Count { name: [u8; 4], count: u32 },
    /// A `pssh` whose data size claims more bytes than the box holds.
    DataSize { name: [u8; 4], size: u32 },
    /// A box `mp4parse` reads whole whose declared size runs past its
    /// parent.
    Overrun {
        name: [u8; 4],
        size: u64,
        available: usize,
    },
    /// More per-box records than [`MP4PARSE_RECORD_BUDGET`].
    Records,
}

impl std::fmt::Display for Mp4parseRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Count { name, count } => write!(
                f,
                "{} entry count {} exceeds its box",
                fourcc_to_string(name),
                count
            ),
            Self::DataSize { name, size } => write!(
                f,
                "{} data size {} exceeds its box",
                fourcc_to_string(name),
                size
            ),
            Self::Overrun {
                name,
                size,
                available,
            } => write!(
                f,
                "{} box size {} exceeds the {} bytes left in its parent",
                fourcc_to_string(name),
                size,
                available
            ),
            Self::Records => f.write_str("track and sample records exceed the parser budget"),
        }
    }
}

/// Refuses what `mp4parse` would allocate for without the bytes to back it,
/// lowers what it would read short but tolerate, and lowers an overstated
/// `stsd` count to the entries present. `mp4parse` stops reading sample
/// descriptions at the end of the box, so the lower count reads exactly the
/// entries the original one did.
pub(super) fn bound_mp4parse_input(data: &mut [u8]) -> Result<(), Mp4parseRefusal> {
    let mut walk = Walk {
        records_left: MP4PARSE_RECORD_BUDGET,
    };
    let whole = Range {
        start: 0,
        end: data.len(),
    };
    for_each_child(data, whole, |data, child| {
        match &child.name {
            b"ftyp" => refuse_overrun(child)?,
            b"moov" => walk.moov(data, child.payload)?,
            _ => {}
        }
        Ok(Flow::Next)
    })
}

/// How `mp4parse` reads a track's sample entries, from the last handler it
/// read for the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handler {
    Video,
    Audio,
    /// Metadata and unknown handlers: `mp4parse` skips their entries whole.
    Other,
}

struct Walk {
    records_left: usize,
}

impl Walk {
    fn charge(&mut self, bytes: usize) -> Result<(), Mp4parseRefusal> {
        self.records_left = self
            .records_left
            .checked_sub(bytes)
            .ok_or(Mp4parseRefusal::Records)?;
        Ok(())
    }

    fn moov(&mut self, data: &mut [u8], payload: Range<usize>) -> Result<(), Mp4parseRefusal> {
        for_each_child(data, payload, |data, child| {
            match &child.name {
                b"trak" => {
                    self.charge(size_of::<mp4parse::Track>())?;
                    self.trak(data, child.payload)?;
                }
                b"pssh" => self.pssh(data, child)?,
                b"udta" => clamp_udta(data, child.payload),
                _ => {}
            }
            Ok(Flow::Next)
        })
    }

    fn trak(&mut self, data: &mut [u8], payload: Range<usize>) -> Result<(), Mp4parseRefusal> {
        let mut handler = Handler::Other;
        for_each_child(data, payload, |data, child| {
            match &child.name {
                b"edts" => for_each_child(data, child.payload, |data, child| {
                    if &child.name == b"elst" {
                        bound_count(&data[child.payload.start..child.payload.end], child.name)?;
                    }
                    Ok(Flow::Next)
                })?,
                b"tref" => for_each_child(data, child.payload, |_, child| {
                    if &child.name == b"auxl" {
                        refuse_overrun(child)?;
                    }
                    Ok(Flow::Next)
                })?,
                b"mdia" => self.mdia(data, child.payload, &mut handler)?,
                _ => {}
            }
            Ok(Flow::Next)
        })
    }

    fn mdia(
        &mut self,
        data: &mut [u8],
        payload: Range<usize>,
        handler: &mut Handler,
    ) -> Result<(), Mp4parseRefusal> {
        for_each_child(data, payload, |data, child| {
            match &child.name {
                b"hdlr" => {
                    refuse_overrun(child)?;
                    // Version and flags, pre_defined, then the handler type.
                    let body = &data[child.payload.start..child.payload.end];
                    *handler = match body.get(8..12) {
                        Some(b"vide" | b"pict" | b"auxv") => Handler::Video,
                        Some(b"soun") => Handler::Audio,
                        Some(b"meta") => Handler::Other,
                        _ => *handler,
                    };
                }
                b"minf" => for_each_child(data, child.payload, |data, child| {
                    if &child.name == b"stbl" {
                        self.stbl(data, child.payload, *handler)?;
                    }
                    Ok(Flow::Next)
                })?,
                _ => {}
            }
            Ok(Flow::Next)
        })
    }

    fn stbl(
        &mut self,
        data: &mut [u8],
        payload: Range<usize>,
        handler: Handler,
    ) -> Result<(), Mp4parseRefusal> {
        for_each_child(data, payload, |data, child| {
            match &child.name {
                b"stsd" => self.stsd(data, child.payload, handler)?,
                b"stts" | b"stsc" | b"stsz" | b"stco" | b"co64" | b"stss" | b"ctts" => {
                    bound_count(&data[child.payload.start..child.payload.end], child.name)?
                }
                _ => {}
            }
            Ok(Flow::Next)
        })
    }

    fn stsd(
        &mut self,
        data: &mut [u8],
        payload: Range<usize>,
        handler: Handler,
    ) -> Result<(), Mp4parseRefusal> {
        // Version and flags, then the entry count.
        let Some(count) = data
            .get(payload.start + 4..payload.end)
            .and_then(read_be_u32)
        else {
            return Ok(());
        };
        let first = payload.start + 8;
        // The entries `mp4parse` reads: it stops at a header that does not
        // fit, and fails at a malformed one, which must stay within the
        // count so that it still fails there.
        let mut present = 0_u32;
        let mut pos = first;
        while present < count {
            match next_child(data, pos, payload.end) {
                Next::End => break,
                Next::Malformed => {
                    present += 1;
                    break;
                }
                Next::Box(_, next) => {
                    present += 1;
                    pos = next;
                }
            }
        }
        if present < count {
            data[payload.start + 4..first].copy_from_slice(&present.to_be_bytes());
        }
        self.charge(
            usize::try_from(present)
                .unwrap_or(usize::MAX)
                .saturating_mul(size_of::<mp4parse::SampleEntry>()),
        )?;
        if handler == Handler::Other {
            return Ok(());
        }
        let mut pos = first;
        for _ in 0..present {
            let Next::Box(entry, next) = next_child(data, pos, payload.end) else {
                break;
            };
            match handler {
                Handler::Video => self.video_entry(data, &entry)?,
                Handler::Audio => self.audio_entry(data, &entry)?,
                Handler::Other => {}
            }
            pos = next;
        }
        Ok(())
    }

    /// A visual sample entry's children. `mp4parse` gives the entry up,
    /// without failing, when a child is unsupported, and reads no further
    /// children; after a child whose support is not checked here, a later
    /// short box is lowered rather than refused.
    fn video_entry(&mut self, data: &mut [u8], entry: &Child) -> Result<(), Mp4parseRefusal> {
        let start = (entry.payload.start + VIDEO_ENTRY_FIELDS).min(entry.payload.end);
        let mut uncertain = false;
        let children = Range {
            start,
            end: entry.payload.end,
        };
        for_each_child(data, children, |data, child| {
            let first = data[child.payload.start..child.payload.end]
                .first()
                .copied();
            match &child.name {
                b"avcC" | b"d263" | b"esds" => read_whole(data, child, uncertain)?,
                b"av1C" => {
                    // `mp4parse` reads the bytes present, then gives the
                    // entry up when the marker byte is wrong: reading just
                    // those bytes changes nothing.
                    let marked = first == Some(0x81);
                    if first.is_some() && !marked {
                        clamp_overrun(data, child);
                    } else {
                        read_whole(data, child, uncertain)?;
                    }
                    if !marked {
                        return Ok(Flow::Stop);
                    }
                }
                b"vpcC" if !matches!(first, Some(0 | 1)) => return Ok(Flow::Stop),
                b"sinf" => {
                    self.charge(size_of::<mp4parse::ProtectionSchemeInfoBox>())?;
                    uncertain = true;
                }
                _ => {}
            }
            Ok(Flow::Next)
        })
    }

    /// An audio sample entry's children, read as for [`Self::video_entry`].
    fn audio_entry(&mut self, data: &mut [u8], entry: &Child) -> Result<(), Mp4parseRefusal> {
        let body = &data[entry.payload.start..entry.payload.end];
        // Fixed fields by sound description version; `mp4parse` gives up
        // other versions before the children.
        let fields = match body.get(8..).and_then(read_be_u16) {
            Some(0) => 28,
            Some(1) => 44,
            Some(2) => 64,
            _ => return Ok(()),
        };
        let children = Range {
            start: (entry.payload.start + fields).min(entry.payload.end),
            end: entry.payload.end,
        };
        let mut uncertain = false;
        for_each_child(data, children, |data, child| {
            let first = data[child.payload.start..child.payload.end]
                .first()
                .copied();
            match &child.name {
                b"esds" => {
                    read_whole(data, child, uncertain)?;
                    uncertain = true;
                }
                b"dfLa" => {
                    // An unsupported version gives the entry up before any
                    // block is read.
                    if first != Some(0) {
                        return Ok(Flow::Stop);
                    }
                    read_whole(data, child, uncertain)?;
                    let blocks = flac_block_count(&data[child.payload.start..child.payload.end]);
                    self.charge(blocks.saturating_mul(size_of::<mp4parse::FLACMetadataBlock>()))?;
                }
                b"dOps" | b"alac" if first != Some(0) => return Ok(Flow::Stop),
                b"wave" => {
                    wave(data, child.payload, uncertain)?;
                    uncertain = true;
                }
                b"sinf" => {
                    self.charge(size_of::<mp4parse::ProtectionSchemeInfoBox>())?;
                    uncertain = true;
                }
                _ => {}
            }
            Ok(Flow::Next)
        })
    }

    /// A `pssh` is read whole, then its key IDs are reserved from their
    /// count, 16 records for every key ID, and its data from its size.
    fn pssh(&mut self, data: &[u8], child: &Child) -> Result<(), Mp4parseRefusal> {
        refuse_overrun(child)?;
        let body = &data[child.payload.start..child.payload.end];
        let Some(&version) = body.first() else {
            return Ok(());
        };
        // Version and flags, then the 16-byte system ID.
        let mut at = 20;
        let mut key_ids = 0_usize;
        if version > 0 {
            let Some(count) = body.get(at..).and_then(read_be_u32) else {
                return Ok(());
            };
            at += 4;
            let bytes = u64::from(count) * 16;
            if bytes > (body.len() - at) as u64 {
                return Err(Mp4parseRefusal::Count {
                    name: child.name,
                    count,
                });
            }
            at += bytes as usize;
            key_ids = count as usize;
        }
        let Some(size) = body.get(at..).and_then(read_be_u32) else {
            return Ok(());
        };
        at += 4;
        if u64::from(size) > (body.len() - at) as u64 {
            return Err(Mp4parseRefusal::DataSize {
                name: child.name,
                size,
            });
        }
        let per_key_id = 16 * size_of::<mp4parse::ByteData>() + 16;
        self.charge(
            key_ids
                .saturating_mul(per_key_id)
                .saturating_add(size_of::<mp4parse::ProtectionSystemSpecificHeaderBox>()),
        )
    }
}

/// A QuickTime `wave` atom in an audio entry. `mp4parse` reads its `esds`
/// and skips the rest without checking that they fit.
fn wave(
    data: &mut [u8],
    payload: Range<usize>,
    mut uncertain: bool,
) -> Result<(), Mp4parseRefusal> {
    for_each_child(data, payload, |data, child| {
        if &child.name == b"esds" {
            read_whole(data, child, uncertain)?;
            uncertain = true;
        }
        Ok(Flow::Next)
    })
}

/// `udta`, read by `mp4parse` down to the `data` of each metadata item.
/// A failure anywhere in `udta` does not fail the file, so a short `data`
/// box is lowered to the bytes present rather than refused.
fn clamp_udta(data: &mut [u8], payload: Range<usize>) {
    let lowered = for_each_child(data, payload, |data, meta| {
        if &meta.name != b"meta" {
            return Ok(Flow::Next);
        }
        // `meta` is a full box: version and flags come first.
        let items = Range {
            start: (meta.payload.start + 4).min(meta.payload.end),
            end: meta.payload.end,
        };
        for_each_child(data, items, |data, ilst| {
            if &ilst.name == b"ilst" {
                for_each_child(data, ilst.payload, |data, item| {
                    for_each_child(data, item.payload, |data, value| {
                        if &value.name == b"data" {
                            clamp_overrun(data, value);
                        }
                        Ok(Flow::Next)
                    })?;
                    Ok(Flow::Next)
                })?;
            }
            Ok(Flow::Next)
        })?;
        Ok(Flow::Next)
    });
    debug_assert!(lowered.is_ok(), "the udta walk refuses nothing");
}

/// A box `mp4parse` reads whole: refused when it runs past its parent, or
/// lowered to the bytes present when an earlier child may already have
/// ended the read.
fn read_whole(data: &mut [u8], child: &Child, uncertain: bool) -> Result<(), Mp4parseRefusal> {
    if uncertain {
        clamp_overrun(data, child);
        Ok(())
    } else {
        refuse_overrun(child)
    }
}

/// `mp4parse` reserves a box it reads whole from its declared size, then
/// fails once the parent runs out, so refusing it first changes no result.
fn refuse_overrun(child: &Child) -> Result<(), Mp4parseRefusal> {
    if child.overruns {
        return Err(Mp4parseRefusal::Overrun {
            name: child.name,
            size: child.size,
            available: child.payload.end - child.start,
        });
    }
    Ok(())
}

/// Lowers a box that runs past its parent to the bytes present, which are
/// the bytes `mp4parse` reads from it either way.
fn clamp_overrun(data: &mut [u8], child: &Child) {
    if child.overruns {
        let present = (child.payload.end - child.start) as u64;
        write_box_size_at(data, child.start, child.size_field, present);
    }
}

/// The metadata blocks `mp4parse` reads from a `dfLa` body: a 4-byte block
/// header with a 24-bit length, after the version and flags.
fn flac_block_count(body: &[u8]) -> usize {
    let mut at = 4;
    let mut blocks = 0;
    while let Some(header) = body.get(at..at + 4) {
        let length = u32::from_be_bytes([0, header[1], header[2], header[3]]);
        blocks += 1;
        at += 4 + length as usize;
    }
    blocks
}

/// Bytes per entry as `mp4parse` reads them, and where the count sits after
/// the full-box version and flags.
fn table_layout(name: &[u8; 4], body: &[u8]) -> Option<(u64, usize)> {
    Some(match name {
        b"stts" | b"co64" | b"ctts" => (8, 4),
        b"stsc" => (12, 4),
        b"stco" | b"stss" => (4, 4),
        // A constant sample size leaves the table out, and nothing is
        // reserved for it.
        b"stsz" if read_be_u32(body.get(4..)?)? == 0 => (4, 8),
        b"elst" if body.first() == Some(&1) => (20, 4),
        b"elst" => (12, 4),
        _ => return None,
    })
}

/// Refuses a sample or edit table whose count overstates its box.
/// `mp4parse` fails on such a table once it runs out of bytes, so refusing
/// it first changes no result, only what was allocated before the failure.
fn bound_count(body: &[u8], name: [u8; 4]) -> Result<(), Mp4parseRefusal> {
    let Some((entry_bytes, count_at)) = table_layout(&name, body) else {
        return Ok(());
    };
    let Some(count) = body.get(count_at..).and_then(read_be_u32) else {
        return Ok(());
    };
    let available = (body.len() - (count_at + 4)) as u64;
    if u64::from(count) * entry_bytes > available {
        return Err(Mp4parseRefusal::Count { name, count });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Next,
    Stop,
}

/// A child box as `mp4parse` reads its header.
#[derive(Debug, Clone, Copy)]
struct Child {
    name: [u8; 4],
    start: usize,
    /// 8 for a 32-bit size field, 16 for a 64-bit one.
    size_field: usize,
    /// The declared size; 0 for an unsized `mdat`.
    size: u64,
    /// The content after the header, cut at the parent's end.
    payload: Range<usize>,
    /// The declared size runs past the parent's end.
    overruns: bool,
}

enum Next {
    /// A box, and where the one after it starts.
    Box(Child, usize),
    /// No header fits: `mp4parse` ends the walk here.
    End,
    /// A header `mp4parse` fails on.
    Malformed,
}

/// Reads the header at `pos` the way `mp4parse` does, within `end`.
fn next_child(data: &[u8], pos: usize, end: usize) -> Next {
    let rest = &data[pos..end];
    let (Some(size32), Some(name)) = (
        read_be_u32(rest),
        rest.get(4..8)
            .and_then(|name| <[u8; 4]>::try_from(name).ok()),
    ) else {
        return Next::End;
    };
    let (size, size_field) = match size32 {
        0 if &name != b"mdat" => return Next::Malformed,
        // An unsized `mdat` holds nothing for `mp4parse`, which moves on
        // past its header.
        0 => (0, 8),
        1 => match rest
            .get(8..16)
            .and_then(|size| <[u8; 8]>::try_from(size).ok())
        {
            Some(size) => (u64::from_be_bytes(size), 16),
            None => return Next::End,
        },
        size => (u64::from(size), 8),
    };
    if size32 != 0 && size < size_field as u64 {
        return Next::Malformed;
    }
    let mut header = size_field;
    // A `uuid` box large enough for one carries its 16-byte type in the
    // header; `mp4parse` ends the walk when those bytes are not there.
    if &name == b"uuid" && size >= header as u64 + 16 {
        if rest.len() < header + 16 {
            return Next::End;
        }
        header += 16;
    }
    let (len, overruns) = match usize::try_from(size) {
        _ if size32 == 0 => (header, false),
        Ok(size) if size <= rest.len() => (size, false),
        _ => (rest.len(), true),
    };
    Next::Box(
        Child {
            name,
            start: pos,
            size_field,
            size,
            payload: Range {
                start: pos + header,
                end: pos + len,
            },
            overruns,
        },
        pos + len,
    )
}

/// Visits each child box in `range` the way `mp4parse` iterates them: a
/// header that does not fit ends the walk, a box past the parent's end is
/// cut at it, and a malformed or unsized box ends it, as `mp4parse` fails
/// there.
fn for_each_child(
    data: &mut [u8],
    range: Range<usize>,
    mut visit: impl FnMut(&mut [u8], &Child) -> Result<Flow, Mp4parseRefusal>,
) -> Result<(), Mp4parseRefusal> {
    let mut pos = range.start;
    while let Next::Box(child, next) = next_child(data, pos, range.end) {
        if child.size == 0 || visit(data, &child)? == Flow::Stop {
            break;
        }
        pos = next;
    }
    Ok(())
}
