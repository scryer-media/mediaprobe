//! Headers that declare payloads far larger than the read budget must not
//! make a parser size a buffer, or a read, by the declaration.

use std::io::{self, Read, Seek, SeekFrom};

use crate::source::{BoundedSource, MediaSource};

/// A source of `len` bytes: `prefix`, then zeros.
struct SparseSource {
    prefix: Vec<u8>,
    len: u64,
    position: u64,
}

impl Read for SparseSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let available = self.len.saturating_sub(self.position);
        let count = buffer
            .len()
            .min(usize::try_from(available).unwrap_or(usize::MAX));
        for (offset, byte) in buffer[..count].iter_mut().enumerate() {
            *byte = usize::try_from(self.position)
                .ok()
                .and_then(|start| self.prefix.get(start + offset))
                .copied()
                .unwrap_or(0);
        }
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for SparseSource {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = match position {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => self.len.saturating_add_signed(offset),
            SeekFrom::Current(offset) => self.position.saturating_add_signed(offset),
        };
        Ok(self.position)
    }
}

impl MediaSource for SparseSource {
    fn len(&self) -> u64 {
        self.len
    }
}

/// Sits between a parser and its budgeted source and records the largest
/// buffer the parser asked to fill, which is the size it allocated for it.
struct LargestRead {
    inner: BoundedSource<SparseSource>,
    largest: usize,
}

impl Read for LargestRead {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.largest = self.largest.max(buffer.len());
        self.inner.read(buffer)
    }
}

impl Seek for LargestRead {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

impl MediaSource for LargestRead {
    fn len(&self) -> u64 {
        self.inner.len()
    }
    fn exhausted(&self) -> bool {
        self.inner.exhausted()
    }
    fn read_allowance(&self) -> u64 {
        self.inner.read_allowance()
    }
}

const BUDGET: u64 = 64 * 1024;
/// Reads up to the budget, plus the buffering some parsers read through.
const LARGEST_ALLOWED_READ: usize = 2 * BUDGET as usize;
/// A payload size every parser here accepts, far past the budget.
const DECLARED: u64 = 8 << 20;

fn largest_read(
    prefix: Vec<u8>,
    parse: impl FnOnce(&mut dyn MediaSource) -> Result<crate::raw::RawContainer, crate::ProbeError>,
) -> usize {
    let mut source = LargestRead {
        inner: BoundedSource::new(
            SparseSource {
                prefix,
                len: 1 << 40,
                position: 0,
            },
            BUDGET,
        ),
        largest: 0,
    };
    let _ = parse(&mut source);
    source.largest
}

fn ebml(id: &[u8], size: u64) -> Vec<u8> {
    let mut element = id.to_vec();
    element.push(0x01);
    element.extend_from_slice(&size.to_be_bytes()[1..]);
    element
}

#[test]
fn oversized_declarations_read_only_within_the_budget() {
    let mut asf = vec![
        0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62, 0xce,
        0x6c,
    ];
    asf.extend_from_slice(&DECLARED.to_le_bytes());
    asf.extend_from_slice(&[1, 0, 0, 0, 1, 2]);
    let asf = largest_read(asf, super::asf::parse_asf_source);

    let mut mp4 = [&20_u32.to_be_bytes()[..], b"ftypisom", &[0; 4], b"isom"].concat();
    mp4.extend_from_slice(&1_u32.to_be_bytes());
    mp4.extend_from_slice(b"moov");
    mp4.extend_from_slice(&DECLARED.to_be_bytes());
    let mp4 = largest_read(mp4, |source| {
        super::mp4::parse_mp4_source(source, "mp4", crate::ProbeDepth::Full)
    });

    let doc_type = [&[0x42, 0x82, 0x88][..], b"matroska"].concat();
    let mut mkv = [
        &[0x1a, 0x45, 0xdf, 0xa3, 0x80 | doc_type.len() as u8][..],
        &doc_type,
    ]
    .concat();
    mkv.extend(ebml(&[0x18, 0x53, 0x80, 0x67], (1 << 56) - 1));
    mkv.extend(ebml(&[0x16, 0x54, 0xae, 0x6b], DECLARED));
    let mkv = largest_read(mkv, |source| {
        super::mkv::parse_mkv_source(source, crate::ProbeDepth::Full)
    });

    let mut flv = b"FLV\x01\x05\0\0\0\x09\0\0\0\0".to_vec();
    flv.push(0x12);
    flv.extend_from_slice(&DECLARED.to_be_bytes()[5..]);
    flv.extend_from_slice(&[0; 7]);
    let flv = largest_read(flv, super::flv::parse_flv_source);

    let realmedia = largest_read(b".RMF\0\0\0\x12\0\0".to_vec(), super::realmedia::parse);
    let ps = largest_read(vec![0, 0, 1, 0xba, 0x44], super::ps::parse_ps);
    let elementary = largest_read(vec![0, 0, 0, 1, 0x67, 0x64], super::elementary::parse_h264);

    for (name, largest) in [
        ("asf", asf),
        ("mp4", mp4),
        ("matroska", mkv),
        ("flv", flv),
        ("realmedia", realmedia),
        ("mpeg-ps", ps),
        ("elementary", elementary),
    ] {
        assert!(
            largest <= LARGEST_ALLOWED_READ,
            "{name} asked for a {largest} byte read under a {BUDGET} byte budget"
        );
    }
}
