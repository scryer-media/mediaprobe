//! EBML (Matroska, WebM) element trees: parse, edit, serialise.
//!
//! Like [`crate::boxes`], unreadable bytes stay opaque and round-trip
//! unchanged. Element IDs and sizes keep their original encodings unless a
//! mutation changes them, so an unmutated tree writes back byte for byte.

use crate::rng::{Rng, perturb};

/// Master elements: their payload is a list of elements.
pub const MASTERS: [u32; 34] = [
    0x1A45_DFA3, // EBML
    0x1853_8067, // Segment
    0x114D_9B74, // SeekHead
    0x4DBB,      // Seek
    0x1549_A966, // Info
    0x1654_AE6B, // Tracks
    0xAE,        // TrackEntry
    0xE0,        // Video
    0xE1,        // Audio
    0x55B0,      // Colour
    0x55D0,      // MasteringMetadata
    0x7670,      // Projection
    0x6D80,      // ContentEncodings
    0x6240,      // ContentEncoding
    0x5035,      // ContentEncryption
    0x41E4,      // BlockAdditionMapping
    0x1F43_B675, // Cluster
    0xA0,        // BlockGroup
    0x75A1,      // BlockAdditions
    0xA6,        // BlockMore
    0x1C53_BB6B, // Cues
    0xBB,        // CuePoint
    0xB7,        // CueTrackPositions
    0x1043_A770, // Chapters
    0x45B9,      // EditionEntry
    0xB6,        // ChapterAtom
    0x80,        // ChapterDisplay
    0x1254_C367, // Tags
    0x7373,      // Tag
    0x63C0,      // Targets
    0x67C8,      // SimpleTag
    0x1941_A469, // Attachments
    0x61A7,      // AttachedFile
    0xE2,        // TrackOperation
];

/// IDs the ID mutation substitutes: masters plus the leaves the parser reads.
pub const IDS: [u32; 30] = [
    0x1853_8067,
    0x1654_AE6B,
    0xAE,
    0xD7,
    0x83,
    0x86,
    0x63A2,
    0xE0,
    0xB0,
    0xBA,
    0x54B0,
    0x54BA,
    0x55B0,
    0x55B1,
    0x55B9,
    0x55BA,
    0x55BB,
    0x55BC,
    0x55D0,
    0x41E4,
    0x41ED,
    0x41E7,
    0x1F43_B675,
    0xE7,
    0xA3,
    0xA1,
    0x2AD7B1,
    0x4489,
    0x22B59C,
    0x1C53_BB6B,
];

/// A data size of all ones: "unknown size", used by live streams.
const UNKNOWN: u64 = u64::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Leaf(Vec<u8>),
    Children {
        children: Vec<Element>,
        trailer: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// The ID with its length marker, as written (1 to 4 bytes).
    pub id: u32,
    /// Encoded width of the size field (1 to 8), kept from the input.
    pub size_width: usize,
    /// Written instead of the real size: unknown size, or a lie.
    pub size_override: Option<u64>,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EbmlFile {
    pub elements: Vec<Element>,
    pub trailer: Vec<u8>,
}

fn read_id(data: &[u8]) -> Option<(u32, usize)> {
    let first = *data.first()?;
    let width = first.leading_zeros() as usize + 1;
    if width > 4 || data.len() < width {
        return None;
    }
    let id = data[..width]
        .iter()
        .fold(0u32, |acc, &b| acc << 8 | b as u32);
    Some((id, width))
}

fn read_size(data: &[u8]) -> Option<(u64, usize)> {
    let first = *data.first()?;
    let width = first.leading_zeros() as usize + 1;
    if width > 8 || data.len() < width {
        return None;
    }
    let mask = if width == 8 { 0 } else { 0xFFu8 >> width };
    let mut value = (first & mask) as u64;
    let mut all_ones = first & mask == mask;
    for &b in &data[1..width] {
        value = value << 8 | b as u64;
        all_ones &= b == 0xFF;
    }
    Some((if all_ones { UNKNOWN } else { value }, width))
}

fn id_bytes(id: u32) -> Vec<u8> {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|&&b| b == 0).count().min(3);
    bytes[skip..].to_vec()
}

