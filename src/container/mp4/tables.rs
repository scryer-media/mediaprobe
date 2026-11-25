//! Entry counts in the boxes `mp4parse` sizes an allocation from, checked
//! against the bytes behind them before `mp4parse` sees the buffer.
//!
//! `mp4parse` 0.17 reserves a table's entries from its 32-bit count before
//! it reads a single entry, so one corrupt count asks for up to 2^32 entries
//! however small the box is. A count is held here to the entries its box can
//! actually hold. The walk follows the boxes `mp4parse` descends into
//! (`moov`, `trak`, `edts`, `mdia`, `minf`, `stbl`) and reads a box that runs
//! past its parent only as far as the parent's end, as `mp4parse` does.

use super::*;

/// A table whose entry count claims more entries than its box holds.
/// `mp4parse` fails on such a table once it runs out of bytes, so refusing it
/// first changes no result, only what was allocated before the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OversizedTable {
    pub(super) name: [u8; 4],
    pub(super) count: u32,
}

impl std::fmt::Display for OversizedTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} entry count {} exceeds its box",
            fourcc_to_string(self.name),
            self.count
        )
    }
}

/// Refuses a sample or edit table whose count overstates its box, and lowers
/// an overstated `stsd` count to the entries that fit. `mp4parse` stops
/// reading sample descriptions at the end of the box, so the lower count
/// reads exactly the entries the original one did.
pub(super) fn bound_mp4parse_tables(data: &mut [u8]) -> Result<(), OversizedTable> {
    let whole = Range {
        start: 0,
        end: data.len(),
    };
    for_each_child(data, whole, |data, name, payload| match &name {
        b"moov" => for_each_child(data, payload, |data, name, payload| match &name {
            b"trak" => for_each_child(data, payload, bound_trak_child),
            _ => Ok(()),
        }),
        _ => Ok(()),
    })
}

fn bound_trak_child(
    data: &mut [u8],
    name: [u8; 4],
    payload: Range<usize>,
) -> Result<(), OversizedTable> {
    match &name {
        b"edts" => for_each_child(data, payload, |data, name, payload| match &name {
            b"elst" => bound_count(&data[payload.start..payload.end], name),
            _ => Ok(()),
        }),
        b"mdia" => for_each_child(data, payload, |data, name, payload| match &name {
            b"minf" => for_each_child(data, payload, |data, name, payload| match &name {
                b"stbl" => for_each_child(data, payload, bound_sample_table),
                _ => Ok(()),
            }),
            _ => Ok(()),
        }),
        _ => Ok(()),
    }
}

fn bound_sample_table(
    data: &mut [u8],
    name: [u8; 4],
    payload: Range<usize>,
) -> Result<(), OversizedTable> {
    let body = &mut data[payload.start..payload.end];
    match &name {
        b"stsd" => {
            // Every sample description is at least a box header.
            if let Some(count) = body.get(4..).and_then(read_be_u32) {
                let fits = u32::try_from((body.len() - 8) / 8).unwrap_or(u32::MAX);
                if count > fits {
                    body[4..8].copy_from_slice(&fits.to_be_bytes());
                }
            }
            Ok(())
        }
        b"stts" | b"stsc" | b"stsz" | b"stco" | b"co64" | b"stss" | b"ctts" => {
            bound_count(body, name)
        }
        _ => Ok(()),
    }
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

fn bound_count(body: &[u8], name: [u8; 4]) -> Result<(), OversizedTable> {
    let Some((entry_bytes, count_at)) = table_layout(&name, body) else {
        return Ok(());
    };
    let Some(count) = body.get(count_at..).and_then(read_be_u32) else {
        return Ok(());
    };
    let available = (body.len() - (count_at + 4)) as u64;
    if u64::from(count) * entry_bytes > available {
        return Err(OversizedTable { name, count });
    }
    Ok(())
}

/// Visits each child box in `range` the way `mp4parse` iterates them: a
/// header that does not fit ends the walk, a box past the parent's end is
/// cut at it, and an unsized box other than `mdat` ends it as `mp4parse`
/// refuses one.
fn for_each_child(
    data: &mut [u8],
    range: Range<usize>,
    mut visit: impl FnMut(&mut [u8], [u8; 4], Range<usize>) -> Result<(), OversizedTable>,
) -> Result<(), OversizedTable> {
    let mut pos = range.start;
    while pos < range.end {
        let Some(header) = read_box_header_from_bytes(&data[pos..range.end]) else {
            break;
        };
        let end = if data[pos..pos + 4] == [0; 4] {
            if &header.name != b"mdat" {
                break;
            }
            pos + header.header_size
        } else {
            usize::try_from(header.size)
                .ok()
                .and_then(|size| pos.checked_add(size))
                .map_or(range.end, |end| end.min(range.end))
        };
        let payload = Range {
            start: pos + header.header_size,
            end,
        };
        visit(data, header.name, payload)?;
        pos = end;
    }
    Ok(())
}