/// Encode `value` in exactly `width` bytes; `None` if it does not fit.
pub fn encode_size(value: u64, width: usize) -> Option<Vec<u8>> {
    let bits = 7 * width as u32;
    if value == UNKNOWN {
        let mut out = vec![0xFF; width];
        out[0] = 0xFF >> (width - 1);
        return Some(out);
    }
    // All ones in `bits` is reserved for unknown size.
    if bits < 64 && value >= (1u64 << bits) - 1 {
        return None;
    }
    let mut out = value.to_be_bytes()[8 - width..].to_vec();
    out[0] |= 0x80 >> (width - 1);
    Some(out)
}

fn parse_list(mut data: &[u8], depth: usize) -> (Vec<Element>, Vec<u8>) {
    let mut out = Vec::new();
    while let Some((id, id_width)) = read_id(data) {
        let Some((size, size_width)) = read_size(&data[id_width..]) else {
            break;
        };
        let header = id_width + size_width;
        let master = MASTERS.contains(&id);
        let end = if size == UNKNOWN {
            // Unknown size runs to the end of the parent; only masters may.
            if !master {
                break;
            }
            data.len()
        } else {
            match header.checked_add(size as usize) {
                Some(end) if size <= data.len() as u64 && end <= data.len() => end,
                _ => break,
            }
        };
        let payload = &data[header..end];
        let body = if master && depth < 16 {
            let (children, trailer) = parse_list(payload, depth + 1);
            Body::Children { children, trailer }
        } else {
            Body::Leaf(payload.to_vec())
        };
        out.push(Element {
            id,
            size_width,
            size_override: (size == UNKNOWN).then_some(UNKNOWN),
            body,
        });
        data = &data[end..];
    }
    (out, data.to_vec())
}

impl Element {
    pub fn leaf(id: u32, payload: Vec<u8>) -> Self {
        Self {
            id,
            size_width: 8,
            size_override: None,
            body: Body::Leaf(payload),
        }
    }

    pub fn uint(id: u32, value: u64) -> Self {
        let bytes = value.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count().min(7);
        Self::leaf(id, bytes[skip..].to_vec())
    }

    pub fn master(id: u32, children: Vec<Element>) -> Self {
        Self {
            id,
            size_width: 8,
            size_override: None,
            body: Body::Children {
                children,
                trailer: Vec::new(),
            },
        }
    }

    fn payload_len(&self) -> usize {
        match &self.body {
            Body::Leaf(bytes) => bytes.len(),
            Body::Children { children, trailer } => {
                children.iter().map(Element::len).sum::<usize>() + trailer.len()
            }
        }
    }

    pub fn len(&self) -> usize {
        id_bytes(self.id).len() + self.size_width + self.payload_len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&id_bytes(self.id));
        let size = self.size_override.unwrap_or(self.payload_len() as u64);
        let encoded = encode_size(size, self.size_width)
            .or_else(|| encode_size(self.payload_len() as u64, 8))
            .expect("eight bytes hold any real payload length");
        out.extend_from_slice(&encoded);
        match &self.body {
            Body::Leaf(bytes) => out.extend_from_slice(bytes),
            Body::Children { children, trailer } => {
                for child in children {
                    child.write(out);
                }
                out.extend_from_slice(trailer);
            }
        }
    }
}

impl EbmlFile {
    pub fn parse(data: &[u8]) -> Self {
        let (elements, trailer) = parse_list(data, 0);
        Self { elements, trailer }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for e in &self.elements {
            e.write(&mut out);
        }
        out.extend_from_slice(&self.trailer);
        out
    }

    pub fn paths(&self) -> Vec<Vec<usize>> {
        fn walk(list: &[Element], at: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            for (i, e) in list.iter().enumerate() {
                at.push(i);
                out.push(at.clone());
                if let Body::Children { children, .. } = &e.body {
                    walk(children, at, out);
                }
                at.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.elements, &mut Vec::new(), &mut out);
        out
    }

    fn siblings_mut(&mut self, parent: &[usize]) -> &mut Vec<Element> {
        let mut list = &mut self.elements;
        for &i in parent {
            list = match &mut list[i].body {
                Body::Children { children, .. } => children,
                Body::Leaf(_) => unreachable!("paths only descend into masters"),
            };
        }
        list
    }

    pub fn mutate(&mut self, rng: &mut Rng) -> bool {
        let paths = self.paths();
        if paths.is_empty() {
            return false;
        }
        let path = rng.pick(&paths).clone();
        let (parent, index) = path.split_at(path.len() - 1);
        let index = index[0];
        let siblings = self.siblings_mut(parent);
        match rng.below(7) {
            0 => {
                let real = siblings[index].payload_len() as u64;
                siblings[index].size_override = Some(match rng.below(4) {
                    0 => UNKNOWN,
                    1 => real + 1 + rng.below(1 << 20) as u64,
                    2 => real.saturating_sub(1 + rng.below(16) as u64),
                    _ => 0,
                });
            }
            1 => siblings[index].size_width = 1 + rng.below(8),
            2 => siblings[index].id = *rng.pick(&IDS),
            3 => {
                let copy = siblings[index].clone();
                siblings.insert(index + 1, copy);
            }
            4 => {
                siblings.remove(index);
            }
            5 => {
                let other = rng.below(siblings.len());
                siblings.swap(index, other);
            }
            _ => match &mut siblings[index].body {
                Body::Leaf(bytes) => {
                    if bytes.is_empty() || rng.chance(8) {
                        bytes.push(rng.next_u64() as u8);
                    } else {
                        perturb(bytes, rng);
                    }
                }
                Body::Children { trailer, .. } => trailer.push(rng.next_u64() as u8),
            },
        }
        true
    }
}

pub fn mutate(data: &[u8], seed: u64, max_size: usize) -> Option<Vec<u8>> {
    let mut file = EbmlFile::parse(data);
    let mut rng = Rng::new(seed);
    let mut changed = false;
    for _ in 0..=rng.below(3) {
        changed |= file.mutate(&mut rng);
    }
    if !changed {
        return None;
    }
    let mut out = file.to_bytes();
    out.truncate(max_size);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        crate::build::mkv_with_codec(
            b"V_MPEG4/ISO/AVC",
            &crate::params::H264_AVCC,
            &crate::build::length_prefixed(&[&crate::params::H264_SPS[..]]),
            64,
            36,
        )
    }

    #[test]
    fn a_built_file_round_trips_and_is_fully_structured() {
        let bytes = sample();
        let file = EbmlFile::parse(&bytes);
        assert!(file.trailer.is_empty());
        assert_eq!(file.to_bytes(), bytes);
        assert_eq!(file.elements.len(), 2);
        assert!(file.paths().len() > 15);
    }

    #[test]
    fn sizes_encode_in_every_width_and_unknown_size_survives() {
        for width in 1..=8 {
            let enc = encode_size(5, width).unwrap();
            assert_eq!(read_size(&enc), Some((5, width)));
            let unknown = encode_size(UNKNOWN, width).unwrap();
            assert_eq!(read_size(&unknown), Some((UNKNOWN, width)));
        }
        assert!(encode_size(127, 1).is_none());
        // A live-style segment of unknown size parses and writes back.
        let mut live = vec![
            0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ];
        live.extend_from_slice(&[0xEC, 0x81, 0x00]);
        assert_eq!(EbmlFile::parse(&live).to_bytes(), live);
    }

    #[test]
    fn opaque_bytes_round_trip() {
        for data in [
            &b""[..],
            b"\x00\x00",
            b"\x1a\x45\xdf\xa3\xff",
            b"\x80\x88abc",
        ] {
            assert_eq!(EbmlFile::parse(data).to_bytes(), data);
        }
    }

    #[test]
    fn mutation_is_deterministic_and_changes_the_input() {
        let bytes = sample();
        let mut changed = 0;
        for seed in 0..200 {
            let a = mutate(&bytes, seed, 1 << 20).unwrap();
            assert_eq!(a, mutate(&bytes, seed, 1 << 20).unwrap());
            changed += usize::from(a != bytes);
        }
        assert!(
            changed > 150,
            "only {changed} of 200 seeds changed the input"
        );
    }
}
